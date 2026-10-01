extern crate std;

use crate::{FeeForwarderContract, FeeForwarderContractClient};
use soroban_sdk::testutils::{
    Address as _, AuthorizedFunction, AuthorizedInvocation, MockAuth, MockAuthInvoke,
};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, token, Address, Bytes,
    Env, Error, IntoVal, MuxedAddress, Symbol, TryFromVal, Val, Vec,
};
use stellar_fee_abstraction::{collect_fee_and_invoke, FeeAbstractionApproval};

// The stellar-fee-abstraction bounds-check discriminant.
const INVALID_FEE_BOUNDS: u32 = 5003;
// The forwarder's refusal of an allowance-spending target.
const TARGET_NOT_ALLOWED: u32 = 6001;
// The forwarder's refusal to pay the fee to itself.
const INVALID_RECIPIENT: u32 = 6002;
// The mock target's deliberate failure.
const PROBE_FAILED: u32 = 99;

const EXEC_FEE: i128 = 100_000; // 0.01 at 7 decimals, the deployed exec fee
const MARGIN: i128 = 2_000_000_000; // 200
const MAX_FEE: i128 = 10_000_000; // 1
const FEE: i128 = 4_000_000; // 0.4
const EXPIRATION: u32 = 1_000;
const BALANCE: i128 = 1_000 * 10_000_000; // 1000

// ==========================================
// Mocks
// ==========================================

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProbeError {
    Failed = 99,
}

#[contracttype]
enum ProbeKey {
    LastTag,
}

/// A target that reports the fee allowance it sees while it runs.
#[contract]
struct MockProbe;

#[contractimpl]
impl MockProbe {
    /// Records `tag` and returns `user`'s allowance to `forwarder` in `token`
    /// at the moment the target runs. Needs no authorization.
    pub fn probe(e: Env, user: Address, token: Address, forwarder: Address, tag: u32) -> i128 {
        e.storage().instance().set(&ProbeKey::LastTag, &tag);
        token::Client::new(&e, &token).allowance(&user, &forwarder)
    }

    /// Requires `user`'s authorization on its exact arguments, like a market
    /// order does.
    pub fn act(e: Env, user: Address, tag: u32) -> u32 {
        user.require_auth();
        e.storage().instance().set(&ProbeKey::LastTag, &tag);
        tag
    }

    pub fn fail(e: Env) {
        panic_with_error!(&e, ProbeError::Failed);
    }

    pub fn last_tag(e: Env) -> u32 {
        e.storage().instance().get(&ProbeKey::LastTag).unwrap_or(0)
    }
}

#[contracttype]
enum MarketKey {
    Token,
    Next,
}

/// A market with the real `create_order` signature: it requires the user's
/// authorization and pulls `margin + exec_fee` into the market.
#[contract]
struct MockMarket;

#[contractimpl]
impl MockMarket {
    pub fn __constructor(e: Env, token: Address) {
        e.storage().instance().set(&MarketKey::Token, &token);
    }

    pub fn create_order(
        e: Env,
        user: Address,
        _is_long: bool,
        _kind: u32,
        _notional: i128,
        margin: i128,
        _trigger_price: i128,
        _price_bound: i128,
        _expiration: u32,
    ) -> u32 {
        user.require_auth();
        let token: Address = e.storage().instance().get(&MarketKey::Token).unwrap();
        token::Client::new(&e, &token).transfer(
            &user,
            MuxedAddress::from(e.current_contract_address()),
            &(margin + EXEC_FEE),
        );
        let id: u32 = e.storage().instance().get(&MarketKey::Next).unwrap_or(0);
        e.storage().instance().set(&MarketKey::Next, &(id + 1));
        id
    }

    pub fn execute_order(e: Env, keeper: Address, _user: Address, _id: u32, price: Bytes) -> i128 {
        assert!(!price.is_empty());
        let token: Address = e.storage().instance().get(&MarketKey::Token).unwrap();
        token::Client::new(&e, &token).transfer(
            &e.current_contract_address(),
            MuxedAddress::from(keeper),
            &EXEC_FEE,
        );
        EXEC_FEE
    }
}

