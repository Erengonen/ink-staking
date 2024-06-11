#![cfg_attr(not(feature = "std"), no_std)]

#[ink::contract]
mod staking {
    use ink::prelude::vec::Vec;
    use ink::storage::Mapping;
    use ink::storage::traits::{Storable, StorageLayout};
    use openbrush::contracts::traits::psp22::PSP22Ref;

    #[derive(Debug, Clone, PartialEq, Eq, scale::Encode, scale::Decode, StorageLayout)]
    #[cfg_attr(feature = "std", derive(scale_info::TypeInfo))]
    pub struct StakeInfo {
        pub amount: u128,
        pub started_at: u64,
        pub period: u32,
        pub active_until: u64,
    }

    
    #[ink(storage)]
    pub struct Staking {
        pub stakes: Mapping<AccountId, StakeInfo>,
        pub level_periods: Mapping<u32, Vec<u32>>,
        pub last_reward_claims: Mapping<AccountId, u64>,
        pub operators: Mapping<AccountId, bool>,
        pub available_periods: Vec<u32>,
        pub reward_token: AccountId,
        pub total_staked: u128,
        pub rewards_balance: u128,
        pub reward_rate: u128,
        pub early_withdraw_fee: u128,
        pub reward_conversion_rate: u128,
    }

    
    impl Staking {
        #[ink(constructor)]
        pub fn new(reward_token: AccountId, reward_conversion_rate: u128) -> Self {
            let mut available_periods = Vec::new();
            available_periods.push(6);
            available_periods.push(12);

            Self {
                stakes: Mapping::new(),
                level_periods: Mapping::new(),
                last_reward_claims: Mapping::new(),
                operators: Mapping::new(),
                available_periods,
                reward_token,
                total_staked: 0,
                rewards_balance: 0,
                reward_rate: 5,
                early_withdraw_fee: 10,
                reward_conversion_rate,
            }
        }

        #[ink(message)]
        pub fn get_staking_period(&self, account: AccountId) -> Result<u32, String> {
            self.stakes.get(&account)
                .map(|stake_info| ((stake_info.active_until - stake_info.started_at) / 86400) as u32)
                .ok_or_else(|| "Stake info not found".to_string())
        }

        #[ink(message)]
        pub fn available_rewards(&self, account: AccountId) -> Result<u128, String> {
            let (_, reward) = self.reward_amount(account)?;
            Ok(reward)
        }


        #[ink(message)]
        pub fn passed_reward_periods(&self, account: AccountId) -> Result<u32, String> {
            let (passed_periods, _) = self.reward_amount(account)?;
            Ok(passed_periods)
        }

        #[ink(message)]
        pub fn all_stake_info(&self, account: AccountId) -> Result<(u128, u64, u32, u64, u128, u64), String> {
            let stake_info = self.stakes.get(&account).ok_or_else(|| "Stake info not found".to_string())?;
            let amount = stake_info.amount;
            let started_at = stake_info.started_at;
            let period = stake_info.period;
            let active_until = stake_info.active_until;
            let (rewards, next_reward_seconds) = if amount != 0 {
                let (_, reward) = self.reward_amount(account)?;
                let next_reward_seconds = self.next_reward_date(account)?;
                (reward, next_reward_seconds)
            } else {
                (0, 0)
            };

            Ok((amount, started_at, period, active_until, rewards, next_reward_seconds))
        }

        #[ink(message)]
        pub fn next_reward_date(&self, account: AccountId) -> Result<u64, String> {
            self._next_reward_date(account)
        }

        #[ink(message, payable)]
        pub fn stake(&mut self, period: u32) -> Result<(), String> {
            let caller = self.env().caller();
            let value = self.env().transferred_value();
            assert!(value > 0, "amount should be > 0");

            let previous_amount = self.stakes.get(&caller).map(|info| info.amount).unwrap_or(0);
            if previous_amount != 0 {
                self._collect_rewards(caller, true)?;
            }
            self._stake(caller, period, value)?;
            Ok(())
        }

        #[ink(message)]
        pub fn withdraw(&mut self) -> Result<(), String> {
            let caller: ink::primitives::AccountId = self.env().caller();
            if self.stakes.get(&caller).is_none() {
                return Err("no stake".to_string());
            }
            self._collect_rewards(caller, true)?;
            let amount = self.stakes.get(&caller).ok_or_else(|| "Stake info not found".to_string())?.amount;
            self._withdraw(caller, amount)?;
            Ok(())
        }

