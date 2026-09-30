extern crate std;

use soroban_sdk::{
    auth::{Context, ContractContext, ContractExecutable, CreateContractHostFnContext},
    testutils::{Address as _, MuxedAddress as _},
    vec, Address, BytesN, Env, IntoVal, MuxedAddress, String, Symbol, Val, Vec,
};
use stellar_accounts::smart_account::{ContextRule, ContextRuleType, Signer};

use crate::contract::{Session, SessionConfig, SessionPolicyContract, SessionPolicyContractClient};

const SCALAR_7: i128 = 10_000_000; // one token unit at 7 decimals
const LIMIT: i128 = 100 * SCALAR_7; // a $100 budget
const BALANCE: i128 = 1_000 * SCALAR_7; // the wallet's $1,000

// ==========================================
// Helpers
// ==========================================

struct Setup<'a> {
    e: Env,
    client: SessionPolicyContractClient<'a>,
    smart_account: Address,
    market: Address,
    router: Address,
    token: Address,
}

impl Setup<'_> {
    fn new() -> Self {
        let e = Env::default();
        e.mock_all_auths();
        let client = SessionPolicyContractClient::new(&e, &e.register(SessionPolicyContract, ()));
        Setup {
            smart_account: Address::generate(&e),
            market: Address::generate(&e),
            router: Address::generate(&e),
            token: Address::generate(&e),
            client,
            e,
        }
    }

    fn config(&self) -> SessionConfig {
        SessionConfig {
            allowed_contracts: vec![&self.e, self.market.clone(), self.router.clone()],
            token: self.token.clone(),
            spend_limit: LIMIT,
        }
    }

    fn rule(&self, id: u32) -> ContextRule {
        ContextRule {
            id,
            context_type: ContextRuleType::Default,
            name: String::from_str(&self.e, "Trading Session"),
            signers: Vec::new(&self.e),
            signer_ids: Vec::new(&self.e),
            policies: Vec::new(&self.e),
            policy_ids: Vec::new(&self.e),
            valid_until: None,
        }
    }

    /// Installs the default config under rule `id` and returns the rule.
    fn install(&self, id: u32) -> ContextRule {
        let rule = self.rule(id);
        self.client
            .install(&self.config(), &rule, &self.smart_account);
        rule
    }

    fn call(&self, contract: &Address, fn_name: &str, args: Vec<Val>) -> Context {
        Context::Contract(ContractContext {
            contract: contract.clone(),
            fn_name: Symbol::new(&self.e, fn_name),
            args,
        })
    }

    /// `token.transfer(wallet, to, amount)`.
    fn transfer(&self, to: Val, amount: i128) -> Context {
        let e = &self.e;
        let args = vec![e, self.smart_account.into_val(e), to, amount.into_val(e)];
        self.call(&self.token, "transfer", args)
    }

    /// `token.approve(wallet, spender, amount, expiration_ledger)`.
    fn approve(&self, spender: &Address, amount: i128) -> Context {
        let e = &self.e;
        let args = vec![
            e,
            self.smart_account.into_val(e),
            spender.into_val(e),
            amount.into_val(e),
            1_000u32.into_val(e),
        ];
        self.call(&self.token, "approve", args)
    }

    fn enforce(&self, context: &Context, rule: &ContextRule) {
        let signers: Vec<Signer> = Vec::new(&self.e);
        self.client
            .enforce(context, &signers, rule, &self.smart_account);
    }

    /// The contract error code `enforce` fails with.
    fn enforce_error(&self, context: &Context, rule: &ContextRule) -> u32 {
        let signers: Vec<Signer> = Vec::new(&self.e);
        match self
            .client
            .try_enforce(context, &signers, rule, &self.smart_account)
        {
            Err(Ok(error)) => error.get_code(),
            other => panic!("expected a contract error, got {other:?}"),
        }
    }

    fn spent(&self, rule: &ContextRule) -> i128 {
        self.client.get_session(&self.smart_account, &rule.id).spent
    }
}

