extern crate std;
use crate::{Call, RouterContract, RouterContractClient};
use soroban_sdk::testutils::{Address as _, AuthorizedFunction, Ledger};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, token,
    vec, Address, Bytes, Env, IntoVal, Symbol, TryFromVal, Val, Vec,
};

mod market_wasm {
    soroban_sdk::contractimport!(file = "testdata/market.wasm");
}

const SCALAR_18: i128 = 1_000_000_000_000_000_000;

/// Price scale of every price in this suite: `10^18`, the scale a production
/// Chainlink Data Streams V3 report carries. Every price derives from this
/// constant. A fixture at any other scale exercises a base-size granularity
/// the deployment never sees.
const PRICE_SCALAR: i128 = SCALAR_18;

// Market error discriminants asserted by the flow tests.
const PRICE_BOUND_EXCEEDED: u32 = 741;
const VAULT_ORDER_LOCKED: u32 = 751;
const ADL_NOT_TRIGGERED: u32 = 770;
const ADL_OVERSHOOT: u32 = 771;

// Market OrderKind / VaultOrderKind discriminants exercised by the flow tests.
const ORDER_KIND_MARKET_INCREASE: u32 = 0;
const ORDER_KIND_LIMIT_DECREASE: u32 = 4;
const ORDER_KIND_STOP_DECREASE: u32 = 5;
const VAULT_ORDER_KIND_REDEEM: u32 = 1;

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetError {
    Boom = 7,
}

// Batch target: returns values, writes storage, and fails on demand.
#[contract]
struct Target;

#[contractimpl]
impl Target {
    pub fn add(_e: Env, a: i128, b: i128) -> i128 {
        a + b
    }

    pub fn put(e: Env, value: i128) {
        e.storage().instance().set(&symbol_short!("v"), &value);
    }

    pub fn stored(e: Env) -> i128 {
        e.storage().instance().get(&symbol_short!("v")).unwrap_or(0)
    }

    // Writes before panicking, so rollback of the write is observable.
    pub fn boom(e: Env) -> i128 {
        e.storage().instance().set(&symbol_short!("v"), &99_i128);
        panic_with_error!(&e, TargetError::Boom)
    }
}

fn setup() -> (Env, RouterContractClient<'static>, Address) {
    let e = Env::default();
    let router = e.register(RouterContract, ());
    let target = e.register(Target, ());
    let client = RouterContractClient::new(&e, &router);
    (e, client, target)
}

fn call(e: &Env, target: &Address, func: &str, args: Vec<Val>) -> Call {
    Call {
        contract: target.clone(),
        func: Symbol::new(e, func),
        args,
    }
}

/// A `create_order` [`Call`] against `market`, mirroring the market
/// contract's positional `create_order` signature.
#[allow(clippy::too_many_arguments)]
fn create_order_call(
    e: &Env,
    market: &Address,
    user: &Address,
    is_long: bool,
    kind: u32,
    notional: i128,
    margin: i128,
    trigger_price: i128,
    price_bound: i128,
    expiration: u32,
) -> Call {
    call(
        e,
        market,
        "create_order",
        vec![
            e,
            user.into_val(e),
            is_long.into_val(e),
            kind.into_val(e),
            notional.into_val(e),
            margin.into_val(e),
            trigger_price.into_val(e),
            price_bound.into_val(e),
            expiration.into_val(e),
        ],
    )
}

/// A single-element create-and-fill batch: just the market `create_order`.
fn open_batch(
    e: &Env,
    market: &Address,
    user: &Address,
    is_long: bool,
    notional: i128,
    margin: i128,
    price_bound: i128,
) -> Vec<Call> {
    vec![
        e,
        create_order_call(
            e,
            market,
            user,
            is_long,
            ORDER_KIND_MARKET_INCREASE,
            notional,
            margin,
            0,
            price_bound,
            1000,
        ),
    ]
}

fn as_i128(e: &Env, value: &Val) -> i128 {
    i128::try_from_val(e, value).unwrap()
}

/// Decode a raw `multicall_try` outcome as a contract error and return its code.
fn contract_error(e: &Env, outcome: &Val) -> u32 {
    let error = soroban_sdk::Error::try_from_val(e, outcome).unwrap();
    assert!(error.is_type(soroban_sdk::xdr::ScErrorType::Contract));
    error.get_code()
}

