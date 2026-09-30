extern crate std;

use soroban_sdk::{
    auth::{Context, ContractContext, ContractExecutable, CreateContractHostFnContext},
    testutils::{storage::Instance as _, Address as _, MuxedAddress as _},
    vec, Address, BytesN, Env, IntoVal, MuxedAddress, String, Symbol, Val, Vec,
};
use stellar_accounts::smart_account::{ContextRule, ContextRuleType, Signer};

use crate::contract::{
    SessionPolicyContract, SessionPolicyContractClient, FEE_RECIPIENT, FORWARDER, MARKETS, ROUTER,
    TOKEN,
};

const SCALAR_7: i128 = 10_000_000; // one token unit at 7 decimals
const BALANCE: i128 = 1_000 * SCALAR_7; // the wallet's $1,000

// ==========================================
// Helpers
// ==========================================

struct Setup<'a> {
    e: Env,
    client: SessionPolicyContractClient<'a>,
    smart_account: Address,
    forwarder: Address,
    router: Address,
    market: Address,
    token: Address,
    fee_recipient: Address,
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
        let forwarder = Address::generate(&e);
        let router = Address::generate(&e);
        let market = Address::generate(&e);
        let token = Address::generate(&e);
        let fee_recipient = Address::generate(&e);
        let policy = e.register(
            SessionPolicyContract,
            (
                forwarder.clone(),
                router.clone(),
                vec![&e, market.clone()],
                token.clone(),
                fee_recipient.clone(),
            ),
        );
        Setup {
            client: SessionPolicyContractClient::new(&e, &policy),
            smart_account: Address::generate(&e),
            forwarder,
            router,
            market,
            token,
            fee_recipient,
            e,
        }
    }

    /// Installs the policy under rule `id` with the empty parameter and
    /// returns the rule.
    fn install(&self, id: u32) -> ContextRule {
        let rule = session_rule(&self.e, id);
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

    /// The signed projection of a forward that pays `fee_token` to
    /// `recipient` and targets `target.target_fn`. `forward` appends
    /// `target_args`.
    fn projection(
        &self,
        fn_name: &str,
        fee_token: &Address,
        recipient: &Address,
        target: &Address,
        target_fn: &str,
    ) -> Vec<Val> {
        let e = &self.e;
        let mut args = vec![
            e,
            fee_token.into_val(e),
            SCALAR_7.into_val(e),
            1_000u32.into_val(e),
            recipient.into_val(e),
            target.into_val(e),
            Symbol::new(e, target_fn).into_val(e),
        ];
        if fn_name == "forward" {
            let target_args: Vec<Val> = Vec::new(e);
            args.push_back(target_args.into_val(e));
        }
        args
    }

    /// The root context of an honest relayed trade.
    fn forward(&self, fn_name: &str, target_fn: &str) -> Context {
        let args = self.projection(
            fn_name,
            &self.token,
            &self.fee_recipient,
            &self.router,
            target_fn,
        );
        self.call(&self.forwarder, fn_name, args)
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
}

// ==========================================
// Constructor Tests
// ==========================================

#[test]
fn test_constructor_stores_one_instance_entry_per_value() {
    let s = Setup::new();
    let e = &s.e;
    e.as_contract(&s.client.address, || {
        let instance = e.storage().instance();
        assert_eq!(instance.all().len(), 5, "five entries and nothing else");
        assert_eq!(instance.get(&FORWARDER), Some(s.forwarder.clone()));
        assert_eq!(instance.get(&ROUTER), Some(s.router.clone()));
        assert_eq!(instance.get(&MARKETS), Some(vec![e, s.market.clone()]));
        assert_eq!(instance.get(&TOKEN), Some(s.token.clone()));
        assert_eq!(instance.get(&FEE_RECIPIENT), Some(s.fee_recipient.clone()));
    });

    // With no getter, these names are what off-chain readers look up.
    for (key, name) in [
        (FORWARDER, "forwarder"),
        (ROUTER, "router"),
        (MARKETS, "markets"),
        (TOKEN, "token"),
        (FEE_RECIPIENT, "recipient"),
    ] {
        assert_eq!(key, Symbol::new(e, name));
    }
}

/// Registers the policy with the given constructor arguments.
fn register(
    e: &Env,
    forwarder: &Address,
    router: &Address,
    markets: Vec<Address>,
    token: &Address,
    fee_recipient: &Address,
) {
    e.register(
        SessionPolicyContract,
        (
            forwarder.clone(),
            router.clone(),
            markets,
            token.clone(),
            fee_recipient.clone(),
        ),
    );
}

/// Five distinct addresses: forwarder, router, market, token, recipient.
fn addresses(e: &Env) -> [Address; 5] {
    core::array::from_fn(|_| Address::generate(e))
}

#[test]
#[should_panic(expected = "Error(Contract, #4001)")]
fn test_constructor_rejects_empty_markets() {
    let e = Env::default();
    let [forwarder, router, _, token, recipient] = addresses(&e);
    register(&e, &forwarder, &router, Vec::new(&e), &token, &recipient);
}

#[test]
fn test_constructor_rejects_every_overlap() {
    let e = Env::default();
    let [forwarder, router, market, token, recipient] = addresses(&e);
    let base = [&forwarder, &router, &market, &token, &recipient];

    // Every pair of roles sharing one address is rejected.
    for i in 0..5 {
        for j in (i + 1)..5 {
            let mut roles = base.map(|a| a.clone());
            roles[j] = roles[i].clone();
            let [fw, rt, mk, tk, rc] = roles;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                register(&e, &fw, &rt, vec![&e, mk.clone()], &tk, &rc)
            }));
            assert!(result.is_err(), "roles {i} and {j} overlap");
        }
    }

    // A market listed twice is an overlap too.
    let twice = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        register(
            &e,
            &forwarder,
            &router,
            vec![&e, market.clone(), market.clone()],
            &token,
            &recipient,
        )
    }));
    assert!(twice.is_err());
}

