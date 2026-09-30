extern crate std;

use crate::{Call, Config, FeeForwarderContract, FeeForwarderContractClient};
use soroban_sdk::testutils::{
    Address as _, AuthorizedFunction, AuthorizedInvocation, MockAuth, MockAuthInvoke,
};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, token,
    xdr::{ScErrorCode, ScErrorType},
    Address, Bytes, Env, Error, IntoVal, MuxedAddress, Symbol, TryFromVal, Val, Vec,
};

// The stellar-fee-abstraction bounds-check discriminant.
const INVALID_FEE_BOUNDS: u32 = 5003;
// The mock market's fill rejection, in the real market's error range.
const FILL_REJECTED: u32 = 741;

const EXEC_FEE: i128 = 100_000; // 0.01 at 7 decimals, the deployed exec fee
const MARGIN: i128 = 2_000_000_000; // 200
const MAX_FEE: i128 = 10_000_000; // 1
const FEE: i128 = 4_000_000; // 0.4
const EXPIRATION: u32 = 1_000;

// ==========================================
// Mocks
// ==========================================

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MockMarketError {
    FillRejected = 741,
}

#[contracttype]
enum MarketKey {
    Token,
    Next,
    Escrow(Address, u32),
}

/// A market with the real entry-point signatures: `create_order` requires the
/// user's authorization and pulls `margin + exec_fee` into the market, and
/// `execute_order` pays the keeper the exec fee. An empty price rejects the
/// fill, like a stale one.
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
        let escrow = margin + EXEC_FEE;
        token::Client::new(&e, &token).transfer(
            &user,
            MuxedAddress::from(e.current_contract_address()),
            &escrow,
        );
        let id: u32 = e.storage().instance().get(&MarketKey::Next).unwrap_or(0);
        e.storage().instance().set(&MarketKey::Next, &(id + 1));
        e.storage()
            .persistent()
            .set(&MarketKey::Escrow(user, id), &escrow);
        id
    }

    pub fn execute_order(e: Env, keeper: Address, user: Address, id: u32, price: Bytes) -> i128 {
        if price.is_empty() {
            panic_with_error!(&e, MockMarketError::FillRejected);
        }
        let key = MarketKey::Escrow(user, id);
        let _escrow: i128 = e.storage().persistent().get(&key).unwrap();
        e.storage().persistent().remove(&key);
        let token: Address = e.storage().instance().get(&MarketKey::Token).unwrap();
        token::Client::new(&e, &token).transfer(
            &e.current_contract_address(),
            MuxedAddress::from(keeper),
            &EXEC_FEE,
        );
        EXEC_FEE
    }
}

/// The market router's non-fee flows, as generic as the real ones: every
/// call runs on the router's behalf with `invoke_contract`.
#[contract]
struct MockRouter;

#[contractimpl]
impl MockRouter {
    pub fn multicall(e: Env, calls: Vec<Call>) -> Vec<Val> {
        run(&e, &calls)
    }

    pub fn create_and_fill(
        e: Env,
        calls: Vec<Call>,
        user: Address,
        keeper: Address,
        price: Bytes,
    ) -> Vec<Val> {
        let market = calls.get(0).unwrap().contract;
        let mut results = run(&e, &calls);
        let id = u32::try_from_val(&e, &results.get(0).unwrap()).unwrap();
        let payout: i128 = e.invoke_contract(
            &market,
            &Symbol::new(&e, "execute_order"),
            (keeper, user, id, price).into_val(&e),
        );
        results.push_back(payout.into_val(&e));
        results
    }

    pub fn create_and_try_fill(
        e: Env,
        calls: Vec<Call>,
        user: Address,
        keeper: Address,
        price: Bytes,
    ) -> Vec<Val> {
        let market = calls.get(0).unwrap().contract;
        let mut results = run(&e, &calls);
        let id = u32::try_from_val(&e, &results.get(0).unwrap()).unwrap();
        let outcome: Val = match e.try_invoke_contract::<i128, Error>(
            &market,
            &Symbol::new(&e, "execute_order"),
            (keeper, user, id, price).into_val(&e),
        ) {
            Ok(Ok(payout)) => payout.into_val(&e),
            Err(Ok(error)) => error.into(),
            _ => Error::from_type_and_code(ScErrorType::Context, ScErrorCode::InvalidAction).into(),
        };
        results.push_back(outcome);
        results
    }
}