/// The last element of a create-and-fill result: the appended fill outcome.
fn fill_outcome(results: &Vec<Val>) -> Val {
    results.get(results.len() - 1).unwrap()
}

/// The order id from a create-and-fill result (`results[0]`).
fn fill_order_id(e: &Env, results: &Vec<Val>) -> u32 {
    u32::try_from_val(e, &results.get(0).unwrap()).unwrap()
}

/// True when the appended fill outcome is an `Error` (the fill rests).
fn fill_rested(e: &Env, results: &Vec<Val>) -> bool {
    soroban_sdk::Error::try_from_val(e, &fill_outcome(results)).is_ok()
}

#[test]
fn multicall_executes_in_order_and_returns_values() {
    let (e, router, target) = setup();
    let results = router.multicall(&vec![
        &e,
        call(
            &e,
            &target,
            "add",
            vec![&e, 1_i128.into_val(&e), 2_i128.into_val(&e)],
        ),
        call(&e, &target, "put", vec![&e, 5_i128.into_val(&e)]),
        call(&e, &target, "stored", vec![&e]),
    ]);
    assert_eq!(results.len(), 3);
    assert_eq!(as_i128(&e, &results.get(0).unwrap()), 3);
    assert_eq!(as_i128(&e, &results.get(2).unwrap()), 5);
}

// Strict semantics: the failing second call reverts the first call's write.
#[test]
fn multicall_failure_reverts_the_whole_batch() {
    let (e, router, target) = setup();
    let result = router.try_multicall(&vec![
        &e,
        call(&e, &target, "put", vec![&e, 5_i128.into_val(&e)]),
        call(&e, &target, "boom", vec![&e]),
    ]);
    assert!(result.is_err());
    assert_eq!(TargetClient::new(&e, &target).stored(), 0);
}

// Try semantics: the failed call reports its raw error and rolls back only
// its own write; the calls around it land.
#[test]
fn multicall_try_isolates_failures_and_reports_codes() {
    let (e, router, target) = setup();
    let outcomes = router.multicall_try(&vec![
        &e,
        call(
            &e,
            &target,
            "add",
            vec![&e, 1_i128.into_val(&e), 2_i128.into_val(&e)],
        ),
        call(&e, &target, "boom", vec![&e]),
        call(&e, &target, "put", vec![&e, 5_i128.into_val(&e)]),
    ]);
    assert_eq!(outcomes.len(), 3);

    let first = outcomes.get(0).unwrap();
    assert_eq!(as_i128(&e, &first), 3);

    let second = outcomes.get(1).unwrap();
    assert_eq!(contract_error(&e, &second), 7);

    let third = outcomes.get(2).unwrap();
    assert!(third.is_void()); // put returns unit
                              // boom's write rolled back; put's write survived.
    assert_eq!(TargetClient::new(&e, &target).stored(), 5);
}

#[test]
fn multicall_try_passes_untyped_failures_through_raw() {
    let (e, router, target) = setup();
    let outcomes = router.multicall_try(&vec![&e, call(&e, &target, "nope", vec![&e])]);
    let outcome = outcomes.get(0).unwrap();
    // A missing entry point surfaces as a raw host error, not a contract code.
    let error = soroban_sdk::Error::try_from_val(&e, &outcome).unwrap();
    assert!(!error.is_type(soroban_sdk::xdr::ScErrorType::Contract));
}

// --- flow tests against the compiled market WASM ---

/// Mirrors the market contract's `PriceData`. Same XDR encoding on-chain.
#[contracttype]
#[derive(Clone)]
pub struct PriceData {
    pub bid: i128,
    pub ask: i128,
    pub publish_time: u64,
}

// Returns the constructor-set flat price for any payload; publish_time = now.
#[contract]
struct MockVerifier;

#[contractimpl]
impl MockVerifier {
    pub fn __constructor(e: Env, price: i128) {
        e.storage().instance().set(&symbol_short!("price"), &price);
    }

    pub fn verify_price(
        e: Env,
        _report: Bytes,
        _feed_id: soroban_sdk::BytesN<32>,
        _protective: bool,
    ) -> PriceData {
        let price: i128 = e.storage().instance().get(&symbol_short!("price")).unwrap();
        PriceData {
            bid: price,
            ask: price,
            publish_time: e.ledger().timestamp(),
        }
    }

