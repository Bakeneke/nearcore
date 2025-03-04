//! Helper functions to compute the costs of certain actions assuming they succeed and the only
//! actions in the transaction batch.
use near_primitives::types::Balance;
use near_runtime_fees::RuntimeFeesConfig;

// We currently don't have mechanism to set the gas cost. So it is equal to 1.
const GAS_COST: u64 = 1;

pub fn create_account_cost() -> Balance {
    let cfg = RuntimeFeesConfig::default();
    let gas = cfg.action_receipt_creation_config.exec_fee()
        + cfg.action_receipt_creation_config.send_fee(false)
        + cfg.action_creation_config.create_account_cost.exec_fee()
        + cfg.action_creation_config.create_account_cost.send_fee(false);
    (gas * GAS_COST) as Balance
}

pub fn create_account_transfer_full_key_cost() -> Balance {
    let cfg = RuntimeFeesConfig::default();
    let gas = cfg.action_receipt_creation_config.exec_fee()
        + cfg.action_receipt_creation_config.send_fee(false)
        + cfg.action_creation_config.create_account_cost.exec_fee()
        + cfg.action_creation_config.create_account_cost.send_fee(false)
        + cfg.action_creation_config.transfer_cost.exec_fee()
        + cfg.action_creation_config.transfer_cost.send_fee(false)
        + cfg.action_creation_config.add_key_cost.full_access_cost.exec_fee()
        + cfg.action_creation_config.add_key_cost.full_access_cost.send_fee(false);
    (gas * GAS_COST) as Balance
}

pub fn create_account_transfer_full_key_cost_fail_on_create_account() -> Balance {
    let cfg = RuntimeFeesConfig::default();
    let gas = cfg.action_receipt_creation_config.exec_fee()
        + cfg.action_receipt_creation_config.send_fee(false)
        + cfg.action_creation_config.create_account_cost.exec_fee()
        + cfg.action_creation_config.create_account_cost.send_fee(false)
        + cfg.action_creation_config.transfer_cost.send_fee(false)
        + cfg.action_creation_config.add_key_cost.full_access_cost.send_fee(false);
    (gas * GAS_COST) as Balance
}

pub fn deploy_contract_cost(num_bytes: u64) -> Balance {
    let cfg = RuntimeFeesConfig::default();
    let gas = cfg.action_receipt_creation_config.exec_fee()
        + cfg.action_receipt_creation_config.send_fee(false)
        + cfg.action_creation_config.deploy_contract_cost.exec_fee()
        + cfg.action_creation_config.deploy_contract_cost.send_fee(false)
        + num_bytes
            * (cfg.action_creation_config.deploy_contract_cost_per_byte.exec_fee()
                + cfg.action_creation_config.deploy_contract_cost_per_byte.send_fee(false));
    (gas * GAS_COST) as Balance
}

pub fn function_call_cost(num_bytes: u64) -> Balance {
    let cfg = RuntimeFeesConfig::default();
    let gas = cfg.action_receipt_creation_config.exec_fee()
        + cfg.action_receipt_creation_config.send_fee(false)
        + cfg.action_creation_config.function_call_cost.exec_fee()
        + cfg.action_creation_config.function_call_cost.send_fee(false)
        + num_bytes
            * (cfg.action_creation_config.function_call_cost_per_byte.exec_fee()
                + cfg.action_creation_config.function_call_cost_per_byte.send_fee(false));
    (gas * GAS_COST) as Balance
}

pub fn transfer_cost() -> Balance {
    let cfg = RuntimeFeesConfig::default();
    let gas = cfg.action_receipt_creation_config.exec_fee()
        + cfg.action_receipt_creation_config.send_fee(false)
        + cfg.action_creation_config.transfer_cost.exec_fee()
        + cfg.action_creation_config.transfer_cost.send_fee(false);
    (gas * GAS_COST) as Balance
}

pub fn stake_cost() -> Balance {
    let cfg = RuntimeFeesConfig::default();
    let gas = cfg.action_receipt_creation_config.exec_fee()
        + cfg.action_receipt_creation_config.send_fee(false)
        + cfg.action_creation_config.stake_cost.exec_fee()
        + cfg.action_creation_config.stake_cost.send_fee(false);
    (gas * GAS_COST) as Balance
}

pub fn add_key_cost(num_bytes: u64) -> Balance {
    let cfg = RuntimeFeesConfig::default();
    let gas = cfg.action_receipt_creation_config.exec_fee()
        + cfg.action_receipt_creation_config.send_fee(false)
        + cfg.action_creation_config.add_key_cost.function_call_cost.exec_fee()
        + cfg.action_creation_config.add_key_cost.function_call_cost.send_fee(false)
        + num_bytes
            * (cfg.action_creation_config.add_key_cost.function_call_cost_per_byte.exec_fee()
                + cfg
                    .action_creation_config
                    .add_key_cost
                    .function_call_cost_per_byte
                    .send_fee(false));
    (gas * GAS_COST) as Balance
}

pub fn add_key_full_cost() -> Balance {
    let cfg = RuntimeFeesConfig::default();
    let gas = cfg.action_receipt_creation_config.exec_fee()
        + cfg.action_receipt_creation_config.send_fee(false)
        + cfg.action_creation_config.add_key_cost.full_access_cost.exec_fee()
        + cfg.action_creation_config.add_key_cost.full_access_cost.send_fee(false);
    (gas * GAS_COST) as Balance
}

pub fn delete_key_cost() -> Balance {
    let cfg = RuntimeFeesConfig::default();
    let gas = cfg.action_receipt_creation_config.exec_fee()
        + cfg.action_receipt_creation_config.send_fee(false)
        + cfg.action_creation_config.delete_key_cost.exec_fee()
        + cfg.action_creation_config.delete_key_cost.send_fee(false);
    (gas * GAS_COST) as Balance
}

pub fn delete_account_cost() -> Balance {
    let cfg = RuntimeFeesConfig::default();
    let gas = cfg.action_receipt_creation_config.exec_fee()
        + cfg.action_receipt_creation_config.send_fee(false)
        + cfg.action_creation_config.delete_account_cost.exec_fee()
        + cfg.action_creation_config.delete_account_cost.send_fee(false);
    (gas * GAS_COST) as Balance
}