/// The market router's call descriptor.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Call {
    pub contract: Address,
    pub func: Symbol,
    pub args: Vec<Val>,
}

/// The market router's `create_and_fill`, as generic as the real one: every
/// call runs on the router's behalf with `invoke_contract`.
#[contract]
struct MockRouter;

#[contractimpl]
impl MockRouter {
    pub fn create_and_fill(
        e: Env,
        calls: Vec<Call>,
        user: Address,
        keeper: Address,
        price: Bytes,
    ) -> Vec<Val> {
        let market = calls.get(0).unwrap().contract;
        let mut results = Vec::new(&e);
        for call in calls.iter() {
            results.push_back(e.invoke_contract::<Val>(&call.contract, &call.func, call.args));
        }
        let id = u32::try_from_val(&e, &results.get(0).unwrap()).unwrap();
        let payout: i128 = e.invoke_contract(
            &market,
            &Symbol::new(&e, "execute_order"),
            (keeper, user, id, price).into_val(&e),
        );
        results.push_back(payout.into_val(&e));
        results
    }
}

/// OpenZeppelin's permissionless fee forwarder example, verbatim in logic:
/// the recipient is unsigned and any target is called. Built on the pinned
/// v0.9.0 helper, so its Eager collection consumes the whole approval.
#[contract]
struct OzExampleForwarder;

#[contractimpl]
impl OzExampleForwarder {
    pub fn forward(
        e: &Env,
        fee_token: Address,
        fee_amount: i128,
        max_fee_amount: i128,
        expiration_ledger: u32,
        target_contract: Address,
        target_fn: Symbol,
        target_args: Vec<Val>,
        user: Address,
        relayer: Address,
    ) -> Val {
        relayer.require_auth();
        collect_fee_and_invoke(
            e,
            &fee_token,
            fee_amount,
            max_fee_amount,
            expiration_ledger,
            &target_contract,
            &target_fn,
            &target_args,
            &user,
            &relayer,
            FeeAbstractionApproval::Eager,
        )
    }
}

// ==========================================
// Setup
// ==========================================

struct World {
    e: Env,
    forwarder: FeeForwarderContractClient<'static>,
    usdc: token::TokenClient<'static>,
    probe: Address,
    user: Address,
    recipient: Address,
}

fn new_token(e: &Env) -> token::TokenClient<'static> {
    let issuer = Address::generate(e);
    token::TokenClient::new(e, &e.register_stellar_asset_contract_v2(issuer).address())
}

fn mint(e: &Env, token: &Address, to: &Address, amount: i128) {
    e.mock_all_auths();
    token::StellarAssetClient::new(e, token).mint(to, &amount);
    e.set_auths(&[]);
}

fn setup() -> World {
    let e = Env::default();
    let usdc = new_token(&e);
    let forwarder = e.register(FeeForwarderContract, ());
    let probe = e.register(MockProbe, ());
    let user = Address::generate(&e);
    mint(&e, &usdc.address, &user, BALANCE);
    World {
        forwarder: FeeForwarderContractClient::new(&e, &forwarder),
        usdc,
        probe,
        user,
        recipient: Address::generate(&e),
        e,
    }
}

impl World {
    fn probe_args(&self, tag: u32) -> Vec<Val> {
        (
            self.user.clone(),
            self.usdc.address.clone(),
            self.forwarder.address.clone(),
            tag,
        )
            .into_val(&self.e)
    }

    /// The arguments `forward_dynamic` signs.
    fn dynamic_signed_args(&self, target: &Address, target_fn: &str) -> Vec<Val> {
        (
            self.usdc.address.clone(),
            MAX_FEE,
            EXPIRATION,
            self.recipient.clone(),
            target.clone(),
            Symbol::new(&self.e, target_fn),
        )
            .into_val(&self.e)
    }

