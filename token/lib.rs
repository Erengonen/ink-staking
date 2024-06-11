#![cfg_attr(not(feature = "std"), no_std, no_main)]

#[openbrush::implementation(PSP22)]
#[openbrush::contract]
pub mod usdt_psp22 {
    use openbrush::traits::Storage;
    use openbrush::contracts::traits::psp22::PSP22;

    #[ink(storage)]
    #[derive(Default, Storage)]
    pub struct USDT {
    	#[storage_field]
		psp22: psp22::Data,
    }
    
    impl USDT {
        #[ink(constructor)]
        pub fn new(initial_supply: Balance) -> Self {
            let mut _instance = Self::default();
			<dyn psp22::Internal>::_mint_to(&mut _instance, Self::env().caller(), initial_supply).expect("Should mint"); 
			_instance
        }
    }

}