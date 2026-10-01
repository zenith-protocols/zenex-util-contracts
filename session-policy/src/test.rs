extern crate std;

use soroban_sdk::{
    auth::{Context, ContractContext, ContractExecutable, CreateContractHostFnContext},
    testutils::{storage::Instance as _, Address as _, MuxedAddress as _},
    vec, Address, Bytes, BytesN, Env, IntoVal, MuxedAddress, String, Symbol, Val, Vec,
};
use stellar_accounts::smart_account::{ContextRule, ContextRuleType, Signer};

use crate::{
    SessionPolicyContract, SessionPolicyContractClient, FEE_RECIPIENT, FORWARDER, MARKETS, TOKEN,
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
    /// Not configured: the target an honest forward calls.
    router: Address,
    market: Address,
    token: Address,
    fee_recipient: Address,
}

/// An ed25519 session key signer, verified by `verifier`.
fn session_signer(e: &Env, key_byte: u8) -> Signer {
    Signer::External(Address::generate(e), Bytes::from_array(e, &[key_byte; 32]))
}

/// A `Default` session rule with id `id`, as the frontend registers it: one
/// session key.
fn session_rule(e: &Env, id: u32) -> ContextRule {
    ContextRule {
        id,
        context_type: ContextRuleType::Default,
        name: String::from_str(e, "Trading Session"),
        signers: vec![e, session_signer(e, 7)],
        signer_ids: vec![e, 1],
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

    /// Enforces `context` with every signer of `rule` authenticated, as the
    /// wallet reports a correctly signed session.
    fn enforce(&self, context: &Context, rule: &ContextRule) {
        self.client
            .enforce(context, &rule.signers, rule, &self.smart_account);
    }

    /// Enforces `context` with `signers` reported as authenticated.
    fn enforce_with(&self, context: &Context, rule: &ContextRule, signers: &Vec<Signer>) {
        self.client
            .enforce(context, signers, rule, &self.smart_account);
    }

    /// The contract error code `enforce` fails with, every signer of `rule`
    /// authenticated.
    fn enforce_error(&self, context: &Context, rule: &ContextRule) -> u32 {
        self.enforce_error_signed_by(context, rule, &rule.signers)
    }

    /// The contract error code `enforce` fails with when the wallet reports
    /// `signers` as authenticated.
    fn enforce_error_signed_by(
        &self,
        context: &Context,
        rule: &ContextRule,
        signers: &Vec<Signer>,
    ) -> u32 {
        match self
            .client
            .try_enforce(context, signers, rule, &self.smart_account)
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
        assert_eq!(instance.all().len(), 4, "four entries and nothing else");
        assert_eq!(instance.get(&FORWARDER), Some(s.forwarder.clone()));
        assert_eq!(instance.get(&MARKETS), Some(vec![e, s.market.clone()]));
        assert_eq!(instance.get(&TOKEN), Some(s.token.clone()));
        assert_eq!(instance.get(&FEE_RECIPIENT), Some(s.fee_recipient.clone()));
    });

    // With no getter, these names are what off-chain readers look up.
    for (key, name) in [
        (FORWARDER, "forwarder"),
        (MARKETS, "markets"),
        (TOKEN, "token"),
        (FEE_RECIPIENT, "recipient"),
    ] {
        assert_eq!(key, Symbol::new(e, name));
    }
}