    /// The arguments `forward` signs.
    fn signed_args(&self, target: &Address, target_fn: &str, target_args: &Vec<Val>) -> Vec<Val> {
        (
            self.usdc.address.clone(),
            MAX_FEE,
            EXPIRATION,
            self.recipient.clone(),
            target.clone(),
            Symbol::new(&self.e, target_fn),
            target_args.clone(),
        )
            .into_val(&self.e)
    }

    fn approve_args(&self, amount: i128) -> Vec<Val> {
        (
            self.user.clone(),
            self.forwarder.address.clone(),
            amount,
            EXPIRATION,
        )
            .into_val(&self.e)
    }

    /// Mocks exactly the tree the wallet signs when the target needs no
    /// authorization of its own: the root's signed arguments and the fee approve.
    fn sign(&self, fn_name: &str, signed_args: Vec<Val>) {
        let approve_max = self.approve_args(MAX_FEE);
        self.e.mock_auths(&[MockAuth {
            address: &self.user,
            invoke: &MockAuthInvoke {
                contract: &self.forwarder.address,
                fn_name,
                args: signed_args,
                sub_invokes: &[MockAuthInvoke {
                    contract: &self.usdc.address,
                    fn_name: "approve",
                    args: approve_max,
                    sub_invokes: &[],
                }],
            },
        }]);
    }

    fn forward_probe(&self, fn_name: &str, tag: u32, recipient: &Address) -> i128 {
        let args = self.probe_args(tag);
        let result = match fn_name {
            "forward" => self.forwarder.forward(
                &self.usdc.address,
                &FEE,
                &MAX_FEE,
                &EXPIRATION,
                &self.probe,
                &Symbol::new(&self.e, "probe"),
                &args,
                &self.user,
                recipient,
            ),
            _ => self.forwarder.forward_dynamic(
                &self.usdc.address,
                &FEE,
                &MAX_FEE,
                &EXPIRATION,
                &self.probe,
                &Symbol::new(&self.e, "probe"),
                &args,
                &self.user,
                recipient,
            ),
        };
        i128::try_from_val(&self.e, &result).unwrap()
    }

    fn allowance(&self) -> i128 {
        self.usdc.allowance(&self.user, &self.forwarder.address)
    }

    fn last_tag(&self) -> u32 {
        MockProbeClient::new(&self.e, &self.probe).last_tag()
    }
}

/// Runs a call that must fail authorization. A `try_` call only reports the
/// host's `Error(Context, InvalidAction)` wrapper, so the check reads the
/// host error the non-`try_` call panics with.
fn assert_auth_failure(call: impl FnOnce()) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(call));
    let payload = outcome.expect_err("the call must fail");
    let message = payload
        .downcast_ref::<std::string::String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|s| std::string::String::from(*s))
        })
        .unwrap_or_default();
    assert!(
        message.contains("Error(Auth, InvalidAction)"),
        "not an authorization failure: {message}"
    );
}

fn contract_error(code: u32) -> Error {
    Error::from_contract_error(code)
}

// ==========================================
// Fee collection
// ==========================================

#[test]
fn forward_collects_the_fee_refunds_the_rest_and_calls_the_target() {
    let w = setup();
    let args = w.probe_args(1);
    w.sign("forward", w.signed_args(&w.probe, "probe", &args));

    // The probe reports the allowance it sees while it runs.
    let seen = w.forward_probe("forward", 1, &w.recipient);

    assert_eq!(
        seen, 0,
        "the pull consumes the allowance before the target runs"
    );
    assert_eq!(w.allowance(), 0);
    assert_eq!(w.usdc.balance(&w.recipient), FEE);
    assert_eq!(
        w.usdc.balance(&w.user),
        BALANCE - FEE,
        "the rest is refunded"
    );
    assert_eq!(w.usdc.balance(&w.forwarder.address), 0);
    assert_eq!(w.last_tag(), 1);
}

