extern crate std;

use soroban_sdk::{
    auth::{Context, ContractContext, ContractExecutable, CreateContractHostFnContext},
    testutils::{Address as _, MuxedAddress as _},
    vec, Address, BytesN, Env, IntoVal, MuxedAddress, String, Symbol, Val, Vec,
};
use stellar_accounts::smart_account::{ContextRule, ContextRuleType, Signer};

use crate::contract::{Config, SessionPolicyContract, SessionPolicyContractClient};

const SCALAR_7: i128 = 10_000_000; // one token unit at 7 decimals
const BUDGET: i128 = 50 * SCALAR_7; // the $50 relay-fee budget
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

/// A `Default` session rule with id `id`, as the frontend registers it.
fn session_rule(e: &Env, id: u32) -> ContextRule {
    ContextRule {
        id,
        context_type: ContextRuleType::Default,
        name: String::from_str(e, "Trading Session"),
        signers: Vec::new(e),
        signer_ids: Vec::new(e),
        policies: Vec::new(e),
        policy_ids: Vec::new(e),
        valid_until: None,
    }
}

impl Setup<'_> {
    fn new() -> Self {
        let e = Env::default();
        e.mock_all_auths();
        let market = Address::generate(&e);
        let router = Address::generate(&e);
        let token = Address::generate(&e);
        let policy = e.register(
            SessionPolicyContract,
            (
                vec![&e, market.clone()],
                token.clone(),
                router.clone(),
                BUDGET,
            ),
        );
        Setup {
            client: SessionPolicyContractClient::new(&e, &policy),
            smart_account: Address::generate(&e),
            market,
            router,
            token,
            e,
        }
    }

    fn rule(&self, id: u32) -> ContextRule {
        session_rule(&self.e, id)
    }

    /// Installs the policy under rule `id` with the empty parameter and
    /// returns the rule.
    fn install(&self, id: u32) -> ContextRule {
        let rule = self.rule(id);
        self.client.install(&(), &rule, &self.smart_account);
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

    /// The user contexts of a router fee envelope `fn_name(calls = [],
    /// token, max_fee_amount, fee_expiration)`: the signed prefix only.
    fn envelope(&self, fn_name: &str, max_fee: i128) -> Context {
        let e = &self.e;
        let calls: Vec<Val> = Vec::new(e);
        let args = vec![
            e,
            calls.into_val(e),
            self.token.into_val(e),
            max_fee.into_val(e),
            1_000u32.into_val(e),
        ];
        self.call(&self.router, fn_name, args)
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

    fn fees_spent(&self, rule: &ContextRule) -> i128 {
        self.client.get_fees_spent(&self.smart_account, &rule.id)
    }
}

// ==========================================
// Constructor Tests
// ==========================================

#[test]
fn test_constructor_stores_config() {
    let s = Setup::new();

    assert_eq!(
        s.client.get_config(),
        Config {
            markets: vec![&s.e, s.market.clone()],
            token: s.token.clone(),
            router: s.router.clone(),
            fee_budget: BUDGET,
        }
    );
}

/// Registers the policy with the given constructor arguments.
fn register(e: &Env, markets: Vec<Address>, token: &Address, router: &Address, fee_budget: i128) {
    e.register(
        SessionPolicyContract,
        (markets, token.clone(), router.clone(), fee_budget),
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #4001)")]
fn test_constructor_rejects_empty_markets() {
    let e = Env::default();
    let (token, router) = (Address::generate(&e), Address::generate(&e));
    register(&e, Vec::new(&e), &token, &router, BUDGET);
}

#[test]
#[should_panic(expected = "Error(Contract, #4001)")]
fn test_constructor_rejects_zero_fee_budget() {
    let e = Env::default();
    let (market, token, router) = (
        Address::generate(&e),
        Address::generate(&e),
        Address::generate(&e),
    );
    register(&e, vec![&e, market], &token, &router, 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #4001)")]
fn test_constructor_rejects_negative_fee_budget() {
    let e = Env::default();
    let (market, token, router) = (
        Address::generate(&e),
        Address::generate(&e),
        Address::generate(&e),
    );
    register(&e, vec![&e, market], &token, &router, -1);
}

#[test]
#[should_panic(expected = "Error(Contract, #4001)")]
fn test_constructor_rejects_token_as_market() {
    let e = Env::default();
    let (token, router) = (Address::generate(&e), Address::generate(&e));
    register(&e, vec![&e, token.clone()], &token, &router, BUDGET);
}

#[test]
#[should_panic(expected = "Error(Contract, #4001)")]
fn test_constructor_rejects_router_as_market() {
    let e = Env::default();
    let (token, router) = (Address::generate(&e), Address::generate(&e));
    register(&e, vec![&e, router.clone()], &token, &router, BUDGET);
}

#[test]
#[should_panic(expected = "Error(Contract, #4001)")]
fn test_constructor_rejects_router_as_token() {
    let e = Env::default();
    let (market, token) = (Address::generate(&e), Address::generate(&e));
    register(&e, vec![&e, market], &token, &token, BUDGET);
}

// ==========================================
// Install / Uninstall Tests
// ==========================================

#[test]
fn test_install_takes_empty_param_and_stores_nothing() {
    let s = Setup::new();
    let rule = s.install(0);

    assert_eq!(s.fees_spent(&rule), 0);
}

#[test]
fn test_uninstall_clears_fee_counter() {
    let s = Setup::new();
    let rule = s.install(0);
    s.enforce(&s.approve(&s.router, 30 * SCALAR_7), &rule);
    assert_eq!(s.fees_spent(&rule), 30 * SCALAR_7);

    s.client.uninstall(&rule, &s.smart_account);

    assert_eq!(s.fees_spent(&rule), 0);
}

#[test]
fn test_uninstall_without_counter_succeeds() {
    let s = Setup::new();
    let rule = s.install(0);

    s.client.uninstall(&rule, &s.smart_account);
}

// ==========================================
// Enforce — Market Tests
// ==========================================

#[test]
fn test_enforce_allows_market_trading_functions() {
    let s = Setup::new();
    let rule = s.install(0);

    for name in ["create_order", "cancel_order", "claim_credit"] {
        s.enforce(&s.call(&s.market, name, vec![&s.e]), &rule);
    }
    assert_eq!(s.fees_spent(&rule), 0);
}

#[test]
fn test_enforce_allows_every_configured_market() {
    let e = Env::default();
    e.mock_all_auths();
    let [market_a, market_b, token, router, account] =
        std::array::from_fn(|_| Address::generate(&e));
    let policy = e.register(
        SessionPolicyContract,
        (
            vec![&e, market_a.clone(), market_b.clone()],
            token.clone(),
            router,
            BUDGET,
        ),
    );
    let client = SessionPolicyContractClient::new(&e, &policy);
    let rule = session_rule(&e, 0);
    let signers: Vec<Signer> = Vec::new(&e);

    for market in [&market_a, &market_b] {
        let order = Context::Contract(ContractContext {
            contract: market.clone(),
            fn_name: Symbol::new(&e, "create_order"),
            args: vec![&e],
        });
        client.enforce(&order, &signers, &rule, &account);

        let escrow = Context::Contract(ContractContext {
            contract: token.clone(),
            fn_name: Symbol::new(&e, "transfer"),
            args: vec![
                &e,
                account.into_val(&e),
                market.into_val(&e),
                BALANCE.into_val(&e),
            ],
        });
        client.enforce(&escrow, &signers, &rule, &account);
    }
}

#[test]
fn test_enforce_blocks_other_market_functions() {
    let s = Setup::new();
    let rule = s.install(0);

    // Vault deposits and redeems stay behind the passkey, and the keeper
    // entry points never carry the wallet's signature.
    for name in [
        "create_vault_order",
        "cancel_vault_order",
        "execute_order",
        "upgrade",
    ] {
        assert_eq!(
            s.enforce_error(&s.call(&s.market, name, vec![&s.e]), &rule),
            4003,
            "{name}"
        );
    }
}

// ==========================================
// Enforce — Router Tests
// ==========================================

#[test]
fn test_enforce_allows_router_fee_envelopes() {
    let s = Setup::new();
    let rule = s.install(0);

    for name in [
        "multicall_with_fee",
        "create_and_fill_with_fee",
        "create_and_try_fill_with_fee",
    ] {
        s.enforce(&s.envelope(name, SCALAR_7), &rule);
    }
    // The envelope's max fee counts at its approve, not here.
    assert_eq!(s.fees_spent(&rule), 0);
}

#[test]
fn test_enforce_blocks_other_router_functions() {
    let s = Setup::new();
    let rule = s.install(0);

    for name in [
        "multicall",
        "multicall_try",
        "create_and_fill",
        "create_and_try_fill",
    ] {
        assert_eq!(
            s.enforce_error(&s.call(&s.router, name, vec![&s.e]), &rule),
            4003,
            "{name}"
        );
    }
}

// ==========================================
// Enforce — Token Transfer Tests
// ==========================================

#[test]
fn test_enforce_allows_escrow_transfer_into_market() {
    let s = Setup::new();
    let rule = s.install(0);

    // The escrow amount is unconstrained and spends no fee budget.
    s.enforce(&s.transfer(s.market.into_val(&s.e), BALANCE), &rule);
    assert_eq!(s.fees_spent(&rule), 0);
}

#[test]
fn test_enforce_blocks_transfer_outside_markets() {
    let s = Setup::new();
    let rule = s.install(0);
    let attacker = Address::generate(&s.e);

    for to in [&attacker, &s.router, &s.token, &s.smart_account] {
        assert_eq!(
            s.enforce_error(&s.transfer(to.into_val(&s.e), 1), &rule),
            4004
        );
    }
}

#[test]
#[should_panic(expected = "Error(Contract, #4004)")]
fn test_enforce_blocks_transfer_to_muxed_address() {
    let s = Setup::new();
    let rule = s.install(0);

    let attacker_g = MuxedAddress::generate(&s.e).address();
    let muxed_attacker = MuxedAddress::new(attacker_g, 42);
    s.enforce(&s.transfer(muxed_attacker.to_val(), 1), &rule);
}

#[test]
#[should_panic(expected = "Error(Contract, #4004)")]
fn test_enforce_blocks_transfer_with_non_address_to() {
    let s = Setup::new();
    let rule = s.install(0);

    s.enforce(&s.transfer(7u32.into_val(&s.e), 1), &rule);
}

#[test]
#[should_panic(expected = "Error(Contract, #4004)")]
fn test_enforce_blocks_transfer_without_to() {
    let s = Setup::new();
    let rule = s.install(0);

    let args = vec![&s.e, s.smart_account.into_val(&s.e)];
    s.enforce(&s.call(&s.token, "transfer", args), &rule);
}

// ==========================================
// Enforce — Token Approve Tests
// ==========================================

#[test]
fn test_enforce_allows_router_approve_within_budget() {
    let s = Setup::new();
    let rule = s.install(0);

    s.enforce(&s.approve(&s.router, SCALAR_7), &rule);
    assert_eq!(s.fees_spent(&rule), SCALAR_7);
}

#[test]
fn test_enforce_zero_approve_spends_nothing() {
    let s = Setup::new();
    let rule = s.install(0);

    // The router wipes its leftover allowance with `approve(.., 0, ..)`.
    s.enforce(&s.approve(&s.router, 0), &rule);
    assert_eq!(s.fees_spent(&rule), 0);
}

#[test]
fn test_enforce_fee_budget_is_exact() {
    let s = Setup::new();
    let rule = s.install(0);

    s.enforce(&s.approve(&s.router, 30 * SCALAR_7), &rule);
    s.enforce(&s.approve(&s.router, 20 * SCALAR_7), &rule);
    assert_eq!(s.fees_spent(&rule), BUDGET);

    // One more unit is over budget, and the rejected context spends nothing.
    assert_eq!(s.enforce_error(&s.approve(&s.router, 1), &rule), 4007);
    assert_eq!(s.fees_spent(&rule), BUDGET);
}

#[test]
#[should_panic(expected = "Error(Contract, #4007)")]
fn test_enforce_blocks_single_approve_over_budget() {
    let s = Setup::new();
    let rule = s.install(0);

    s.enforce(&s.approve(&s.router, BUDGET + 1), &rule);
}

#[test]
fn test_enforce_blocks_approve_to_other_spenders() {
    let s = Setup::new();
    let rule = s.install(0);
    let attacker = Address::generate(&s.e);

    for spender in [&attacker, &s.market, &s.token] {
        assert_eq!(s.enforce_error(&s.approve(spender, 0), &rule), 4005);
    }
}

#[test]
#[should_panic(expected = "Error(Contract, #4005)")]
fn test_enforce_blocks_approve_without_spender() {
    let s = Setup::new();
    let rule = s.install(0);

    let args = vec![&s.e, s.smart_account.into_val(&s.e)];
    s.enforce(&s.call(&s.token, "approve", args), &rule);
}

#[test]
#[should_panic(expected = "Error(Contract, #4006)")]
fn test_enforce_blocks_negative_approve() {
    let s = Setup::new();
    let rule = s.install(0);

    s.enforce(&s.approve(&s.router, -1), &rule);
}

#[test]
#[should_panic(expected = "Error(Contract, #4006)")]
fn test_enforce_blocks_undecodable_approve_amount() {
    let s = Setup::new();
    let rule = s.install(0);

    let e = &s.e;
    let args = vec![
        e,
        s.smart_account.into_val(e),
        s.router.into_val(e),
        5u32.into_val(e),
    ];
    s.enforce(&s.call(&s.token, "approve", args), &rule);
}

#[test]
#[should_panic(expected = "Error(Contract, #4006)")]
fn test_enforce_blocks_approve_without_amount() {
    let s = Setup::new();
    let rule = s.install(0);

    let e = &s.e;
    let args = vec![e, s.smart_account.into_val(e), s.router.into_val(e)];
    s.enforce(&s.call(&s.token, "approve", args), &rule);
}

#[test]
fn test_enforce_blocks_other_token_functions() {
    let s = Setup::new();
    let rule = s.install(0);

    for name in [
        "burn",
        "burn_from",
        "transfer_from",
        "mint",
        "clawback",
        "set_admin",
        "set_authorized",
    ] {
        assert_eq!(
            s.enforce_error(&s.call(&s.token, name, vec![&s.e]), &rule),
            4003,
            "{name}"
        );
    }
}

// ==========================================
// Enforce — Other Contract Tests
// ==========================================

#[test]
fn test_enforce_blocks_other_contracts() {
    let s = Setup::new();
    let rule = s.install(0);
    let (vault, other_token) = (Address::generate(&s.e), Address::generate(&s.e));

    // The wallet itself (no signer, rule or upgrade calls), the vault, and
    // any other token.
    let contexts = [
        s.call(&s.smart_account, "add_context_rule", vec![&s.e]),
        s.call(&vault, "transfer", vec![&s.e]),
        s.call(&other_token, "transfer", vec![&s.e]),
    ];
    for context in contexts.iter() {
        assert_eq!(s.enforce_error(context, &rule), 4002);
    }
}

#[test]
#[should_panic(expected = "Error(Contract, #4002)")]
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
// Enforce — Fee Counter Isolation Tests
// ==========================================

#[test]
fn test_separate_rules_have_separate_budgets() {
    let s = Setup::new();
    let rule_0 = s.install(0);
    let rule_1 = s.install(1);

    s.enforce(&s.approve(&s.router, BUDGET), &rule_0);
    s.enforce(&s.approve(&s.router, SCALAR_7), &rule_1);

    assert_eq!(s.fees_spent(&rule_0), BUDGET);
    assert_eq!(s.fees_spent(&rule_1), SCALAR_7);
}

#[test]
fn test_new_rule_renews_the_budget() {
    let s = Setup::new();
    let spent_rule = s.install(0);
    s.enforce(&s.approve(&s.router, BUDGET), &spent_rule);
    assert_eq!(s.enforce_error(&s.approve(&s.router, 1), &spent_rule), 4007);

    // Renewal: the frontend removes the spent rule and adds a new one.
    s.client.uninstall(&spent_rule, &s.smart_account);
    let renewed = s.install(1);

    s.enforce(&s.approve(&s.router, BUDGET), &renewed);
    assert_eq!(s.fees_spent(&renewed), BUDGET);
}

// ==========================================
// Regression Tests — v1 bypasses
// ==========================================

#[test]
fn test_regression_approve_to_attacker_is_blocked() {
    let s = Setup::new();
    let rule = s.install(0);
    let attacker = Address::generate(&s.e);

    // v1 let `approve(wallet, attacker, MAX)` through.
    assert_eq!(
        s.enforce_error(&s.approve(&attacker, i128::MAX), &rule),
        4005
    );
}

#[test]
fn test_regression_muxed_transfer_is_blocked() {
    let s = Setup::new();
    let rule = s.install(0);

    // v1 skipped its destination check on a muxed `to`.
    let attacker_g = MuxedAddress::generate(&s.e).address();
    let muxed_attacker = MuxedAddress::new(attacker_g, 42);
    assert_eq!(
        s.enforce_error(&s.transfer(muxed_attacker.to_val(), BALANCE), &rule),
        4004
    );
}

#[test]
fn test_regression_router_fee_envelope_is_capped() {
    let s = Setup::new();
    let rule = s.install(0);

    // v1 let `multicall_with_fee(calls = [], max_fee_amount = balance, ..)`
    // drain the wallet: the fee reaches its recipient by `transfer_from` with
    // the router as spender, so only the approve shows.
    s.enforce(&s.envelope("multicall_with_fee", BALANCE), &rule);
    assert_eq!(s.enforce_error(&s.approve(&s.router, BALANCE), &rule), 4007);
    assert_eq!(s.fees_spent(&rule), 0);

    // The most any sequence of envelopes can take is the budget.
    s.enforce(&s.approve(&s.router, BUDGET), &rule);
    assert_eq!(s.enforce_error(&s.approve(&s.router, 1), &rule), 4007);
}
