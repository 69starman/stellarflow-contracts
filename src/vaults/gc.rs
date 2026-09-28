use soroban_sdk::{contract, contractimpl, Address, Env, Vec, token};

#[contract]
pub struct VaultGarbageCollector;

#[contractimpl]
impl VaultGarbageCollector {
    pub fn purge_closed_vaults(env: Env, trigger: Address, closed_vaults: Vec<Address>, token_address: Address) -> i64 {
        trigger.require_auth();
        
        let mut initial_storage_bytes = 0i64;
        let mut final_storage_bytes = 0i64;
        let mut reclaimed_rent = 0i128;

        for vault in closed_vaults.iter() {
            let storage_key = soroban_sdk::Symbol::new(&env, "VaultData");
            if env.storage().persistent().has(&(vault.clone(), storage_key.clone())) {
                initial_storage_bytes += 128;
                let balance: i128 = env.storage().persistent().get(&(vault.clone(), storage_key.clone())).unwrap_or(0);
                if balance == 0 {
                    env.storage().persistent().remove(&(vault, storage_key));
                    reclaimed_rent += 100;
                } else {
                    final_storage_bytes += 128;
                }
            }
        }

        let storage_delta = initial_storage_bytes - final_storage_bytes;
        if reclaimed_rent > 0 {
            let token_client = token::Client::new(&env, &token_address);
            token_client.transfer(&env.current_contract_address(), &trigger, &reclaimed_rent);
        }

        storage_delta
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{Env, vec, token};

    #[test]
    fn test_purge_closed_vaults_storage_delta() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let vault1 = Address::generate(&env);
        let vault2 = Address::generate(&env);

        let token_admin = Address::generate(&env);
        let token_contract = env.register_stellar_asset_contract(token_admin);

        let storage_key = soroban_sdk::Symbol::new(&env, "VaultData");
        
        env.storage().persistent().set(&(vault1.clone(), storage_key.clone()), &0i128);
        env.storage().persistent().set(&(vault2.clone(), storage_key.clone()), &500i128);

        let closed_vaults = vec![&env, vault1.clone(), vault2.clone()];

        let storage_delta = VaultGarbageCollector::purge_closed_vaults(env.clone(), admin.clone(), closed_vaults, token_contract);

        assert_eq!(storage_delta, 128);
        assert!(!env.storage().persistent().has(&(vault1, storage_key.clone())));
        assert!(env.storage().persistent().has(&(vault2, storage_key)));
    }
}