#[test]
fn forward_dynamic_collects_the_fee_and_consumes_the_allowance() {
    let w = setup();
    w.sign("forward_dynamic", w.dynamic_signed_args(&w.probe, "probe"));

    let seen = w.forward_probe("forward_dynamic", 1, &w.recipient);

    assert_eq!(seen, 0);
    assert_eq!(w.allowance(), 0);
    assert_eq!(w.usdc.balance(&w.recipient), FEE);
    assert_eq!(w.usdc.balance(&w.user), BALANCE - FEE);
    assert_eq!(w.usdc.balance(&w.forwarder.address), 0);
}

#[test]
fn a_fee_at_the_cap_leaves_nothing_to_refund() {
    let w = setup();
    let args = w.probe_args(1);
    w.e.mock_all_auths();

    w.forwarder.forward(
        &w.usdc.address,
        &MAX_FEE,
        &MAX_FEE,
        &EXPIRATION,
        &w.probe,
        &Symbol::new(&w.e, "probe"),
        &args,
        &w.user,
        &w.recipient,
    );

    assert_eq!(w.usdc.balance(&w.recipient), MAX_FEE);
    assert_eq!(w.usdc.balance(&w.user), BALANCE - MAX_FEE);
    assert_eq!(w.usdc.balance(&w.forwarder.address), 0);
    assert_eq!(w.allowance(), 0);
}

#[test]
fn the_balance_must_cover_the_fee_cap() {
    // The pull takes the whole cap before the refund, so a balance that
    // covers the fee but not the cap fails, and nothing moves.
    let e = Env::default();
    let usdc = new_token(&e);
    let forwarder = FeeForwarderContractClient::new(&e, &e.register(FeeForwarderContract, ()));
    let probe = e.register(MockProbe, ());
    let user = Address::generate(&e);
    let recipient = Address::generate(&e);
    mint(&e, &usdc.address, &user, MAX_FEE - 1);
    let args: Vec<Val> = (
        user.clone(),
        usdc.address.clone(),
        forwarder.address.clone(),
        1u32,
    )
        .into_val(&e);
    e.mock_all_auths();

    let result = forwarder.try_forward(
        &usdc.address,
        &FEE,
        &MAX_FEE,
        &EXPIRATION,
        &probe,
        &Symbol::new(&e, "probe"),
        &args,
        &user,
        &recipient,
    );

    assert!(result.is_err());
    assert_eq!(usdc.balance(&user), MAX_FEE - 1);
    assert_eq!(usdc.balance(&recipient), 0);
    assert_eq!(usdc.allowance(&user, &forwarder.address), 0);
}

#[test]
fn the_forwarder_cannot_be_the_recipient() {
    let w = setup();
    let args = w.probe_args(1);
    w.e.mock_all_auths();

    let result = w.forwarder.try_forward_dynamic(
        &w.usdc.address,
        &FEE,
        &MAX_FEE,
        &EXPIRATION,
        &w.probe,
        &Symbol::new(&w.e, "probe"),
        &args,
        &w.user,
        &w.forwarder.address,
    );

    assert_eq!(
        result.err().unwrap().unwrap(),
        contract_error(INVALID_RECIPIENT)
    );
    assert_eq!(w.usdc.balance(&w.user), BALANCE);
}

#[test]
fn a_failing_target_reverts_the_fee() {
    let w = setup();
    let none = Vec::<Val>::new(&w.e);
    w.e.mock_all_auths();

    let result = w.forwarder.try_forward(
        &w.usdc.address,
        &FEE,
        &MAX_FEE,
        &EXPIRATION,
        &w.probe,
        &Symbol::new(&w.e, "fail"),
        &none,
        &w.user,
        &w.recipient,
    );

    assert_eq!(result.err().unwrap().unwrap(), contract_error(PROBE_FAILED));
    assert_eq!(w.usdc.balance(&w.recipient), 0);
    assert_eq!(w.usdc.balance(&w.user), BALANCE);
}