    pub fn set_price(e: Env, price: i128) {
        e.storage().instance().set(&symbol_short!("price"), &price);
    }
}

// Reports a zero protocol fee rate.
#[contract]
struct MockTreasury;

#[contractimpl]
impl MockTreasury {
    pub fn get_rate(_e: Env) -> i128 {
        0
    }
}

// Reports the constructor-set balance; `strategy_withdraw` moves real tokens
// from the vault's pre-funded balance, other mutations are out of scope.
// `net_pnl` is accepted and ignored: the mock shares 1:1 regardless of mark.
#[contract]
struct MockVault;

#[contractimpl]
impl MockVault {
    pub fn __constructor(e: Env, token: Address, total: i128) {
        e.storage().instance().set(&symbol_short!("token"), &token);
        e.storage().instance().set(&symbol_short!("total"), &total);
    }

    // Set once the strategy (market contract) address is known, resolving
    // the vault/market construction cycle.
    pub fn set_strategy(e: Env, strategy: Address) {
        e.storage()
            .instance()
            .set(&symbol_short!("strat"), &strategy);
    }

    pub fn total_assets(e: Env) -> i128 {
        e.storage().instance().get(&symbol_short!("total")).unwrap()
    }

    pub fn strategy_deposit(
        _e: Env,
        assets: i128,
        _receiver: Address,
        _from: Address,
        _net_pnl: i128,
    ) -> i128 {
        assets
    }

    pub fn strategy_redeem(
        _e: Env,
        shares: i128,
        _receiver: Address,
        _owner: Address,
        _net_pnl: i128,
    ) -> i128 {
        shares
    }

    pub fn preview_redeem(_e: Env, shares: i128, _net_pnl: i128) -> i128 {
        shares
    }

    pub fn balance(_e: Env, _account: Address) -> i128 {
        0
    }

    pub fn transfer(_e: Env, _from: Address, _to: Address, _amount: i128) {}

    pub fn strategy_withdraw(e: Env, amount: i128) {
        let asset: Address = e.storage().instance().get(&symbol_short!("token")).unwrap();
        let strategy: Address = e.storage().instance().get(&symbol_short!("strat")).unwrap();
        token::Client::new(&e, &asset).transfer(&e.current_contract_address(), &strategy, &amount);
    }
}

fn flow_config() -> market_wasm::Config {
    market_wasm::Config {
        keeper_rate: SCALAR_18 / 10, // 10%
        min_position_notional: 10_000_000,
        max_position_notional: 1_000_000_000_000,
        max_open_interest: 10_000_000_000_000,
        min_order_notional: 1_000_000,
        min_order_margin: 1_000_000,
        exec_fee: 0,
        fee_dom: SCALAR_18 / 200,           // 0.5%
        fee_non_dom: 3 * SCALAR_18 / 1000,  // 0.3%
        impact_scalar: 100_000_000_000_000, // fee rate = notional / $10M, capped at 10%
        max_util_open: 8 * SCALAR_18 / 10,
        max_util_withdraw: 9 * SCALAR_18 / 10,
        init_margin: SCALAR_18 / 10,
        maintenance_margin: SCALAR_18 / 20,
        liq_fee: SCALAR_18 / 100,
        notional_lock: 30,
        target_util: SCALAR_18 / 2,
        borrow_rate: 0,
        increased_borrow_rate: 0,
        funding_increase: 0,
        funding_decrease: 0,
        threshold_stable_funding: 0,
        threshold_decrease_funding: 0,
        funding_min: 0,
        funding_max: 0,
        adl_max_pnl: SCALAR_18 / 2,
        adl_clear_target: 4 * SCALAR_18 / 10,
        max_pnl_trader: 9 * SCALAR_18 / 10,
        max_pnl_withdraw: 15 * SCALAR_18 / 100,
        redeem_lock: 0,
        deposit_fee: SCALAR_18 / 1000,
        redeem_fee: SCALAR_18 / 1000,
        min_deposit: 1_000_000,
        max_vault_balance: 10_000_000_000_000,
    }
}

struct FlowSetup {
    e: Env,
    router: Address,
    market: Address,
    token: Address,
    user: Address,
    keeper: Address,
    verifier: Address,
}