// ==========================================
// Install / Uninstall Tests
// ==========================================

#[test]
fn test_install_stores_session() {
    let s = Setup::new();
    let rule = s.install(0);

    let session = s.client.get_session(&s.smart_account, &rule.id);
    assert_eq!(
        session,
        Session {
            config: s.config(),
            spent: 0
        }
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #4003)")]
fn test_install_twice_fails() {
    let s = Setup::new();
    s.install(0);
    s.install(0);
}

#[test]
#[should_panic(expected = "Error(Contract, #4005)")]
fn test_install_empty_allowed_contracts_fails() {
    let s = Setup::new();
    let config = SessionConfig {
        allowed_contracts: Vec::new(&s.e),
        ..s.config()
    };
    s.client.install(&config, &s.rule(0), &s.smart_account);
}

#[test]
#[should_panic(expected = "Error(Contract, #4008)")]
fn test_install_zero_spend_limit_fails() {
    let s = Setup::new();
    let config = SessionConfig {
        spend_limit: 0,
        ..s.config()
    };
    s.client.install(&config, &s.rule(0), &s.smart_account);
}

#[test]
#[should_panic(expected = "Error(Contract, #4008)")]
fn test_install_negative_spend_limit_fails() {
    let s = Setup::new();
    let config = SessionConfig {
        spend_limit: -1,
        ..s.config()
    };
    s.client.install(&config, &s.rule(0), &s.smart_account);
}

#[test]
#[should_panic(expected = "Error(Contract, #4001)")]
fn test_install_wallet_as_allowed_contract_fails() {
    let s = Setup::new();
    let config = SessionConfig {
        allowed_contracts: vec![&s.e, s.market.clone(), s.smart_account.clone()],
        ..s.config()
    };
    s.client.install(&config, &s.rule(0), &s.smart_account);
}

#[test]
#[should_panic(expected = "Error(Contract, #4001)")]
fn test_install_wallet_as_token_fails() {
    let s = Setup::new();
    let config = SessionConfig {
        token: s.smart_account.clone(),
        ..s.config()
    };
    s.client.install(&config, &s.rule(0), &s.smart_account);
}

#[test]
fn test_uninstall_removes_session() {
    let s = Setup::new();
    let rule = s.install(0);

    s.client.uninstall(&rule, &s.smart_account);

    let result = s.client.try_get_session(&s.smart_account, &rule.id);
    assert_eq!(result.unwrap_err().unwrap().get_code(), 4004);
}

#[test]
#[should_panic(expected = "Error(Contract, #4004)")]
fn test_uninstall_not_installed_fails() {
    let s = Setup::new();
    s.client.uninstall(&s.rule(0), &s.smart_account);
}

#[test]
#[should_panic(expected = "Error(Contract, #4004)")]
fn test_enforce_not_installed_fails() {
    let s = Setup::new();
    s.enforce(&s.call(&s.market, "create_order", vec![&s.e]), &s.rule(0));
}

// ==========================================
// Enforce — Contract Allowlist Tests
// ==========================================

#[test]
fn test_enforce_allows_market_call() {
    let s = Setup::new();
    let rule = s.install(0);

    s.enforce(&s.call(&s.market, "create_order", vec![&s.e]), &rule);
    assert_eq!(s.spent(&rule), 0);
}

#[test]
fn test_enforce_allows_router_call() {
    let s = Setup::new();
    let rule = s.install(0);

    s.enforce(&s.call(&s.router, "multicall_with_fee", vec![&s.e]), &rule);
    assert_eq!(s.spent(&rule), 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #4001)")]
fn test_enforce_blocks_non_allowed_contract() {
    let s = Setup::new();
    let rule = s.install(0);

    let other = Address::generate(&s.e);
    s.enforce(&s.call(&other, "steal_funds", vec![&s.e]), &rule);
}

#[test]
#[should_panic(expected = "Error(Contract, #4001)")]
fn test_enforce_blocks_wallet_call() {
    let s = Setup::new();
    let rule = s.install(0);

    // The key cannot add a signer, change a rule, or upgrade the wallet.
    s.enforce(
        &s.call(&s.smart_account, "add_context_rule", vec![&s.e]),
        &rule,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #4001)")]
fn test_enforce_blocks_create_contract() {
    let s = Setup::new();
    let rule = s.install(0);

    let context = Context::CreateContractHostFn(CreateContractHostFnContext {
        executable: ContractExecutable::Wasm(BytesN::from_array(&s.e, &[1; 32])),
        salt: BytesN::from_array(&s.e, &[2; 32]),
    });
    s.enforce(&context, &rule);
}

// ==========================================
// Enforce — Token Budget Tests
// ==========================================

#[test]
fn test_enforce_allows_transfer_within_budget() {
    let s = Setup::new();
    let rule = s.install(0);

    s.enforce(&s.transfer(s.market.into_val(&s.e), 60 * SCALAR_7), &rule);
    assert_eq!(s.spent(&rule), 60 * SCALAR_7);
}

#[test]
fn test_enforce_allows_approve_within_budget() {
    let s = Setup::new();
    let rule = s.install(0);

    s.enforce(&s.approve(&s.router, SCALAR_7), &rule);
    assert_eq!(s.spent(&rule), SCALAR_7);
}

#[test]
fn test_enforce_zero_approve_spends_nothing() {
    let s = Setup::new();
    let rule = s.install(0);

    // The router wipes its leftover allowance with `approve(.., 0, ..)`.
    s.enforce(&s.approve(&s.router, 0), &rule);
    assert_eq!(s.spent(&rule), 0);
}

#[test]
fn test_enforce_allows_spend_up_to_exact_limit() {
    let s = Setup::new();
    let rule = s.install(0);

    s.enforce(&s.transfer(s.market.into_val(&s.e), 60 * SCALAR_7), &rule);
    s.enforce(&s.approve(&s.router, 40 * SCALAR_7), &rule);
    assert_eq!(s.spent(&rule), LIMIT);
}

#[test]
#[should_panic(expected = "Error(Contract, #4007)")]
fn test_enforce_blocks_transfer_over_budget() {
    let s = Setup::new();
    let rule = s.install(0);

    s.enforce(&s.transfer(s.market.into_val(&s.e), LIMIT + 1), &rule);
}

#[test]
#[should_panic(expected = "Error(Contract, #4007)")]
fn test_enforce_blocks_approve_over_budget() {
    let s = Setup::new();
    let rule = s.install(0);

    s.enforce(&s.approve(&s.router, LIMIT + 1), &rule);
}

#[test]
fn test_enforce_blocks_cumulative_spend_over_budget() {
    let s = Setup::new();
    let rule = s.install(0);

    s.enforce(&s.transfer(s.market.into_val(&s.e), 60 * SCALAR_7), &rule);
    assert_eq!(
        s.enforce_error(&s.approve(&s.router, 40 * SCALAR_7 + 1), &rule),
        4007
    );
    // The rejected context spends nothing.
    assert_eq!(s.spent(&rule), 60 * SCALAR_7);
}

#[test]
#[should_panic(expected = "Error(Contract, #4006)")]
fn test_enforce_blocks_other_token_function() {
    let s = Setup::new();
    let rule = s.install(0);

    let args = vec![&s.e, s.smart_account.into_val(&s.e), 1i128.into_val(&s.e)];
    s.enforce(&s.call(&s.token, "burn", args), &rule);
}

#[test]
#[should_panic(expected = "Error(Contract, #4009)")]
fn test_enforce_blocks_negative_amount() {
    let s = Setup::new();
    let rule = s.install(0);

    s.enforce(&s.transfer(s.market.into_val(&s.e), -1), &rule);
}

#[test]
#[should_panic(expected = "Error(Contract, #4009)")]
fn test_enforce_blocks_undecodable_amount() {
    let s = Setup::new();
    let rule = s.install(0);

    let e = &s.e;
    let args = vec![
        e,
        s.smart_account.into_val(e),
        s.market.into_val(e),
        5u32.into_val(e),
    ];
    s.enforce(&s.call(&s.token, "transfer", args), &rule);
}

#[test]
#[should_panic(expected = "Error(Contract, #4009)")]
fn test_enforce_blocks_missing_amount() {
    let s = Setup::new();
    let rule = s.install(0);

    let e = &s.e;
    let args = vec![e, s.smart_account.into_val(e), s.market.into_val(e)];
    s.enforce(&s.call(&s.token, "transfer", args), &rule);
}

// ==========================================
// Enforce — Multi-rule Isolation Tests
// ==========================================

#[test]
fn test_separate_rules_have_separate_budgets() {
    let s = Setup::new();
    let rule_0 = s.install(0);
    let rule_1 = s.install(1);

    s.enforce(&s.transfer(s.market.into_val(&s.e), LIMIT), &rule_0);
    s.enforce(&s.transfer(s.market.into_val(&s.e), SCALAR_7), &rule_1);

    assert_eq!(s.spent(&rule_0), LIMIT);
    assert_eq!(s.spent(&rule_1), SCALAR_7);
}

#[test]
fn test_reinstall_starts_with_fresh_budget() {
    let s = Setup::new();
    let rule = s.install(0);
    s.enforce(&s.transfer(s.market.into_val(&s.e), 60 * SCALAR_7), &rule);

    s.client.uninstall(&rule, &s.smart_account);
    s.install(0);

    assert_eq!(s.spent(&rule), 0);
}

// ==========================================
// Regression Tests — v1 bypasses
// ==========================================

#[test]
fn test_regression_approve_to_attacker_is_capped() {
    let s = Setup::new();
    let rule = s.install(0);
    let attacker = Address::generate(&s.e);

    // v1 let `approve(wallet, attacker, MAX)` through.
    assert_eq!(
        s.enforce_error(&s.approve(&attacker, i128::MAX), &rule),
        4007
    );
    assert_eq!(
        s.enforce_error(&s.approve(&attacker, LIMIT + 1), &rule),
        4007
    );

    // Within the budget it passes and counts: the loss is bounded by it.
    s.enforce(&s.approve(&attacker, LIMIT), &rule);
    assert_eq!(s.spent(&rule), LIMIT);
}

#[test]
fn test_regression_muxed_transfer_counts() {
    let s = Setup::new();
    let rule = s.install(0);

    // v1 skipped its destination check on a muxed `to`. v2 never decodes the
    // destination, so the amount counts like any other.
    let attacker_g = MuxedAddress::generate(&s.e).address();
    let muxed_attacker = MuxedAddress::new(attacker_g, 42);

    s.enforce(&s.transfer(muxed_attacker.to_val(), 60 * SCALAR_7), &rule);
    assert_eq!(s.spent(&rule), 60 * SCALAR_7);
    assert_eq!(
        s.enforce_error(
            &s.transfer(muxed_attacker.to_val(), 40 * SCALAR_7 + 1),
            &rule
        ),
        4007
    );
}

#[test]
fn test_regression_router_fee_envelope_is_capped() {
    let s = Setup::new();
    let rule = s.install(0);
    let e = &s.e;

    // The user contexts of `multicall_with_fee(calls = [], token,
    // max_fee_amount = balance, ..)`. The fee reaches its recipient by
    // `transfer_from` with the router as spender, so the approve is where it
    // shows.
    let calls: Vec<Val> = Vec::new(e);
    let envelope = s.call(
        &s.router,
        "multicall_with_fee",
        vec![
            e,
            calls.into_val(e),
            s.token.into_val(e),
            BALANCE.into_val(e),
            1_000u32.into_val(e),
        ],
    );
    s.enforce(&envelope, &rule);
    assert_eq!(s.enforce_error(&s.approve(&s.router, BALANCE), &rule), 4007);
    assert_eq!(s.spent(&rule), 0);
}