#[test]
fn a_fee_above_the_cap_is_rejected() {
    let w = setup();
    let args = w.probe_args(1);
    w.e.mock_all_auths();

    let result = w.forwarder.try_forward(
        &w.usdc.address,
        &(MAX_FEE + 1),
        &MAX_FEE,
        &EXPIRATION,
        &w.probe,
        &Symbol::new(&w.e, "probe"),
        &args,
        &w.user,
        &w.recipient,
    );

    assert_eq!(
        result.err().unwrap().unwrap(),
        contract_error(INVALID_FEE_BOUNDS)
    );
}

#[test]
fn a_zero_fee_is_rejected() {
    // OpenZeppelin's bounds check: a forward always pays a fee.
    let w = setup();
    let args = w.probe_args(1);
    w.e.mock_all_auths();

    let result = w.forwarder.try_forward_dynamic(
        &w.usdc.address,
        &0,
        &MAX_FEE,
        &EXPIRATION,
        &w.probe,
        &Symbol::new(&w.e, "probe"),
        &args,
        &w.user,
        &w.recipient,
    );

    assert_eq!(
        result.err().unwrap().unwrap(),
        contract_error(INVALID_FEE_BOUNDS)
    );
}

// ==========================================
// The signed arguments
// ==========================================

#[test]
fn forward_signs_the_target_args() {
    let w = setup();
    let signed = w.probe_args(1);
    w.sign("forward", w.signed_args(&w.probe, "probe", &signed));

    // Different target args no longer match the signature.
    assert_auth_failure(|| {
        w.forward_probe("forward", 2, &w.recipient);
    });

    // The signed args still land.
    w.sign("forward", w.signed_args(&w.probe, "probe", &signed));
    w.forward_probe("forward", 1, &w.recipient);
    assert_eq!(w.last_tag(), 1);
}

#[test]
fn forward_dynamic_leaves_the_target_args_to_the_relayer() {
    let w = setup();
    w.sign("forward_dynamic", w.dynamic_signed_args(&w.probe, "probe"));

    // The relayer refreshes the args after signing, like a price update.
    w.forward_probe("forward_dynamic", 7, &w.recipient);

    assert_eq!(w.last_tag(), 7);
    assert_eq!(w.usdc.balance(&w.recipient), FEE);
}

#[test]
fn forward_dynamic_cannot_change_a_user_authorized_call() {
    // The target's own `require_auth` pins its exact arguments inside the
    // signed tree, whatever the relayer puts in `target_args`.
    let w = setup();
    let e = &w.e;
    let sign_act = |tag: u32| {
        let approve_max = w.approve_args(MAX_FEE);
        let act: Vec<Val> = (w.user.clone(), tag).into_val(e);
        e.mock_auths(&[MockAuth {
            address: &w.user,
            invoke: &MockAuthInvoke {
                contract: &w.forwarder.address,
                fn_name: "forward_dynamic",
                args: w.dynamic_signed_args(&w.probe, "act"),
                sub_invokes: &[
                    MockAuthInvoke {
                        contract: &w.usdc.address,
                        fn_name: "approve",
                        args: approve_max,
                        sub_invokes: &[],
                    },
                    MockAuthInvoke {
                        contract: &w.probe,
                        fn_name: "act",
                        args: act,
                        sub_invokes: &[],
                    },
                ],
            },
        }]);
    };
    let act = |tag: u32| {
        w.forwarder.forward_dynamic(
            &w.usdc.address,
            &FEE,
            &MAX_FEE,
            &EXPIRATION,
            &w.probe,
            &Symbol::new(e, "act"),
            &(w.user.clone(), tag).into_val(e),
            &w.user,
            &w.recipient,
        );
    };

    sign_act(1);
    assert_auth_failure(|| act(2));

    sign_act(1);
    act(1);
    assert_eq!(w.last_tag(), 1);
}

