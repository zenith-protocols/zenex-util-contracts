extern crate std;

use soroban_sdk::{
    auth::{Context, ContractContext},
    symbol_short,
    testutils::Address as _,
    vec, Address, Env, IntoVal, String, Symbol, Val, Vec,
};
use stellar_accounts::smart_account::{ContextRule, ContextRuleType, Signer};

use crate::contract::{SessionConfig, SessionPolicyContract, SessionPolicyContractClient};

// ==========================================
// Helpers
// ==========================================

fn setup(e: &Env) -> (SessionPolicyContractClient<'_>, Address, Address, Address) {
    let policy_addr = e.register(SessionPolicyContract, ());
    let client = SessionPolicyContractClient::new(e, &policy_addr);

    let smart_account = Address::generate(e);
    let trading_contract = Address::generate(e);
    let token_contract = Address::generate(e);

    (client, smart_account, trading_contract, token_contract)
}

fn make_config(e: &Env, trading: &Address, token: &Address) -> SessionConfig {
    SessionConfig {
        allowed_contracts: vec![e, trading.clone(), token.clone()],
        allowed_transfer_to: trading.clone(),
    }
}

fn make_context_rule(e: &Env, id: u32) -> ContextRule {
    ContextRule {
        id,
        context_type: ContextRuleType::Default,
        name: String::from_str(e, "session"),
        signers: Vec::new(e),
        signer_ids: Vec::new(e),
        policies: Vec::new(e),
        policy_ids: Vec::new(e),
        valid_until: None,
    }
}

fn make_contract_context(contract: &Address, fn_name: Symbol, args: Vec<Val>) -> Context {
    Context::Contract(ContractContext {
        contract: contract.clone(),
        fn_name,
        args,
    })
}

// ==========================================
// Install / Uninstall Tests
// ==========================================

#[test]
fn test_install_stores_config() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, smart_account, trading, token) = setup(&e);
    let config = make_config(&e, &trading, &token);
    let rule = make_context_rule(&e, 0);

    // Install should succeed
    client.install(&config, &rule, &smart_account);
}

#[test]
#[should_panic(expected = "Error(Contract, #4003)")]
fn test_install_twice_fails() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, smart_account, trading, token) = setup(&e);
    let config = make_config(&e, &trading, &token);
    let rule = make_context_rule(&e, 0);

    client.install(&config, &rule, &smart_account);
    client.install(&config, &rule, &smart_account); // should panic
}

#[test]
#[should_panic(expected = "Error(Contract, #4005)")]
fn test_install_empty_whitelist_fails() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, smart_account, trading, _token) = setup(&e);
    let config = SessionConfig {
        allowed_contracts: Vec::new(&e),
        allowed_transfer_to: trading,
    };
    let rule = make_context_rule(&e, 0);

    client.install(&config, &rule, &smart_account);
}

#[test]
fn test_uninstall_removes_config() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, smart_account, trading, token) = setup(&e);
    let config = make_config(&e, &trading, &token);
    let rule = make_context_rule(&e, 0);

    client.install(&config, &rule, &smart_account);
    client.uninstall(&rule, &smart_account);
}

#[test]
#[should_panic(expected = "Error(Contract, #4004)")]
fn test_uninstall_not_installed_fails() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, smart_account, _trading, _token) = setup(&e);
    let rule = make_context_rule(&e, 0);

    client.uninstall(&rule, &smart_account); // not installed
}

// ==========================================
// Enforce — Contract Whitelist Tests
// ==========================================

#[test]
fn test_enforce_allows_whitelisted_contract() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, smart_account, trading, token) = setup(&e);
    let config = make_config(&e, &trading, &token);
    let rule = make_context_rule(&e, 0);
    client.install(&config, &rule, &smart_account);

    // Calling open_position on trading contract — should pass
    let context = make_contract_context(&trading, Symbol::new(&e, "open_position"), vec![&e]);
    let signers: Vec<Signer> = Vec::new(&e);

    client.enforce(&context, &signers, &rule, &smart_account);
}