/// A live market at a flat $10 mark, priced at [`PRICE_SCALAR`], with a deep
/// mock vault. The user holds 100.0 of the 7-dec settlement token.
fn flow_setup(config: market_wasm::Config) -> FlowSetup {
    let e = Env::default();
    // The live network caps a transaction at 400M instructions, the one limit
    // `Env::default()` does not already enforce. Memory stays unpinned here:
    // the invocation limits enforce the network's 40 MiB, and the budget's
    // ceiling would also bound the test harness's own shadow work.
    e.cost_estimate()
        .budget()
        .reset_limits(400_000_000, u64::MAX);
    e.mock_all_auths_allowing_non_root_auth();
    let router = e.register(RouterContract, ());
    let owner = Address::generate(&e);
    let treasury = e.register(MockTreasury, ());
    let token_admin = Address::generate(&e);
    let token = e.register_stellar_asset_contract_v2(token_admin);
    let vault = e.register(MockVault, (token.address(), 1000_0000000_i128));
    let verifier = e.register(MockVerifier, (10 * PRICE_SCALAR,));
    let market = e.register(
        market_wasm::WASM,
        (
            owner,
            token.address(),
            vault.clone(),
            verifier.clone(),
            treasury,
            soroban_sdk::BytesN::from_array(&e, &{
                let mut id = [0u8; 32];
                id[1] = 0x03;
                id[31] = 1;
                id
            }),
            config,
        ),
    );
    MockVaultClient::new(&e, &vault).set_strategy(&market);
    let user = Address::generate(&e);
    let keeper = Address::generate(&e);
    let asset_admin = token::StellarAssetClient::new(&e, &token.address());
    asset_admin.mint(&user, &100_0000000);
    asset_admin.mint(&vault, &1000_0000000);
    FlowSetup {
        e: e.clone(),
        router,
        market,
        token: token.address(),
        user,
        keeper,
        verifier,
    }
}

// A market increase created and filled in one call: the trade fee is
// 0.5% * 50 = 0.25 base plus the size-quadratic 50^2 / 10_000_000 = 0.00025
// impact, and the keeper takes the 10% cut: 0.025025.
#[test]
fn create_and_fill_opens_and_pays_keeper() {
    let setup = flow_setup(flow_config());
    let e = &setup.e;
    let router = RouterContractClient::new(e, &setup.router);

    let results = router.create_and_fill(
        &open_batch(
            e,
            &setup.market,
            &setup.user,
            true,
            50_0000000,
            10_0000000,
            0,
        ),
        &setup.user,
        &setup.keeper,
        &Bytes::new(e),
    );
    // One create call plus the appended fill payout: results[0] is the order
    // id, the last element is the payout.
    assert_eq!(results.len(), 2);
    assert_eq!(fill_order_id(e, &results), 1);
    assert_eq!(as_i128(e, &fill_outcome(&results)), 250_250);
    assert_eq!(
        token::Client::new(e, &setup.token).balance(&setup.keeper),
        250_250
    );

    let position = market_wasm::Client::new(e, &setup.market).get_position(&setup.user, &true);
    assert_eq!(position.notional, 50_0000000);
}

// A market increase created and filled while two resting decrease triggers
// (a limit take-profit and a stop loss) ride the same batch: only the first
// call fills, and the two triggers rest for a later keeper fill.
#[test]
fn create_and_fill_rests_trailing_triggers() {
    let setup = flow_setup(flow_config());
    let e = &setup.e;
    let router = RouterContractClient::new(e, &setup.router);
    let market = market_wasm::Client::new(e, &setup.market);

    let batch = vec![
        e,
        create_order_call(
            e,
            &setup.market,
            &setup.user,
            true,
            ORDER_KIND_MARKET_INCREASE,
            50_0000000,
            10_0000000,
            0,
            0,
            1000,
        ),
        // Take-profit: a long limit decrease that fires above the $10 mark.
        create_order_call(
            e,
            &setup.market,
            &setup.user,
            true,
            ORDER_KIND_LIMIT_DECREASE,
            50_0000000,
            0,
            12 * PRICE_SCALAR,
            0,
            1000,
        ),
        // Stop loss: a long stop decrease that fires below the $10 mark.
        create_order_call(
            e,
            &setup.market,
            &setup.user,
            true,
            ORDER_KIND_STOP_DECREASE,
            50_0000000,
            0,
            8 * PRICE_SCALAR,
            0,
            1000,
        ),
    ];

    let results = router.create_and_fill(&batch, &setup.user, &setup.keeper, &Bytes::new(e));
    // Three creates plus the appended fill payout.
    assert_eq!(results.len(), 4);
    assert_eq!(fill_order_id(e, &results), 1);
    assert_eq!(as_i128(e, &fill_outcome(&results)), 250_250); // identical to the lone-open fill

    // The market open filled; the position holds the opened size.
    let position = market.get_position(&setup.user, &true);
    assert_eq!(position.notional, 50_0000000);

    // Both triggers rest for later: ids 2 and 3 (the market open took id 1).
    assert_eq!(market.get_order(&setup.user, &2).notional, 50_0000000);
    assert_eq!(market.get_order(&setup.user, &3).notional, 50_0000000);
}