fn run(e: &Env, calls: &Vec<Call>) -> Vec<Val> {
    let mut results = Vec::new(e);
    for call in calls.iter() {
        results.push_back(e.invoke_contract::<Val>(&call.contract, &call.func, call.args.clone()));
    }
    results
}

// ==========================================
// Setup
// ==========================================

struct World {
    e: Env,
    forwarder: FeeForwarderContractClient<'static>,
    router: Address,
    market: Address,
    usdc: token::TokenClient<'static>,
    user: Address,
    recipient: Address,
    keeper: Address,
}

fn setup() -> World {
    let e = Env::default();
    let issuer = Address::generate(&e);
    let usdc = e.register_stellar_asset_contract_v2(issuer).address();
    let market = e.register(MockMarket, (usdc.clone(),));
    let router = e.register(MockRouter, ());
    let recipient = Address::generate(&e);
    let forwarder = e.register(
        FeeForwarderContract,
        (router.clone(), usdc.clone(), recipient.clone()),
    );
    let user = Address::generate(&e);
    e.mock_all_auths();
    token::StellarAssetClient::new(&e, &usdc).mint(&user, &(1_000 * 10_000_000));
    e.set_auths(&[]);
    World {
        forwarder: FeeForwarderContractClient::new(&e, &forwarder),
        router,
        market,
        usdc: token::TokenClient::new(&e, &usdc),
        user,
        recipient,
        keeper: Address::generate(&e),
        e,
    }
}

impl World {
    fn order_args(&self) -> Vec<Val> {
        (
            self.user.clone(),
            true,
            0u32,
            10 * MARGIN,
            MARGIN,
            0i128,
            0i128,
            EXPIRATION,
        )
            .into_val(&self.e)
    }

    fn create_order_calls(&self) -> Vec<Call> {
        Vec::from_array(
            &self.e,
            [Call {
                contract: self.market.clone(),
                func: Symbol::new(&self.e, "create_order"),
                args: self.order_args(),
            }],
        )
    }

    fn signed_prefix(&self, calls: &Vec<Call>, max_fee: i128, expiration: u32) -> Vec<Val> {
        (calls.clone(), max_fee, expiration).into_val(&self.e)
    }

    fn approve_args(&self, amount: i128, expiration: u32) -> Vec<Val> {
        (
            self.user.clone(),
            self.forwarder.address.clone(),
            amount,
            expiration,
        )
            .into_val(&self.e)
    }

    fn escrow_args(&self) -> Vec<Val> {
        (self.user.clone(), self.market.clone(), MARGIN + EXEC_FEE).into_val(&self.e)
    }

    /// Mocks exactly the tree the wallet signs for a create-order batch:
    /// the two fee approves and the order with its escrow, rooted at the
    /// forwarder's signed prefix. The router is not part of it.
    fn sign(&self, fn_name: &str, calls: &Vec<Call>, max_fee: i128, expiration: u32) {
        let e = &self.e;
        let approve_max = self.approve_args(max_fee, expiration);
        let approve_zero = self.approve_args(0, expiration);
        let escrow = self.escrow_args();
        let order = self.order_args();
        let prefix = self.signed_prefix(calls, max_fee, expiration);
        let usdc = self.usdc.address.clone();
        e.mock_auths(&[MockAuth {
            address: &self.user,
            invoke: &MockAuthInvoke {
                contract: &self.forwarder.address,
                fn_name,
                args: prefix,
                sub_invokes: &[
                    MockAuthInvoke {
                        contract: &usdc,
                        fn_name: "approve",
                        args: approve_max,
                        sub_invokes: &[],
                    },
                    MockAuthInvoke {
                        contract: &usdc,
                        fn_name: "approve",
                        args: approve_zero,
                        sub_invokes: &[],
                    },
                    MockAuthInvoke {
                        contract: &self.market,
                        fn_name: "create_order",
                        args: order,
                        sub_invokes: &[MockAuthInvoke {
                            contract: &usdc,
                            fn_name: "transfer",
                            args: escrow,
                            sub_invokes: &[],
                        }],
                    },
                ],
            },
        }]);
    }