        #[ink(message)]
        pub fn emergency_withdraw(&mut self) -> Result<(), String> {
            let caller = self.env().caller();
            if self.stakes.get(&caller).is_none() {
                return Err("no stake".to_string());
            }
            let amount = self.stakes.get(&caller).unwrap().amount;
            self._withdraw(caller, amount)?;
            self.stakes.insert(caller, &StakeInfo {
                amount: 0,
                started_at: 0,
                period: 0,
                active_until: 0,
            });
            Ok(())
        }

        #[ink(message)]
        pub fn extend(&mut self, period: u32) -> Result<(), String> {
            let caller = self.env().caller();
            let stake_info = self.stakes.get(&caller).ok_or_else(|| "Stake info not found".to_string())?;
            assert!(stake_info.amount > 0, "stake required");
            assert!(stake_info.active_until < self.env().block_timestamp(), "still active");
            self._collect_rewards(caller, true)?;
            self._stake(caller, period, 0)?;
            Ok(())
        }

        #[ink(message)]
        pub fn claim(&mut self) -> Result<(), String> {
            let caller = self.env().caller();
            if self.stakes.get(&caller).is_none() {
                return Err("no stake".to_string());
            }
            self._collect_rewards(caller, false)?;
            Ok(())
        }

        #[ink(message, payable)]
        pub fn update_rewards_pool(&mut self) -> Result<(), String> {
            let value = self.env().transferred_value();
            assert!(value > 0, "amount should be > 0");
            self.rewards_balance += value;
            self.env().emit_event(RewardPoolUpdated { amount: value });
            Ok(())
        }

        fn _validate_period(&self, period: u32) -> Result<(), String> {
            if !self.available_periods.contains(&period) {
                return Err("period not exist".to_string());
            }
            Ok(())
        }

        fn reward_amount(&self, account: AccountId) -> Result<(u32, u128), String> {
            let stake_info = self.stakes.get(&account).ok_or_else(|| "Stake info not found".to_string())?;
            let time = if self.env().block_timestamp() > stake_info.active_until {
                stake_info.active_until
            } else {
                self.env().block_timestamp()
            };
            let periods_passed = (time - self.last_reward_claims.get(&account).unwrap_or(0)) / 86400;
            let reward = (stake_info.amount * self.reward_rate * periods_passed as u128 * 100) / 36000;
            Ok((periods_passed as u32, reward))
        }

        fn _next_reward_date(&self, account: AccountId) -> Result<u64, String> {
            if let Some(last_claim) = self.last_reward_claims.get(&account) {
                if let Some(stake_info) = self.stakes.get(&account) {
                    if self.env().block_timestamp() > stake_info.active_until {
                        Ok(stake_info.active_until)
                    } else {
                        let passed_periods = (self.env().block_timestamp() - stake_info.started_at) / 86400;
                        Ok(((passed_periods + 1) * 86400) + stake_info.started_at)
                    }
                } else {
                    Err("Stake info not found".to_string())
                }
            } else {
                Err("Last reward claim not found".to_string())
            }
        }

        fn _stake(&mut self, account: AccountId, periods: u32, amount: u128) -> Result<(), String> {
            let new_amount = self.stakes.get(&account).map_or(amount, |info| info.amount + amount);
            self._validate_period(periods)?;
            let until = if amount == 0 {
                self.env().block_timestamp() + (periods as u64 * 86400 * 30)
            } else {
                self.stakes.get(&account).map_or(0, |stake_info| stake_info.active_until)
            };

            self._set_stake_info(account, new_amount, periods, self.env().block_timestamp(), until)?;
            self.total_staked += amount;
            self.env().emit_event(Stake {
                account,
                staked_at: self.env().block_timestamp(),
                period: periods,
                sum: amount,
                total_staked: new_amount,
            });
            Ok(())
        }

        fn _withdraw(&mut self, account: AccountId, amount: u128) -> Result<(), String> {
            self._set_stake_info(account, 0, 0, 0, 0)?;
            self.env().transfer(account, amount).map_err(|_| "Transfer failed".to_string())?;
            self.env().emit_event(Withdraw {
                account,
                sum: amount,
                is_early: false,
            });
            Ok(())
        }