// Empty calls has no first order to fill: the invocation traps.
#[test]
#[should_panic]
fn create_and_fill_traps_on_empty_calls() {
    let setup = flow_setup(flow_config());
    let e = &setup.e;
    let router = RouterContractClient::new(e, &setup.router);

    router.create_and_fill(&Vec::new(e), &setup.user, &setup.keeper, &Bytes::new(e));
}

// A first call that does not return a u32 order id traps on the id conversion.
#[test]
#[should_panic]
fn create_and_fill_traps_when_first_call_is_not_a_create() {
    let setup = flow_setup(flow_config());
    let e = &setup.e;
    let router = RouterContractClient::new(e, &setup.router);

    // `get_position` returns a struct, not a u32 id.
    let batch = vec![
        e,
        call(
            e,
            &setup.market,
            "get_position",
            vec![e, setup.user.into_val(e), true.into_val(e)],
        ),
    ];
    router.create_and_fill(&batch, &setup.user, &setup.keeper, &Bytes::new(e));
}

// Fill-or-kill: a long fill bound below the $10 ask fails the fill leg and
// unwinds the creation; nothing rests.
#[test]
fn create_and_fill_failure_leaves_nothing_resting() {
    let setup = flow_setup(flow_config());
    let e = &setup.e;
    let router = RouterContractClient::new(e, &setup.router);

    let result = router.try_create_and_fill(
        &open_batch(
            e,
            &setup.market,
            &setup.user,
            true,
            50_0000000,
            10_0000000,
            9 * PRICE_SCALAR,
        ),
        &setup.user,
        &setup.keeper,
        &Bytes::new(e),
    );
    assert!(result.is_err());
    let market = market_wasm::Client::new(e, &setup.market);
    assert!(market.try_get_order(&setup.user, &1).is_err());
    let position = market.get_position(&setup.user, &true);
    assert_eq!(position.notional, 0);
}

// The try variant fills like `create_and_fill` when the fill succeeds: the
// appended outcome is the payout, not an error, and the position opens.
#[test]
fn create_and_try_fill_fills_and_pays_keeper() {
    let setup = flow_setup(flow_config());
    let e = &setup.e;
    let router = RouterContractClient::new(e, &setup.router);

    let results = router.create_and_try_fill(
        &open_batch(
            e,
            &setup.market,
            &setup.user,
            true,
            50_0000000,
            10_0000000,
            0,
        ),
        &setup.user,
        &setup.keeper,
        &Bytes::new(e),
    );
    // One create plus the appended fill outcome, which is the payout.
    assert_eq!(results.len(), 2);
    assert!(!fill_rested(e, &results));
    assert_eq!(fill_order_id(e, &results), 1);
    assert_eq!(as_i128(e, &fill_outcome(&results)), 250_250);
    assert_eq!(
        token::Client::new(e, &setup.token).balance(&setup.keeper),
        250_250
    );

    let position = market_wasm::Client::new(e, &setup.market).get_position(&setup.user, &true);
    assert_eq!(position.notional, 50_0000000);
}