#[test]
fn changing_the_recipient_after_signing_breaks_auth() {
    let w = setup();
    let other = Address::generate(&w.e);
    let args = w.probe_args(1);

    w.sign("forward", w.signed_args(&w.probe, "probe", &args));
    assert_auth_failure(|| {
        w.forward_probe("forward", 1, &other);
    });

    w.sign("forward_dynamic", w.dynamic_signed_args(&w.probe, "probe"));
    assert_auth_failure(|| {
        w.forward_probe("forward_dynamic", 1, &other);
    });

    // The signed recipient is paid.
    w.sign("forward_dynamic", w.dynamic_signed_args(&w.probe, "probe"));
    w.forward_probe("forward_dynamic", 1, &w.recipient);
    assert_eq!(w.usdc.balance(&w.recipient), FEE);
    assert_eq!(w.usdc.balance(&other), 0);
}

#[test]
fn forward_dynamic_binds_every_other_projected_value() {
    let w = setup();
    let e = &w.e;
    let args = w.probe_args(1);
    let submit =
        |fee_token: &Address, max_fee: i128, expiration: u32, target: &Address, func: &str| {
            w.forwarder.forward_dynamic(
                fee_token,
                &FEE,
                &max_fee,
                &expiration,
                target,
                &Symbol::new(e, func),
                &args,
                &w.user,
                &w.recipient,
            );
        };
    let other_token = new_token(e).address;
    mint(e, &other_token, &w.user, BALANCE);
    let other_probe = e.register(MockProbe, ());

    for tamper in 0..5 {
        w.sign("forward_dynamic", w.dynamic_signed_args(&w.probe, "probe"));
        assert_auth_failure(|| match tamper {
            0 => submit(&other_token, MAX_FEE, EXPIRATION, &w.probe, "probe"),
            1 => submit(&w.usdc.address, MAX_FEE + 1, EXPIRATION, &w.probe, "probe"),
            2 => submit(&w.usdc.address, MAX_FEE, EXPIRATION + 1, &w.probe, "probe"),
            3 => submit(&w.usdc.address, MAX_FEE, EXPIRATION, &other_probe, "probe"),
            _ => submit(&w.usdc.address, MAX_FEE, EXPIRATION, &w.probe, "last_tag"),
        });
    }
}

#[test]
fn the_recorded_tree_is_the_forwarder_root_without_the_router() {
    let w = setup();
    let e = &w.e;
    let market = e.register(MockMarket, (w.usdc.address.clone(),));
    let router = e.register(MockRouter, ());
    let keeper = Address::generate(e);
    let order: Vec<Val> = (
        w.user.clone(),
        true,
        0u32,
        10 * MARGIN,
        MARGIN,
        0i128,
        0i128,
        EXPIRATION,
    )
        .into_val(e);
    let calls = Vec::from_array(
        e,
        [Call {
            contract: market.clone(),
            func: Symbol::new(e, "create_order"),
            args: order.clone(),
        }],
    );
    let target_args: Vec<Val> = (
        calls,
        w.user.clone(),
        keeper.clone(),
        Bytes::from_array(e, &[1]),
    )
        .into_val(e);
    e.mock_all_auths();

    w.forwarder.forward_dynamic(
        &w.usdc.address,
        &FEE,
        &MAX_FEE,
        &EXPIRATION,
        &router,
        &Symbol::new(e, "create_and_fill"),
        &target_args,
        &w.user,
        &w.recipient,
    );

    let auths = e.auths();
    assert_eq!(auths.len(), 1);
    let (address, tree) = &auths[0];
    assert_eq!(address, &w.user);
    let expected = AuthorizedInvocation {
        function: AuthorizedFunction::Contract((
            w.forwarder.address.clone(),
            Symbol::new(e, "forward_dynamic"),
            w.dynamic_signed_args(&router, "create_and_fill"),
        )),
        sub_invocations: std::vec![
            AuthorizedInvocation {
                function: AuthorizedFunction::Contract((
                    w.usdc.address.clone(),
                    Symbol::new(e, "approve"),
                    w.approve_args(MAX_FEE),
                )),
                sub_invocations: std::vec![],
            },
            AuthorizedInvocation {
                function: AuthorizedFunction::Contract((
                    market.clone(),
                    Symbol::new(e, "create_order"),
                    order,
                )),
                sub_invocations: std::vec![AuthorizedInvocation {
                    function: AuthorizedFunction::Contract((
                        w.usdc.address.clone(),
                        Symbol::new(e, "transfer"),
                        (w.user.clone(), market.clone(), MARGIN + EXEC_FEE).into_val(e),
                    )),
                    sub_invocations: std::vec![],
                }],
            },
        ],
    };
    assert_eq!(tree, &expected);
    assert_eq!(w.usdc.balance(&keeper), EXEC_FEE);
}