// ==========================================
// Install / Uninstall Tests
// ==========================================

#[test]
fn test_install_and_uninstall_take_empty_param_and_store_nothing() {
    let s = Setup::new();
    let rule = s.install(0);
    s.client.uninstall(&rule, &s.smart_account);
    // Reinstalling under the same rule is fine: there is no state to clash.
    s.install(0);
}

// ==========================================
// Enforce — Forwarder Tests
// ==========================================

#[test]
fn test_enforce_allows_pinned_forwards_to_every_router_target() {
    let s = Setup::new();
    let rule = s.install(0);

    for fn_name in ["forward", "forward_unsafe"] {
        for target_fn in ["multicall", "create_and_fill", "create_and_try_fill"] {
            s.enforce(&s.forward(fn_name, target_fn), &rule);
        }
    }
}

#[test]
fn test_enforce_blocks_other_forwarder_functions() {
    let s = Setup::new();
    let rule = s.install(0);
    let args = s.projection(
        "forward_unsafe",
        &s.token,
        &s.fee_recipient,
        &s.router,
        "create_and_fill",
    );

    for name in ["forward_all", "upgrade", "collect"] {
        assert_eq!(
            s.enforce_error(&s.call(&s.forwarder, name, args.clone()), &rule),
            4003,
            "{name}"
        );
    }
}

#[test]
fn test_enforce_blocks_a_forward_paying_another_recipient() {
    let s = Setup::new();
    let rule = s.install(0);
    let attacker = Address::generate(&s.e);

    for fn_name in ["forward", "forward_unsafe"] {
        let args = s.projection(fn_name, &s.token, &attacker, &s.router, "create_and_fill");
        assert_eq!(
            s.enforce_error(&s.call(&s.forwarder, fn_name, args), &rule),
            4006,
            "{fn_name}"
        );
    }
}

#[test]
fn test_enforce_blocks_a_forward_in_another_fee_token() {
    let s = Setup::new();
    let rule = s.install(0);
    let other_token = Address::generate(&s.e);

    let args = s.projection(
        "forward_unsafe",
        &other_token,
        &s.fee_recipient,
        &s.router,
        "create_and_fill",
    );
    assert_eq!(
        s.enforce_error(&s.call(&s.forwarder, "forward_unsafe", args), &rule),
        4006
    );
}