// The try variant reports the bound violation and leaves the order resting
// for a later keeper fill.
#[test]
fn create_and_try_fill_rests_on_fill_failure() {
    let setup = flow_setup(flow_config());
    let e = &setup.e;
    let router = RouterContractClient::new(e, &setup.router);

    let results = router.create_and_try_fill(
        &open_batch(
            e,
            &setup.market,
            &setup.user,
            true,
            50_0000000,
            10_0000000,
            9 * PRICE_SCALAR,
        ),
        &setup.user,
        &setup.keeper,
        &Bytes::new(e),
    );
    // One create plus the appended fill outcome, which rests as an Error.
    assert_eq!(results.len(), 2);
    assert!(fill_rested(e, &results));
    assert_eq!(
        contract_error(e, &fill_outcome(&results)),
        PRICE_BOUND_EXCEEDED
    );
    let id = fill_order_id(e, &results);

    let market = market_wasm::Client::new(e, &setup.market);
    let order = market.get_order(&setup.user, &id);
    assert_eq!(order.notional, 50_0000000, "the order rests");
}

// The try variant leaves the whole batch resting when the fill fails: the
// market open and its trailing trigger both rest.
#[test]
fn create_and_try_fill_rests_whole_batch_on_fill_failure() {
    let setup = flow_setup(flow_config());
    let e = &setup.e;
    let router = RouterContractClient::new(e, &setup.router);

    let batch = vec![
        e,
        // Market open bound $9 under the $10 ask, so its fill fails.
        create_order_call(
            e,
            &setup.market,
            &setup.user,
            true,
            ORDER_KIND_MARKET_INCREASE,
            50_0000000,
            10_0000000,
            0,
            9 * PRICE_SCALAR,
            1000,
        ),
        create_order_call(
            e,
            &setup.market,
            &setup.user,
            true,
            ORDER_KIND_LIMIT_DECREASE,
            50_0000000,
            0,
            12 * PRICE_SCALAR,
            0,
            1000,
        ),
    ];

    let results = router.create_and_try_fill(&batch, &setup.user, &setup.keeper, &Bytes::new(e));
    // Two creates plus the appended fill outcome, which rests as an Error.
    assert_eq!(results.len(), 3);
    assert!(fill_rested(e, &results));
    assert_eq!(
        contract_error(e, &fill_outcome(&results)),
        PRICE_BOUND_EXCEEDED
    );
    assert_eq!(fill_order_id(e, &results), 1); // the market open kept its id

    // Both creates rest: the failed fill isolates only the fill leg.
    let market = market_wasm::Client::new(e, &setup.market);
    assert_eq!(market.get_order(&setup.user, &1).notional, 50_0000000);
    assert_eq!(market.get_order(&setup.user, &2).notional, 50_0000000);
    assert_eq!(market.get_position(&setup.user, &true).notional, 0);
}

// An ADL sweep expressed as a `multicall_try` batch of `execute_adl` calls
// over an armed market: the long enters 50.0 at $10 (5 base tokens) and the
// mark jumps to $111, so the side pends 505.0 against the armed trigger (50%
// of half the 1000.0 mock vault = 250.0). The first $25 close lands (side
// pnl falls to 252.5, above the 200.0 clear allowance) and pays the keeper
// 10% of the 0.075 improving fee. The second $25 target runs as a full
// close, lands the side at zero under the allowance, and is recorded as
// AdlOvershoot (771). The third targets the unarmed short side and reports
// AdlNotTriggered (770). The fourth repeats the overshoot; every failure
// stays isolated and the batch runs to the end.
#[test]
fn adl_sweep_via_multicall_try_isolates_failed_closes() {
    let setup = flow_setup(flow_config());
    let e = &setup.e;
    let router = RouterContractClient::new(e, &setup.router);

    router.create_and_fill(
        &open_batch(
            e,
            &setup.market,
            &setup.user,
            true,
            50_0000000,
            10_0000000,
            0,
        ),
        &setup.user, // user
        &setup.user, // self-keeper
        &Bytes::new(e),
    );
    // Let the fresh position's 30s decrease lock expire: ADL honors it.
    e.ledger().with_mut(|ledger| ledger.timestamp += 31);
    MockVerifierClient::new(e, &setup.verifier).set_price(&(111 * PRICE_SCALAR));

    let market = market_wasm::Client::new(e, &setup.market);
    market.update_adl_state(&Bytes::new(e));

    let adl_close = |is_long: bool| {
        call(
            e,
            &setup.market,
            "execute_adl",
            vec![
                e,
                setup.keeper.into_val(e),
                setup.user.into_val(e),
                is_long.into_val(e),
                25_0000000_i128.into_val(e),
                Bytes::new(e).into_val(e),
            ],
        )
    };
    let outcomes = router.multicall_try(&vec![
        e,
        adl_close(true),
        adl_close(true),
        adl_close(false),
        adl_close(true),
    ]);

    assert_eq!(outcomes.len(), 4);
    let first = outcomes.get(0).unwrap();
    assert_eq!(as_i128(e, &first), 75_062); // 10% of ceil(0.3% * 25.0) + the 625 impact
    let second = outcomes.get(1).unwrap();
    assert_eq!(contract_error(e, &second), ADL_OVERSHOOT); // the full close overshoots
    let third = outcomes.get(2).unwrap();
    assert_eq!(contract_error(e, &third), ADL_NOT_TRIGGERED); // the short side is unarmed
    let fourth = outcomes.get(3).unwrap();
    assert_eq!(contract_error(e, &fourth), ADL_OVERSHOOT); // the retried overshoot stays isolated

    // The overshoots reverted: the survivor holds the post-first-close size.
    let position = market.get_position(&setup.user, &true);
    assert_eq!(position.notional, 25_0000000);
}