// ==========================================
// Allowance-spending targets
// ==========================================

#[test]
fn transfer_from_and_burn_from_are_refused_on_any_contract() {
    let w = setup();
    let e = &w.e;
    let attacker = Address::generate(e);
    e.mock_all_auths();

    for (target, func) in [
        (&w.usdc.address, "transfer_from"),
        (&w.usdc.address, "burn_from"),
        (&w.probe, "transfer_from"),
        (&w.probe, "burn_from"),
    ] {
        let args: Vec<Val> = (
            w.forwarder.address.clone(),
            w.user.clone(),
            attacker.clone(),
            1i128,
        )
            .into_val(e);
        let refused = w.forwarder.try_forward(
            &w.usdc.address,
            &FEE,
            &MAX_FEE,
            &EXPIRATION,
            target,
            &Symbol::new(e, func),
            &args,
            &attacker,
            &attacker,
        );
        assert_eq!(
            refused.err().unwrap().unwrap(),
            contract_error(TARGET_NOT_ALLOWED)
        );
        let refused = w.forwarder.try_forward_dynamic(
            &w.usdc.address,
            &FEE,
            &MAX_FEE,
            &EXPIRATION,
            target,
            &Symbol::new(e, func),
            &args,
            &attacker,
            &attacker,
        );
        assert_eq!(
            refused.err().unwrap().unwrap(),
            contract_error(TARGET_NOT_ALLOWED)
        );
    }
}

#[test]
fn a_cross_token_drain_is_refused() {
    // The victim holds an allowance to the forwarder in token B. The attacker
    // forwards paying the fee in token A and targets B.transfer_from.
    let w = setup();
    let e = &w.e;
    let victim = &w.user;
    let attacker = Address::generate(e);
    let token_a = new_token(e);
    mint(e, &token_a.address, &attacker, BALANCE);
    e.mock_all_auths();
    w.usdc
        .approve(victim, &w.forwarder.address, &BALANCE, &EXPIRATION);

    let drain: Vec<Val> = (
        w.forwarder.address.clone(),
        victim.clone(),
        attacker.clone(),
        BALANCE,
    )
        .into_val(e);
    let refused = w.forwarder.try_forward_dynamic(
        &token_a.address,
        &FEE,
        &MAX_FEE,
        &EXPIRATION,
        &w.usdc.address,
        &Symbol::new(e, "transfer_from"),
        &drain,
        &attacker,
        &attacker,
    );

    assert_eq!(
        refused.err().unwrap().unwrap(),
        contract_error(TARGET_NOT_ALLOWED)
    );
    assert_eq!(w.usdc.balance(victim), BALANCE);
    assert_eq!(w.usdc.balance(&attacker), 0);
}