#[test]
fn test_enforce_blocks_a_forward_to_another_target() {
    let s = Setup::new();
    let rule = s.install(0);

    // The token itself (a drain through the forwarder), a market directly,
    // and any other contract.
    for target in [s.token.clone(), s.market.clone(), Address::generate(&s.e)] {
        let args = s.projection(
            "forward_unsafe",
            &s.token,
            &s.fee_recipient,
            &target,
            "create_and_fill",
        );
        assert_eq!(
            s.enforce_error(&s.call(&s.forwarder, "forward_unsafe", args), &rule),
            4006
        );
    }
}

#[test]
fn test_enforce_blocks_a_forward_to_another_router_function() {
    let s = Setup::new();
    let rule = s.install(0);

    for target_fn in [
        "multicall_try",
        "multicall_with_fee",
        "create_and_fill_with_fee",
        "transfer_from",
    ] {
        let args = s.projection(
            "forward_unsafe",
            &s.token,
            &s.fee_recipient,
            &s.router,
            target_fn,
        );
        assert_eq!(
            s.enforce_error(&s.call(&s.forwarder, "forward_unsafe", args), &rule),
            4006,
            "{target_fn}"
        );
    }
}

#[test]
fn test_enforce_blocks_a_malformed_projection() {
    let s = Setup::new();
    let rule = s.install(0);
    let e = &s.e;

    // `forward` without `target_args`, and `forward_unsafe` with them.
    let short = s.projection(
        "forward_unsafe",
        &s.token,
        &s.fee_recipient,
        &s.router,
        "multicall",
    );
    assert_eq!(
        s.enforce_error(&s.call(&s.forwarder, "forward", short), &rule),
        4006
    );
    let long = s.projection(
        "forward",
        &s.token,
        &s.fee_recipient,
        &s.router,
        "multicall",
    );
    assert_eq!(
        s.enforce_error(&s.call(&s.forwarder, "forward_unsafe", long), &rule),
        4006
    );

    // A recipient that is not an address, and a target function that is not
    // a symbol.
    let mut bad_recipient = s.projection(
        "forward_unsafe",
        &s.token,
        &s.fee_recipient,
        &s.router,
        "create_and_fill",
    );
    bad_recipient.set(3, 7i128.into_val(e));
    assert_eq!(
        s.enforce_error(
            &s.call(&s.forwarder, "forward_unsafe", bad_recipient),
            &rule
        ),
        4006
    );
    let mut bad_target_fn = s.projection(
        "forward_unsafe",
        &s.token,
        &s.fee_recipient,
        &s.router,
        "create_and_fill",
    );
    bad_target_fn.set(5, String::from_str(e, "create_and_fill").into_val(e));
    assert_eq!(
        s.enforce_error(
            &s.call(&s.forwarder, "forward_unsafe", bad_target_fn),
            &rule
        ),
        4006
    );
    assert_eq!(
        s.enforce_error(&s.call(&s.forwarder, "forward_unsafe", vec![e]), &rule),
        4006
    );
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
}

#[test]
fn test_enforce_allows_every_configured_market() {
    let e = Env::default();
    e.mock_all_auths();
    let [forwarder, router, market_a, token, recipient] = addresses(&e);
    let market_b = Address::generate(&e);
    let policy = e.register(
        SessionPolicyContract,
        (
            forwarder,
            router,
            vec![&e, market_a.clone(), market_b.clone()],
            token.clone(),
            recipient,
        ),
    );
    let client = SessionPolicyContractClient::new(&e, &policy);
    let smart_account = Address::generate(&e);
    let rule = session_rule(&e, 0);
    let signers: Vec<Signer> = Vec::new(&e);

    for market in [&market_a, &market_b] {
        let order = Context::Contract(ContractContext {
            contract: market.clone(),
            fn_name: Symbol::new(&e, "create_order"),
            args: vec![&e],
        });
        client.enforce(&order, &signers, &rule, &smart_account);
        let escrow = Context::Contract(ContractContext {
            contract: token.clone(),
            fn_name: Symbol::new(&e, "transfer"),
            args: vec![
                &e,
                smart_account.into_val(&e),
                market.into_val(&e),
                SCALAR_7.into_val(&e),
            ],
        });
        client.enforce(&escrow, &signers, &rule, &smart_account);
    }
}