// The redeem cooldown outlives the price-time gate: a direct fill one ledger
// after creation clears the gate but still reports the lock.
#[test]
fn vault_redeem_flow_honors_the_lock_after_the_price_gate() {
    let setup = flow_setup(market_wasm::Config {
        redeem_lock: 50,
        ..flow_config()
    });
    let e = &setup.e;
    let market = market_wasm::Client::new(e, &setup.market);

    let id = market.create_vault_order(&setup.user, &VAULT_ORDER_KIND_REDEEM, &5_0000000, &0);
    e.ledger().with_mut(|ledger| ledger.timestamp += 1);

    let result = market.try_execute_vault_order(&setup.keeper, &setup.user, &id, &Bytes::new(e));
    if let Err(Ok(error)) = result {
        assert_eq!(crate::error_code(error), VAULT_ORDER_LOCKED);
    } else {
        panic!("expected a typed VaultOrderLocked failure");
    }
    let order = market.get_vault_order(&setup.user, &id);
    assert_eq!(order.amount, 5_0000000, "the order rests");
}

// --- forwarded fills: fee-forwarder -> this router -> the market WASM ---

/// The relay fee cap and the fee the relay sets after signing, 7-dec.
const FORWARD_MAX_FEE: i128 = 1_0000000;
const FORWARD_FEE: i128 = 25_000;
const FORWARD_EXPIRATION: u32 = 1000;

/// `target_args` for the router's create-and-fill functions: `[calls, user,
/// keeper, price]`, in the router's argument order.
fn fill_target_args(e: &Env, calls: &Vec<Call>, user: &Address, keeper: &Address) -> Vec<Val> {
    vec![
        e,
        calls.into_val(e),
        user.into_val(e),
        keeper.into_val(e),
        Bytes::new(e).into_val(e),
    ]
}