#[test]
fn a_standalone_approve_cannot_be_spent_through_a_forward() {
    // A stolen session key signs a plain approve to the forwarder, then a
    // forward in the same token that targets transfer_from or burn_from.
    let w = setup();
    let e = &w.e;
    let victim = &w.user;
    let attacker = Address::generate(e);
    mint(e, &w.usdc.address, &attacker, BALANCE);
    e.mock_all_auths();
    w.usdc
        .approve(victim, &w.forwarder.address, &BALANCE, &EXPIRATION);

    for func in ["transfer_from", "burn_from"] {
        let args: Vec<Val> = if func == "transfer_from" {
            (
                w.forwarder.address.clone(),
                victim.clone(),
                attacker.clone(),
                BALANCE,
            )
                .into_val(e)
        } else {
            (w.forwarder.address.clone(), victim.clone(), BALANCE).into_val(e)
        };
        let refused = w.forwarder.try_forward_dynamic(
            &w.usdc.address,
            &FEE,
            &MAX_FEE,
            &EXPIRATION,
            &w.usdc.address,
            &Symbol::new(e, func),
            &args,
            &attacker,
            &attacker,
        );
        assert_eq!(
            refused.err().unwrap().unwrap(),
            contract_error(TARGET_NOT_ALLOWED)
        );
    }
    assert_eq!(w.usdc.balance(victim), BALANCE);
    assert_eq!(w.usdc.allowance(victim, &w.forwarder.address), BALANCE);
}

#[test]
fn the_oz_example_forwarder_spends_an_allowance_granted_outside_a_forward() {
    // Why the refusal still exists on the v0.9.0 helper. OpenZeppelin #873
    // (issue #875) makes the Eager collection consume the whole approval, so
    // a forward no longer leaves a leftover behind. The example still calls
    // any target, though, so an allowance a user grants the forwarder
    // directly, as a stolen session key could, is spendable by anyone's
    // forward. This forwarder refuses that target (see
    // `a_standalone_approve_cannot_be_spent_through_a_forward`).
    let e = Env::default();
    let token_b = new_token(&e);
    let token_a = new_token(&e);
    let forwarder = e.register(OzExampleForwarder, ());
    let probe = e.register(MockProbe, ());
    let victim = Address::generate(&e);
    let relayer = Address::generate(&e);
    let attacker = Address::generate(&e);
    mint(&e, &token_b.address, &victim, BALANCE);
    mint(&e, &token_a.address, &attacker, BALANCE);
    let client = OzExampleForwarderClient::new(&e, &forwarder);
    e.mock_all_auths();

    // The victim pays 0.4 of a 1 cap in token B: the fix leaves no leftover.
    let probe_args: Vec<Val> = (
        victim.clone(),
        token_b.address.clone(),
        forwarder.clone(),
        1u32,
    )
        .into_val(&e);
    client.forward(
        &token_b.address,
        &FEE,
        &MAX_FEE,
        &EXPIRATION,
        &probe,
        &Symbol::new(&e, "probe"),
        &probe_args,
        &victim,
        &relayer,
    );
    assert_eq!(token_b.allowance(&victim, &forwarder), 0);
    assert_eq!(token_b.balance(&victim), BALANCE - FEE);

    // The victim then approves the forwarder directly, outside any forward.
    token_b.approve(&victim, &forwarder, &MAX_FEE, &EXPIRATION);

    // The attacker pays in token A and targets token B's transfer_from. The
    // example requires the relayer's authorization first, so the attacker
    // relays from a second address of their own.
    let attacker_relayer = Address::generate(&e);
    let drain: Vec<Val> =
        (forwarder.clone(), victim.clone(), attacker.clone(), MAX_FEE).into_val(&e);
    client.forward(
        &token_a.address,
        &FEE,
        &MAX_FEE,
        &EXPIRATION,
        &token_b.address,
        &Symbol::new(&e, "transfer_from"),
        &drain,
        &attacker,
        &attacker_relayer,
    );

    assert_eq!(token_b.balance(&attacker), MAX_FEE);
    assert_eq!(token_b.balance(&victim), BALANCE - FEE - MAX_FEE);
}