/// Four distinct addresses: forwarder, market, token, recipient.
fn addresses(e: &Env) -> [Address; 4] {
    core::array::from_fn(|_| Address::generate(e))
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
// Enforce — Signer Tests
// ==========================================

/// The contexts of an honest trade, which the rest of the suite allows when
/// the session key signed.
fn honest_contexts(s: &Setup) -> [Context; 4] {
    [
        s.forward("forward_dynamic", "create_and_fill"),
        s.approve(&s.forwarder, SCALAR_7),
        s.call(&s.market, "create_order", vec![&s.e]),
        s.transfer(s.market.into_val(&s.e), SCALAR_7),
    ]
}

#[test]
fn test_enforce_rejects_every_context_without_a_signature() {
    // The wallet hands a rule with a policy to `enforce` even when no signer
    // signed: an empty authorization must not pass.
    let s = Setup::new();
    let rule = s.install(0);
    let nobody: Vec<Signer> = Vec::new(&s.e);
    for context in honest_contexts(&s) {
        assert_eq!(s.enforce_error_signed_by(&context, &rule, &nobody), 4007);
    }
}

#[test]
fn test_enforce_rejects_a_missing_rule_signer() {
    // A rule with two signers needs both: one of them is not enough.
    let s = Setup::new();
    let mut rule = s.install(0);
    rule.signers.push_back(session_signer(&s.e, 8));
    rule.signer_ids.push_back(2);
    let only_first: Vec<Signer> = vec![&s.e, rule.signers.get_unchecked(0)];
    for context in honest_contexts(&s) {
        assert_eq!(
            s.enforce_error_signed_by(&context, &rule, &only_first),
            4007
        );
        s.enforce_with(&context, &rule, &rule.signers);
    }
}

#[test]
fn test_enforce_rejects_a_rule_without_signers() {
    // A rule that lists no signer can never be satisfied.
    let s = Setup::new();
    let mut rule = s.install(0);
    rule.signers = Vec::new(&s.e);
    rule.signer_ids = Vec::new(&s.e);
    let nobody: Vec<Signer> = Vec::new(&s.e);
    for context in honest_contexts(&s) {
        assert_eq!(s.enforce_error_signed_by(&context, &rule, &nobody), 4007);
    }
}

#[test]
fn test_enforce_allows_the_honest_trade_when_signed() {
    let s = Setup::new();
    let rule = s.install(0);
    for context in honest_contexts(&s) {
        s.enforce(&context, &rule);
    }
}

// ==========================================
// Enforce — Forwarder Tests
// ==========================================

#[test]
fn test_enforce_allows_any_forward_paying_the_recipient() {
    let s = Setup::new();
    let rule = s.install(0);
    let other = Address::generate(&s.e);

    // Only the signed recipient is pinned: any target, any function. Every
    // call the forward makes that needs the wallet is a context of its own.
    let targets = [
        (&s.router, "multicall"),
        (&s.router, "create_and_fill"),
        (&s.router, "create_and_try_fill"),
        (&s.market, "create_order"),
        (&other, "anything"),
    ];
    for fn_name in ["forward", "forward_dynamic"] {
        for (target, target_fn) in targets {
            let args = s.projection(fn_name, &s.token, &s.fee_recipient, target, target_fn);
            s.enforce(&s.call(&s.forwarder, fn_name, args), &rule);
        }
    }
}

#[test]
fn test_enforce_blocks_a_forward_paying_another_recipient() {
    let s = Setup::new();
    let rule = s.install(0);
    let attacker = Address::generate(&s.e);

    for fn_name in ["forward", "forward_dynamic"] {
        let args = s.projection(fn_name, &s.token, &attacker, &s.router, "create_and_fill");
        assert_eq!(
            s.enforce_error(&s.call(&s.forwarder, fn_name, args), &rule),
            4006,
            "{fn_name}"
        );
    }
}

#[test]
fn test_enforce_blocks_a_forward_without_a_recipient_address() {
    let s = Setup::new();
    let rule = s.install(0);
    let e = &s.e;

    // A recipient that is not an address, or no recipient at all.
    let mut bad_recipient = s.projection(
        "forward_dynamic",
        &s.token,
        &s.fee_recipient,
        &s.router,
        "create_and_fill",
    );
    bad_recipient.set(3, 7i128.into_val(e));
    assert_eq!(
        s.enforce_error(
            &s.call(&s.forwarder, "forward_dynamic", bad_recipient),
            &rule
        ),
        4006
    );
    assert_eq!(
        s.enforce_error(&s.call(&s.forwarder, "forward_dynamic", vec![e]), &rule),
        4006
    );
}

#[test]
fn test_enforce_blocks_a_fee_in_another_token() {
    let s = Setup::new();
    let rule = s.install(0);
    let other_token = Address::generate(&s.e);

    // The root does not pin the fee token, but a fee in another token needs
    // that token's `approve`, and only the configured token is allowed.
    let args = s.projection(
        "forward_dynamic",
        &other_token,
        &s.fee_recipient,
        &s.router,
        "create_and_fill",
    );
    s.enforce(&s.call(&s.forwarder, "forward_dynamic", args), &rule);
    let e = &s.e;
    let approve = vec![
        e,
        s.smart_account.into_val(e),
        s.forwarder.into_val(e),
        SCALAR_7.into_val(e),
        1_000u32.into_val(e),
    ];
    assert_eq!(
        s.enforce_error(&s.call(&other_token, "approve", approve), &rule),
        4002
    );
}

// ==========================================
// Enforce — Market Tests
// ==========================================

#[test]
fn test_enforce_allows_any_market_function() {
    let s = Setup::new();
    let rule = s.install(0);

    // A market call that needs the wallet acts on the wallet's own funds, so
    // vault deposits and their cancels pass too.
    for name in [
        "create_order",
        "cancel_order",
        "claim_credit",
        "create_vault_order",
        "cancel_vault_order",
    ] {
        s.enforce(&s.call(&s.market, name, vec![&s.e]), &rule);
    }
}

#[test]
fn test_enforce_allows_every_configured_market() {
    let e = Env::default();
    e.mock_all_auths();
    let [forwarder, market_a, token, recipient] = addresses(&e);
    let market_b = Address::generate(&e);
    let policy = e.register(
        SessionPolicyContract,
        (
            forwarder,
            vec![&e, market_a.clone(), market_b.clone()],
            token.clone(),
            recipient,
        ),
    );
    let client = SessionPolicyContractClient::new(&e, &policy);
    let smart_account = Address::generate(&e);
    let rule = session_rule(&e, 0);
    let signers: Vec<Signer> = rule.signers.clone();

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

    // The wallet itself (no signer, rule or upgrade calls), a router (an old
    // `*_with_fee` path included), the vault and any other token.
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
fn test_enforce_blocks_a_vault_share_transfer() {
    let s = Setup::new();
    let rule = s.install(0);
    let vault = Address::generate(&s.e);
    let e = &s.e;

    // A vault redeem moves the wallet's shares with `vault.transfer`: a
    // session can deposit into the vault but never withdraw.
    let shares = vec![
        e,
        s.smart_account.into_val(e),
        s.market.into_val(e),
        SCALAR_7.into_val(e),
    ];
    assert_eq!(
        s.enforce_error(&s.call(&vault, "transfer", shares), &rule),
        4002
    );
}

#[test]
fn test_enforce_blocks_the_wallet_itself() {
    let s = Setup::new();
    let rule = s.install(0);

    for name in [
        "add_context_rule",
        "remove_context_rule",
        "add_signer",
        "add_policy",
        "upgrade",
    ] {
        assert_eq!(
            s.enforce_error(&s.call(&s.smart_account, name, vec![&s.e]), &rule),
            4002,
            "{name}"
        );
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
    // spend its own allowance. The policy accepts neither the envelope nor an
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