#[test]
fn test_enforce_blocks_other_market_functions() {
    let s = Setup::new();
    let rule = s.install(0);

    for name in [
        "create_vault_order",
        "cancel_vault_order",
        "execute_order",
        "set_config",
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
// Enforce — Token Transfer Tests
// ==========================================

#[test]
fn test_enforce_allows_escrow_transfer_into_market() {
    let s = Setup::new();
    let rule = s.install(0);

    // The escrow amount is the trade, not a fee: it is unconstrained.
    s.enforce(&s.transfer(s.market.into_val(&s.e), BALANCE), &rule);
}

#[test]
fn test_enforce_blocks_transfer_outside_markets() {
    let s = Setup::new();
    let rule = s.install(0);

    for to in [
        Address::generate(&s.e),
        s.forwarder.clone(),
        s.router.clone(),
        s.fee_recipient.clone(),
    ] {
        assert_eq!(
            s.enforce_error(&s.transfer(to.into_val(&s.e), SCALAR_7), &rule),
            4004
        );
    }
}

#[test]
fn test_enforce_blocks_transfer_to_muxed_address() {
    let s = Setup::new();
    let rule = s.install(0);
    let muxed = MuxedAddress::new(MuxedAddress::generate(&s.e).address(), 1);

    assert_eq!(
        s.enforce_error(&s.transfer(muxed.to_val(), SCALAR_7), &rule),
        4004
    );
}

#[test]
fn test_enforce_blocks_transfer_with_bad_to() {
    let s = Setup::new();
    let rule = s.install(0);
    let e = &s.e;

    assert_eq!(
        s.enforce_error(&s.transfer(7i128.into_val(e), SCALAR_7), &rule),
        4004
    );
    let no_to = vec![e, s.smart_account.into_val(e)];
    assert_eq!(
        s.enforce_error(&s.call(&s.token, "transfer", no_to), &rule),
        4004
    );
}

// ==========================================
// Enforce — Token Approve Tests
// ==========================================

#[test]
fn test_enforce_allows_forwarder_approve_at_any_amount() {
    let s = Setup::new();
    let rule = s.install(0);

    for amount in [0, SCALAR_7, i128::MAX] {
        s.enforce(&s.approve(&s.forwarder, amount), &rule);
    }
}

#[test]
fn test_enforce_blocks_approve_to_anyone_else() {
    let s = Setup::new();
    let rule = s.install(0);

    for spender in [
        s.router.clone(),
        s.market.clone(),
        s.fee_recipient.clone(),
        Address::generate(&s.e),
    ] {
        assert_eq!(s.enforce_error(&s.approve(&spender, SCALAR_7), &rule), 4005);
    }
    let no_spender = vec![&s.e, s.smart_account.into_val(&s.e)];
    assert_eq!(
        s.enforce_error(&s.call(&s.token, "approve", no_spender), &rule),
        4005
    );
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

    // The wallet itself (no signer, rule or upgrade calls), the router
    // directly (its old `*_with_fee` path included), the vault and any other
    // token.
    let contexts = [
        s.call(&s.smart_account, "add_context_rule", vec![&s.e]),
        s.call(&s.router, "multicall_with_fee", vec![&s.e]),
        s.call(&s.router, "create_and_fill", vec![&s.e]),
        s.call(&vault, "transfer", vec![&s.e]),
        s.call(&other_token, "approve", vec![&s.e]),
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
// Regression Tests — earlier bypasses
// ==========================================

#[test]
fn test_regression_v1_approve_to_attacker_is_blocked() {
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
fn test_regression_v1_muxed_transfer_is_blocked() {
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
fn test_regression_v1_router_fee_envelope_is_blocked() {
    let s = Setup::new();
    let rule = s.install(0);

    // v1 let `multicall_with_fee(calls = [], max_fee_amount = balance, ..)`
    // drain the wallet: its fee recipient is unsigned, and the router can
    // spend its own allowance. v4 accepts neither the envelope nor an
    // approve to the router.
    let e = &s.e;
    let calls: Vec<Val> = Vec::new(e);
    let envelope = vec![
        e,
        calls.into_val(e),
        s.token.into_val(e),
        BALANCE.into_val(e),
        1_000u32.into_val(e),
    ];
    assert_eq!(
        s.enforce_error(&s.call(&s.router, "multicall_with_fee", envelope), &rule),
        4002
    );
    assert_eq!(s.enforce_error(&s.approve(&s.router, BALANCE), &rule), 4005);
}