// The fee forwarder collects the relay fee and calls this router, which runs
// the batch and fills it on the market. The fee reaches the recipient, the
// unused cap is refunded, no allowance or balance stays with the forwarder,
// and the fill matches the fee-free create_and_fill case. The user's signed
// tree is the forwarder root with its projection, the fee approval, and the
// market's create_order with its escrow transfer; the router is not in it.
#[test]
fn forward_unsafe_through_router_fills_on_the_market() {
    let setup = flow_setup(flow_config());
    let e = &setup.e;
    let forwarder_id = e.register(fee_forwarder::FeeForwarderContract, ());
    let forwarder = fee_forwarder::FeeForwarderContractClient::new(e, &forwarder_id);
    let fee_recipient = Address::generate(e);
    let token_client = token::Client::new(e, &setup.token);
    let calls = open_batch(
        e,
        &setup.market,
        &setup.user,
        true,
        50_0000000,
        10_0000000,
        0,
    );

    let result = forwarder.forward_unsafe(
        &setup.token,
        &FORWARD_FEE,
        &FORWARD_MAX_FEE,
        &FORWARD_EXPIRATION,
        &setup.router,
        &Symbol::new(e, "create_and_fill"),
        &fill_target_args(e, &calls, &setup.user, &setup.keeper),
        &setup.user,
        &fee_recipient,
    );

    // The user's tree, recorded for the forward_unsafe call.
    let auths = e.auths();
    assert_eq!(auths.len(), 1);
    let (signer, root) = &auths[0];
    assert_eq!(signer, &setup.user);
    let AuthorizedFunction::Contract((root_contract, root_fn, root_args)) = &root.function else {
        panic!("the root must be a contract call");
    };
    assert_eq!(root_contract, &forwarder_id);
    assert_eq!(root_fn, &Symbol::new(e, "forward_unsafe"));
    assert_eq!(
        root_args,
        &vec![
            e,
            setup.token.into_val(e),
            FORWARD_MAX_FEE.into_val(e),
            FORWARD_EXPIRATION.into_val(e),
            fee_recipient.into_val(e),
            setup.router.into_val(e),
            Symbol::new(e, "create_and_fill").into_val(e),
        ]
    );
    let children: std::vec::Vec<(Address, Symbol)> = root
        .sub_invocations
        .iter()
        .map(|invocation| match &invocation.function {
            AuthorizedFunction::Contract((contract, func, _)) => (contract.clone(), func.clone()),
            _ => panic!("sub-invocations are contract calls"),
        })
        .collect();
    assert_eq!(
        children,
        std::vec![
            (setup.token.clone(), Symbol::new(e, "approve")),
            (setup.market.clone(), Symbol::new(e, "create_order")),
        ],
        "the router never appears in the user's tree"
    );
    let escrow = &root.sub_invocations[1].sub_invocations;
    assert_eq!(escrow.len(), 1);
    assert!(matches!(
        &escrow[0].function,
        AuthorizedFunction::Contract((contract, func, _))
            if contract == &setup.token && func == &Symbol::new(e, "transfer")
    ));

    // The router's create_and_fill results come back through the forwarder.
    let results = Vec::<Val>::try_from_val(e, &result).unwrap();
    assert_eq!(fill_order_id(e, &results), 1);
    assert_eq!(as_i128(e, &fill_outcome(&results)), 250_250);

    assert_eq!(token_client.balance(&fee_recipient), FORWARD_FEE);
    assert_eq!(token_client.balance(&setup.keeper), 250_250);
    assert_eq!(token_client.allowance(&setup.user, &forwarder_id), 0);
    assert_eq!(token_client.balance(&forwarder_id), 0);
    assert_eq!(token_client.balance(&setup.router), 0);
    assert_eq!(
        token_client.balance(&setup.user),
        100_0000000 - 10_0000000 - FORWARD_FEE
    );
    let position = market_wasm::Client::new(e, &setup.market).get_position(&setup.user, &true);
    assert_eq!(position.notional, 50_0000000);
}

// A forwarded create_and_try_fill whose fill fails rests the order and keeps
// the fee: the fee collection lands before the router runs, and the isolated
// fill only reports its failure.
#[test]
fn forward_unsafe_through_router_try_fill_rests_and_keeps_the_fee() {
    let setup = flow_setup(flow_config());
    let e = &setup.e;
    let forwarder_id = e.register(fee_forwarder::FeeForwarderContract, ());
    let forwarder = fee_forwarder::FeeForwarderContractClient::new(e, &forwarder_id);
    let fee_recipient = Address::generate(e);
    let token_client = token::Client::new(e, &setup.token);
    let calls = open_batch(
        e,
        &setup.market,
        &setup.user,
        true,
        50_0000000,
        10_0000000,
        9 * PRICE_SCALAR,
    );

    let result = forwarder.forward_unsafe(
        &setup.token,
        &FORWARD_FEE,
        &FORWARD_MAX_FEE,
        &FORWARD_EXPIRATION,
        &setup.router,
        &Symbol::new(e, "create_and_try_fill"),
        &fill_target_args(e, &calls, &setup.user, &setup.keeper),
        &setup.user,
        &fee_recipient,
    );

    let results = Vec::<Val>::try_from_val(e, &result).unwrap();
    assert!(fill_rested(e, &results));
    assert_eq!(
        contract_error(e, &fill_outcome(&results)),
        PRICE_BOUND_EXCEEDED
    );
    let order = market_wasm::Client::new(e, &setup.market)
        .get_order(&setup.user, &fill_order_id(e, &results));
    assert_eq!(order.notional, 50_0000000, "the order rests");
    assert_eq!(token_client.balance(&fee_recipient), FORWARD_FEE);
    assert_eq!(token_client.allowance(&setup.user, &forwarder_id), 0);
    assert_eq!(token_client.balance(&forwarder_id), 0);
}