        fn _collect_rewards(&mut self, account: AccountId, not_direct: bool) -> Result<(), String> {
            if let Some(stake_info) = self.stakes.get(&account) {
                if stake_info.amount > 0 {
                    let (periods, reward) = self.reward_amount(account)?;
                    if not_direct && periods == 0 {
                        return Ok(());
                    }
                    assert!(self.rewards_balance >= reward, "not enough rewards");
                    assert!(periods > 0, "too early");
                    let last_claim = self.last_reward_claims.get(&account).unwrap_or(0);
                    self.last_reward_claims.insert(account, &(last_claim + ((86400 * periods) as u64)));
                    self.rewards_balance -= reward;
                    let reward_amount_in_reward_token = reward * self.reward_conversion_rate;
                    self.env().emit_event(Claim {
                        account,
                        periods,
                        amount: reward,
                    });
                    // Transfer the reward tokens to the account
                    // Assuming the reward token follows the PSP22 standard
                    // ink::env::call::build_call::<ink::env::DefaultEnvironment>()
                    //     .call(self.reward_token)
                    //     .gas_limit(5000)
                    //     .transferred_value(0)
                    //     .exec_input(
                    //         ink::env::call::ExecutionInput::new(ink::env::call::Selector::new([0x23, 0xb8, 0x72, 0xdd])) // transfer selector
                    //             .push_arg(account)
                    //             .push_arg(reward_amount_in_reward_token),
                    //     )
                    //     .returns::<()>()
                    //     .invoke();
                    // Transfer the reward tokens to the account using the PSP22 interface
                    PSP22Ref::transfer(&self.reward_token, account, reward_amount_in_reward_token, Vec::new()).map_err(|_| "Transfer failed".to_string())?;
                }
            }
            Ok(())
        }

        fn _set_stake_info(&mut self, account: AccountId, amount: u128, periods: u32, started_at: u64, until: u64) -> Result<(), String> {
            self.stakes.insert(account, &StakeInfo {
                amount,
                started_at,
                period: periods,
                active_until: until,
            });
            Ok(())
        }
    }

    #[ink(event)]
    pub struct Stake {
        #[ink(topic)]
        account: AccountId,
        staked_at: u64,
        period: u32,
        sum: u128,
        total_staked: u128,
    }

    #[ink(event)]
    pub struct Withdraw {
        #[ink(topic)]
        account: AccountId,
        sum: u128,
        is_early: bool,
    }

    #[ink(event)]
    pub struct RewardPoolUpdated {
        amount: u128,
    }

    #[ink(event)]
    pub struct Claim {
        #[ink(topic)]
        account: AccountId,
        periods: u32,
        amount: u128,
    }
}

#[cfg(test)]
mod tests {
    use crate::staking::Staking;
    use ink::env::{test, DefaultEnvironment};
    use log::{info, debug};
    use token::usdt_psp22::USDT;
    use openbrush::contracts::traits::psp22::PSP22;
    // Initialize the logger once for all tests in this module
    fn init() {
        let _ = env_logger::builder().is_test(true).try_init();
    }

    fn get_account_id_from_contract(contract_address: &dyn PSP22) -> ink::primitives::AccountId {
        ink::env::account_id::<DefaultEnvironment>()
    }

    fn create_sp22_token() -> (USDT, ink::primitives::AccountId) {
        let reward_token = USDT::new(1_000_000);
        let token_address = get_account_id_from_contract(&reward_token);

        return (reward_token, token_address);
        // Deploy the staking contract with the PSP22 token as the reward token
    }

    #[ink::test]
    fn test_new() {
        init();
        let accounts = test::default_accounts::<DefaultEnvironment>();
        test::set_caller::<DefaultEnvironment>(accounts.alice);
        // Deploy the PSP22 token contract
        let(reward_token, reward_token_account_id) = create_sp22_token();
        // Deploy the staking contract with the PSP22 token as the reward token
        let staking = Staking::new(reward_token_account_id, 1);
        
        let alice_balance = reward_token.balance_of(accounts.alice);

        info!("alice balance: {}", alice_balance);

        // assert_eq!(staking.reward_token, reward_token_account_id);
        assert_eq!(staking.reward_conversion_rate, 1);
        assert_eq!(staking.available_periods, vec![6, 12]);
    }


    #[ink::test]
    fn test_update_rewards_pool() {
        let accounts = test::default_accounts::<DefaultEnvironment>();
        let mut staking = Staking::new(accounts.alice, 1);

        test::set_caller::<DefaultEnvironment>(accounts.alice);
        test::set_value_transferred::<DefaultEnvironment>(100);
        staking.update_rewards_pool().unwrap();

        assert_eq!(staking.rewards_balance, 100);
    }