    fn allowance(&self) -> i128 {
        self.usdc.allowance(&self.user, &self.forwarder.address)
    }
}

fn contract_error(code: u32) -> Error {
    Error::from_contract_error(code)
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

// ==========================================
// Constructor
// ==========================================

#[test]
#[should_panic(expected = "Error(Contract, #6001)")]
fn constructor_rejects_router_as_fee_token() {
    let e = Env::default();
    let a = Address::generate(&e);
    let b = Address::generate(&e);
    e.register(FeeForwarderContract, (a.clone(), a, b));
}

#[test]
#[should_panic(expected = "Error(Contract, #6001)")]
fn constructor_rejects_router_as_fee_recipient() {
    let e = Env::default();
    let a = Address::generate(&e);
    let b = Address::generate(&e);
    e.register(FeeForwarderContract, (a.clone(), b, a));
}

#[test]
#[should_panic(expected = "Error(Contract, #6001)")]
fn constructor_rejects_fee_token_as_fee_recipient() {
    let e = Env::default();
    let a = Address::generate(&e);
    let b = Address::generate(&e);
    e.register(FeeForwarderContract, (a, b.clone(), b));
}

#[test]
fn get_config_returns_the_fixed_settings() {
    let w = setup();
    assert_eq!(
        w.forwarder.get_config(),
        Config {
            router: w.router.clone(),
            fee_token: w.usdc.address.clone(),
            fee_recipient: w.recipient.clone(),
        }
    );
}

// ==========================================
// The signed tree
// ==========================================

#[test]
fn multicall_with_fee_collects_the_fee_and_wipes_the_allowance() {
    let w = setup();
    let calls = w.create_order_calls();
    w.sign("multicall_with_fee", &calls, MAX_FEE, EXPIRATION);
    let results = w
        .forwarder
        .multicall_with_fee(&calls, &w.user, &MAX_FEE, &EXPIRATION, &FEE);

    assert_eq!(results.len(), 1);
    assert_eq!(
        u32::try_from_val(&w.e, &results.get(0).unwrap()).unwrap(),
        0
    );
    assert_eq!(w.usdc.balance(&w.recipient), FEE);
    assert_eq!(w.usdc.balance(&w.market), MARGIN + EXEC_FEE);
    assert_eq!(w.allowance(), 0);
    assert_eq!(w.usdc.balance(&w.forwarder.address), 0);
}

#[test]
fn the_recorded_tree_is_the_forwarder_call_without_the_router() {
    let w = setup();
    let calls = w.create_order_calls();
    w.e.mock_all_auths();
    w.forwarder
        .multicall_with_fee(&calls, &w.user, &MAX_FEE, &EXPIRATION, &FEE);

    let usdc = w.usdc.address.clone();
    let leaf = |contract: &Address, name: &str, args: Vec<Val>| AuthorizedInvocation {
        function: AuthorizedFunction::Contract((contract.clone(), Symbol::new(&w.e, name), args)),
        sub_invocations: std::vec![],
    };
    let expected = AuthorizedInvocation {
        function: AuthorizedFunction::Contract((
            w.forwarder.address.clone(),
            Symbol::new(&w.e, "multicall_with_fee"),
            w.signed_prefix(&calls, MAX_FEE, EXPIRATION),
        )),
        sub_invocations: std::vec![
            leaf(&usdc, "approve", w.approve_args(MAX_FEE, EXPIRATION)),
            leaf(&usdc, "approve", w.approve_args(0, EXPIRATION)),
            AuthorizedInvocation {
                function: AuthorizedFunction::Contract((
                    w.market.clone(),
                    Symbol::new(&w.e, "create_order"),
                    w.order_args(),
                )),
                sub_invocations: std::vec![leaf(&usdc, "transfer", w.escrow_args())],
            },
        ],
    };
    assert_eq!(w.e.auths(), std::vec![(w.user.clone(), expected)]);
}

#[test]
fn fee_amount_is_outside_the_signature() {
    let w = setup();
    let calls = w.create_order_calls();
    // The same signed tree, submitted with two different fees.
    w.sign("multicall_with_fee", &calls, MAX_FEE, EXPIRATION);
    w.forwarder
        .multicall_with_fee(&calls, &w.user, &MAX_FEE, &EXPIRATION, &1);
    w.sign("multicall_with_fee", &calls, MAX_FEE, EXPIRATION);
    w.forwarder
        .multicall_with_fee(&calls, &w.user, &MAX_FEE, &EXPIRATION, &MAX_FEE);
    assert_eq!(w.usdc.balance(&w.recipient), 1 + MAX_FEE);
    assert_eq!(w.allowance(), 0);
}

#[test]
fn changing_a_signed_value_breaks_auth() {
    let w = setup();
    let calls = w.create_order_calls();

    // A different fee cap.
    w.sign("multicall_with_fee", &calls, MAX_FEE, EXPIRATION);
    assert_auth_failure(|| {
        w.forwarder
            .multicall_with_fee(&calls, &w.user, &(MAX_FEE + 1), &EXPIRATION, &FEE);
    });

    // A different expiration.
    w.sign("multicall_with_fee", &calls, MAX_FEE, EXPIRATION);
    assert_auth_failure(|| {
        w.forwarder
            .multicall_with_fee(&calls, &w.user, &MAX_FEE, &(EXPIRATION + 1), &FEE);
    });

    // A different batch.
    w.sign("multicall_with_fee", &calls, MAX_FEE, EXPIRATION);
    let mut other = calls.clone();
    other.push_back(calls.get(0).unwrap());
    assert_auth_failure(|| {
        w.forwarder
            .multicall_with_fee(&other, &w.user, &MAX_FEE, &EXPIRATION, &FEE);
    });

    assert_eq!(w.usdc.balance(&w.recipient), 0);
}

#[test]
fn a_fee_above_the_cap_is_rejected() {
    let w = setup();
    let calls = w.create_order_calls();
    w.sign("multicall_with_fee", &calls, MAX_FEE, EXPIRATION);
    let result =
        w.forwarder
            .try_multicall_with_fee(&calls, &w.user, &MAX_FEE, &EXPIRATION, &(MAX_FEE + 1));
    assert_eq!(
        result.err().unwrap().unwrap(),
        contract_error(INVALID_FEE_BOUNDS)
    );
    assert_eq!(w.usdc.balance(&w.recipient), 0);
}

#[test]
fn a_zero_fee_skips_the_collection() {
    let w = setup();
    let calls = w.create_order_calls();
    // Without a fee, the signed tree has no approves.
    let order = w.order_args();
    let escrow = w.escrow_args();
    let usdc = w.usdc.address.clone();
    w.e.mock_auths(&[MockAuth {
        address: &w.user,
        invoke: &MockAuthInvoke {
            contract: &w.forwarder.address,
            fn_name: "multicall_with_fee",
            args: w.signed_prefix(&calls, MAX_FEE, EXPIRATION),
            sub_invokes: &[MockAuthInvoke {
                contract: &w.market,
                fn_name: "create_order",
                args: order,
                sub_invokes: &[MockAuthInvoke {
                    contract: &usdc,
                    fn_name: "transfer",
                    args: escrow,
                    sub_invokes: &[],
                }],
            }],
        },
    }]);
    w.forwarder
        .multicall_with_fee(&calls, &w.user, &MAX_FEE, &EXPIRATION, &0);
    assert_eq!(w.usdc.balance(&w.recipient), 0);
    assert_eq!(w.usdc.balance(&w.market), MARGIN + EXEC_FEE);
}

// ==========================================
// Fill flows
// ==========================================

#[test]
fn create_and_fill_with_fee_fills_through_the_router() {
    let w = setup();
    let calls = w.create_order_calls();
    w.sign("create_and_fill_with_fee", &calls, MAX_FEE, EXPIRATION);
    let price = Bytes::from_array(&w.e, &[1u8; 8]);
    let results = w.forwarder.create_and_fill_with_fee(
        &calls,
        &w.user,
        &MAX_FEE,
        &EXPIRATION,
        &FEE,
        &w.keeper,
        &price,
    );
    assert_eq!(results.len(), 2);
    assert_eq!(
        i128::try_from_val(&w.e, &results.get(1).unwrap()).unwrap(),
        EXEC_FEE
    );
    assert_eq!(w.usdc.balance(&w.keeper), EXEC_FEE);
    assert_eq!(w.usdc.balance(&w.recipient), FEE);
    assert_eq!(w.allowance(), 0);
}

#[test]
fn a_failing_strict_fill_unwinds_the_fee() {
    let w = setup();
    let calls = w.create_order_calls();
    w.sign("create_and_fill_with_fee", &calls, MAX_FEE, EXPIRATION);
    let result = w.forwarder.try_create_and_fill_with_fee(
        &calls,
        &w.user,
        &MAX_FEE,
        &EXPIRATION,
        &FEE,
        &w.keeper,
        &Bytes::new(&w.e),
    );
    assert_eq!(
        result.err().unwrap().unwrap(),
        contract_error(FILL_REJECTED)
    );
    assert_eq!(w.usdc.balance(&w.recipient), 0);
    assert_eq!(w.usdc.balance(&w.market), 0);
}

#[test]
fn a_resting_isolated_fill_keeps_the_fee() {
    let w = setup();
    let calls = w.create_order_calls();
    w.sign("create_and_try_fill_with_fee", &calls, MAX_FEE, EXPIRATION);
    let results = w.forwarder.create_and_try_fill_with_fee(
        &calls,
        &w.user,
        &MAX_FEE,
        &EXPIRATION,
        &FEE,
        &w.keeper,
        &Bytes::new(&w.e),
    );
    assert_eq!(results.len(), 2);
    assert_eq!(
        Error::try_from_val(&w.e, &results.get(1).unwrap()).unwrap(),
        contract_error(FILL_REJECTED)
    );
    assert_eq!(w.usdc.balance(&w.recipient), FEE);
    assert_eq!(w.usdc.balance(&w.market), MARGIN + EXEC_FEE);
    assert_eq!(w.allowance(), 0);
}

// ==========================================
// Allowance safety
// ==========================================

/// An allowance to the forwarder cannot be spent through the router's
/// generic calls: a contract is authorized only for the calls it makes
/// directly, and there the router, not the forwarder, calls `transfer_from`.
#[test]
fn an_allowance_to_the_forwarder_cannot_be_spent_through_the_router() {
    let w = setup();
    let e = &w.e;
    let usdc = w.usdc.address.clone();
    let attacker = Address::generate(e);
    let amount = 100 * 10_000_000;

    // The user leaves an allowance to the forwarder.
    let approve_args: Vec<Val> = (
        w.user.clone(),
        w.forwarder.address.clone(),
        amount,
        EXPIRATION,
    )
        .into_val(e);
    e.mock_auths(&[MockAuth {
        address: &w.user,
        invoke: &MockAuthInvoke {
            contract: &usdc,
            fn_name: "approve",
            args: approve_args,
            sub_invokes: &[],
        },
    }]);
    w.usdc
        .approve(&w.user, &w.forwarder.address, &amount, &EXPIRATION);
    assert_eq!(w.allowance(), amount);

    let steal = Vec::from_array(
        e,
        [Call {
            contract: usdc.clone(),
            func: Symbol::new(e, "transfer_from"),
            args: (
                w.forwarder.address.clone(),
                w.user.clone(),
                attacker.clone(),
                amount,
            )
                .into_val(e),
        }],
    );

    // Straight through the router.
    e.set_auths(&[]);
    let router = MockRouterClient::new(e, &w.router);
    assert_auth_failure(|| {
        router.multicall(&steal);
    });

    // Through the forwarder, which forwards the batch to the router: the
    // attacker signs their own call, and the router still makes the transfer.
    e.mock_auths(&[MockAuth {
        address: &attacker,
        invoke: &MockAuthInvoke {
            contract: &w.forwarder.address,
            fn_name: "multicall_with_fee",
            args: (steal.clone(), 0i128, EXPIRATION).into_val(e),
            sub_invokes: &[],
        },
    }]);
    assert_auth_failure(|| {
        w.forwarder
            .multicall_with_fee(&steal, &attacker, &0, &EXPIRATION, &0);
    });

    assert_eq!(w.allowance(), amount);
    assert_eq!(w.usdc.balance(&attacker), 0);
}