#[test]
#[should_panic(expected = "Error(Contract, #4001)")]
fn test_enforce_blocks_non_whitelisted_contract() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, smart_account, trading, token) = setup(&e);
    let config = make_config(&e, &trading, &token);
    let rule = make_context_rule(&e, 0);
    client.install(&config, &rule, &smart_account);

    // Calling some random contract — should fail
    let random_contract = Address::generate(&e);
    let context = make_contract_context(&random_contract, Symbol::new(&e, "steal_funds"), vec![&e]);
    let signers: Vec<Signer> = Vec::new(&e);

    client.enforce(&context, &signers, &rule, &smart_account);
}

// ==========================================
// Enforce — Transfer Destination Tests
// ==========================================

#[test]
fn test_enforce_allows_transfer_to_trading() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, smart_account, trading, token) = setup(&e);
    let config = make_config(&e, &trading, &token);
    let rule = make_context_rule(&e, 0);
    client.install(&config, &rule, &smart_account);

    // token.transfer(from, to=trading, amount) — should pass
    let from: Val = smart_account.clone().into_val(&e);
    let to: Val = trading.clone().into_val(&e);
    let amount: Val = 1000i128.into_val(&e);
    let context = make_contract_context(
        &token,
        symbol_short!("transfer"),
        vec![&e, from, to, amount],
    );
    let signers: Vec<Signer> = Vec::new(&e);

    client.enforce(&context, &signers, &rule, &smart_account);
}

#[test]
#[should_panic(expected = "Error(Contract, #4002)")]
fn test_enforce_blocks_transfer_to_attacker() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, smart_account, trading, token) = setup(&e);
    let config = make_config(&e, &trading, &token);
    let rule = make_context_rule(&e, 0);
    client.install(&config, &rule, &smart_account);

    // token.transfer(from, to=attacker, amount) — should fail
    let attacker = Address::generate(&e);
    let from: Val = smart_account.clone().into_val(&e);
    let to: Val = attacker.into_val(&e);
    let amount: Val = 1000i128.into_val(&e);
    let context = make_contract_context(
        &token,
        symbol_short!("transfer"),
        vec![&e, from, to, amount],
    );
    let signers: Vec<Signer> = Vec::new(&e);

    client.enforce(&context, &signers, &rule, &smart_account);
}

#[test]
fn test_enforce_allows_non_transfer_fn_on_token() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, smart_account, trading, token) = setup(&e);
    let config = make_config(&e, &trading, &token);
    let rule = make_context_rule(&e, 0);
    client.install(&config, &rule, &smart_account);

    // token.balance(addr) — not a transfer, should pass even though token is whitelisted
    let context = make_contract_context(
        &token,
        Symbol::new(&e, "balance"),
        vec![&e, smart_account.clone().into_val(&e)],
    );
    let signers: Vec<Signer> = Vec::new(&e);

    client.enforce(&context, &signers, &rule, &smart_account);
}

// ==========================================
// Enforce — Multi-rule Isolation Test
// ==========================================

#[test]
fn test_separate_rules_have_separate_configs() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, smart_account, trading, token) = setup(&e);

    // Install different configs on different rule IDs
    let config_0 = make_config(&e, &trading, &token);
    let rule_0 = make_context_rule(&e, 0);
    client.install(&config_0, &rule_0, &smart_account);

    let other_contract = Address::generate(&e);
    let config_1 = SessionConfig {
        allowed_contracts: vec![&e, other_contract.clone()],
        allowed_transfer_to: other_contract.clone(),
    };
    let rule_1 = make_context_rule(&e, 1);
    client.install(&config_1, &rule_1, &smart_account);

    // Rule 0 allows trading, blocks other_contract
    let ctx_trading = make_contract_context(&trading, Symbol::new(&e, "open_position"), vec![&e]);
    let signers: Vec<Signer> = Vec::new(&e);
    client.enforce(&ctx_trading, &signers, &rule_0, &smart_account);

    // Rule 1 allows other_contract
    let ctx_other = make_contract_context(&other_contract, Symbol::new(&e, "do_thing"), vec![&e]);
    client.enforce(&ctx_other, &signers, &rule_1, &smart_account);
}