    #[ink::test]
    fn test_stake() {
        init();
        let accounts = test::default_accounts::<DefaultEnvironment>();
        let mut staking = Staking::new(accounts.alice, 1);

        test::set_caller::<DefaultEnvironment>(accounts.bob);
        test::set_value_transferred::<DefaultEnvironment>(10);
        staking.stake(6).unwrap();

        let stake_info = staking.stakes.get(&accounts.bob).unwrap();
        

        info!("Testing staking with amount: {}, periods: {}", stake_info.amount, stake_info.period);

        assert_eq!(stake_info.amount, 10);
        assert_eq!(stake_info.period, 6);
    }

    #[ink::test]
    fn test_emergency_withdraw() {
        let accounts = test::default_accounts::<DefaultEnvironment>();
        let mut staking = Staking::new(accounts.alice, 1);

        // Set up initial stake
        test::set_caller::<DefaultEnvironment>(accounts.bob);
        test::set_value_transferred::<DefaultEnvironment>(10);
        staking.stake(6).unwrap();

        // Ensure some time passes
        test::advance_block::<DefaultEnvironment>();

        // Perform emergency withdraw
        test::set_caller::<DefaultEnvironment>(accounts.bob);
        staking.emergency_withdraw().unwrap();

        let stake_info = staking.stakes.get(&accounts.bob).unwrap();
        assert_eq!(stake_info.amount, 0);
    }


    #[ink::test]
    fn test_extend() {
        let accounts = test::default_accounts::<DefaultEnvironment>();
        let mut staking = Staking::new(accounts.alice, 1);

        // Set up initial stake
        test::set_caller::<DefaultEnvironment>(accounts.bob);
        test::set_value_transferred::<DefaultEnvironment>(10);
        staking.stake(6).unwrap();

        // Ensure some time passes
        test::advance_block::<DefaultEnvironment>();

        // Perform extend
        test::set_caller::<DefaultEnvironment>(accounts.bob);
        staking.extend(6).unwrap();

        let stake_info = staking.stakes.get(&accounts.bob).unwrap();
        assert_eq!(stake_info.period, 6);
    }

    #[ink::test]
    fn test_withdraw() {
        init();
        let accounts = test::default_accounts::<DefaultEnvironment>();
        let mut staking = Staking::new(accounts.alice, 1);
        let amount = 10;
        // Set up initial stake
        test::set_caller::<DefaultEnvironment>(accounts.bob);
        test::set_value_transferred::<DefaultEnvironment>(amount);
        staking.stake(6).unwrap();

        // Ensure some time passes
        test::advance_block::<DefaultEnvironment>();
        // Query the native balance of Bob's account
        let bob_native_balance_before = test::get_account_balance::<DefaultEnvironment>(accounts.bob).unwrap();

        // Perform withdraw
        test::set_caller::<DefaultEnvironment>(accounts.bob);
        staking.withdraw().unwrap();

        let bob_native_balance_after = test::get_account_balance::<DefaultEnvironment>(accounts.bob).unwrap();

        info!("BOB BEFORE BALANCE: {}, BOB AFTER BALANCE: {}", bob_native_balance_before, bob_native_balance_after);

        let stake_info = staking.stakes.get(&accounts.bob).unwrap();
        assert_eq!(stake_info.amount, 0);
        assert_eq!(bob_native_balance_before, bob_native_balance_after - amount)
    }

    #[ink::test]
    fn test_claim() {
        init();
        let accounts = test::default_accounts::<DefaultEnvironment>();
        let mut staking = Staking::new(accounts.alice, 1);

        // Set up initial stake
        test::set_caller::<DefaultEnvironment>(accounts.bob);
        test::set_value_transferred::<DefaultEnvironment>(10);
        staking.stake(6).unwrap();

        // Ensure some time passes
        test::advance_block::<DefaultEnvironment>();

        // Perform claim
        test::set_caller::<DefaultEnvironment>(accounts.bob);

        let stake_info = staking.stakes.get(&accounts.bob).unwrap();
        info!("ACTIVE UNTIL {}", stake_info.active_until);

        // staking.claim().unwrap();

        // let reward = staking.available_rewards(accounts.bob).unwrap();
        // assert!(reward > 0);
    }

}