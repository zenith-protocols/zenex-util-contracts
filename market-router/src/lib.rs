#![no_std]
#![allow(clippy::too_many_arguments)]

//! Stateless call router: generic batching in one transaction, plus the
//! dependent Zenex create-and-fill flows, strict and isolated.
//!
//! Relay fees are not collected here. The `fee-forwarder` crate collects them
//! and then calls this router's `multicall`, `create_and_fill` or
//! `create_and_try_fill` as its target.
//!
//! Ported from zenex-contracts `market-router` at commit
//! c93bc95ffea5809723ca14d600c553ad17e91f2d (unchanged since the mainnet
//! release 644a4c4) with the three `*_with_fee` functions removed. The four
//! remaining functions keep their names, argument order, return shapes and the
//! `Call` type.

mod dependencies;
mod types;

#[cfg(test)]
mod test;

pub use types::Call;

use dependencies::MarketClient;
use soroban_sdk::{
    contract, contractimpl,
    xdr::{ScErrorCode, ScErrorType},
    Address, Bytes, Env, IntoVal, TryFromVal, Val, Vec,
};

/// Stateless call router contract. It owns nothing and holds nothing.
#[contract]
pub struct RouterContract;

#[contractimpl]
impl RouterContract {
    /// Executes `calls` in order and returns each call's raw return value, in
    /// call order.
    ///
    /// Any failing call traps the whole invocation, so either every call
    /// lands or none does.
    ///
    /// # Arguments
    ///
    /// * `e` - Access to the Soroban environment.
    /// * `calls` - The `Call` sequence, executed front to back.
    pub fn multicall(e: Env, calls: Vec<Call>) -> Vec<Val> {
        run_multicall(&e, &calls)
    }

    /// Executes `calls` in order, isolates each call's failure, and returns
    /// one raw outcome per call, in call order. An outcome is the call's raw
    /// return value when it lands, or the failure as a host `Error` value
    /// when it does not.
    ///
    /// A call that fails with a recoverable error rolls back its own effects,
    /// and the batch continues with the next call. A budget or
    /// storage-footprint limit aborts the whole transaction. The two outcome
    /// kinds cannot collide, because the host turns an error-tagged return
    /// into a failure.
    ///
    /// # Arguments
    ///
    /// * `e` - Access to the Soroban environment.
    /// * `calls` - The `Call` sequence, executed front to back.
    pub fn multicall_try(e: Env, calls: Vec<Call>) -> Vec<Val> {
        let mut outcomes = Vec::new(&e);
        for call in calls.iter() {
            let outcome: Val = match e.try_invoke_contract::<Val, soroban_sdk::Error>(
                &call.contract,
                &call.func,
                call.args.clone(),
            ) {
                Ok(Ok(value)) => value,
                Err(Ok(error)) => error.into(),
                // Unreachable in practice: Val's conversion is infallible, and
                // every InvokeError converts to an Error. The value is a
                // synthetic sentinel for this unreachable path.
                Ok(Err(_)) | Err(Err(_)) => soroban_sdk::Error::from_type_and_code(
                    ScErrorType::Context,
                    ScErrorCode::InvalidAction,
                )
                .into(),
            };
            outcomes.push_back(outcome);
        }
        outcomes
    }

    /// Runs a create-and-fill batch strictly, fills `calls[0]`, and returns
    /// the `N` batch results with the fill payout appended (length `N + 1`).
    /// The payout is an `i128` (token-dec).
    ///
    /// By convention, `calls[0]` is the order to fill, and it targets the
    /// market contract (`calls[0].contract`). Its `u32` return value is the
    /// order id handed to the fill. A failing fill unwinds the whole batch,
    /// so nothing rests.
    ///
    /// # Arguments
    ///
    /// * `e` - Access to the Soroban environment.
    /// * `calls` - The batch, executed like `multicall`. The router does not
    ///   check the shape of `calls[0]`, and later calls are never filled.
    /// * `user` - The trader who owns the order, passed to `execute_order`.
    ///   It is an explicit argument, not decoded from `calls[0]`.
    /// * `keeper` - The fill-reward recipient. With `keeper = user` the fill
    ///   reward returns to the trader.
    /// * `price` - The serialized price update for the fill.
    ///
    /// # Notes
    ///
    /// * Traps on empty `calls`: there is no first call to fill.
    /// * Traps when `calls[0]` returns anything but a `u32` order id.
    pub fn create_and_fill(
        e: Env,
        calls: Vec<Call>,
        user: Address,
        keeper: Address,
        price: Bytes,
    ) -> Vec<Val> {
        let (mut results, market, id) = run_create_batch(&e, &calls);
        let payout = MarketClient::new(&e, &market).execute_order(&keeper, &user, &id, &price);
        results.push_back(payout.into_val(&e));
        results
    }

    /// Runs a create-and-fill batch strictly, then attempts an immediate
    /// fill, and returns the `N` batch results with the isolated fill outcome
    /// appended (length `N + 1`). The outcome is encoded like
    /// `multicall_try`: the payout (`i128`, token-dec) on a landed fill, or
    /// the failure as a host `Error` value when it rests.
    ///
    /// The batch follows the first-call convention of `create_and_fill`. A
    /// fill that fails with a recoverable error leaves the orders still
    /// standing after the batch available for a later keeper fill. A budget
    /// or storage-footprint limit in the fill aborts the whole transaction.
    ///
    /// # Arguments
    ///
    /// * `e` - Access to the Soroban environment.
    /// * `calls` - The create-and-fill batch. `calls[0]` is the order to
    ///   fill.
    /// * `user` - The trader who owns the order, passed to the fill.
    /// * `keeper` - The fill-reward recipient.
    /// * `price` - The serialized price update for the fill.
    ///
    /// # Notes
    ///
    /// * Traps on empty `calls`: there is no first call to fill.
    /// * Traps when `calls[0]` returns anything but a `u32` order id.
    pub fn create_and_try_fill(
        e: Env,
        calls: Vec<Call>,
        user: Address,
        keeper: Address,
        price: Bytes,
    ) -> Vec<Val> {
        let (mut results, market, id) = run_create_batch(&e, &calls);
        let client = MarketClient::new(&e, &market);
        results.push_back(try_fill_outcome(
            &e,
            client.try_execute_order(&keeper, &user, &id, &price),
        ));
        results
    }
}

/// Executes `calls` strictly front to back and collects each raw return
/// value.
///
/// Any failing call traps the whole invocation, so either every call lands
/// or none does.
fn run_multicall(e: &Env, calls: &Vec<Call>) -> Vec<Val> {
    let mut results = Vec::new(e);
    for call in calls.iter() {
        results.push_back(e.invoke_contract::<Val>(&call.contract, &call.func, call.args.clone()));
    }
    results
}

/// Runs a create-and-fill batch strictly and returns its results and fill
/// target. The tuple is `(results, market, id)`: the raw batch results, the
/// market contract (`calls[0].contract`), and the order id decoded from
/// `results[0]`.
///
/// # Errors
/// - Traps on empty `calls`: there is no first element to read.
/// - Traps when the return value of `calls[0]` is not a `u32`.
fn run_create_batch(e: &Env, calls: &Vec<Call>) -> (Vec<Val>, Address, u32) {
    // Empty `calls` trap here: there is no create leg to fill.
    let market = calls.get(0).unwrap().contract;
    let results = run_multicall(e, calls);
    // By convention `calls[0]` creates the order. Its result is the id.
    // A first call that returns anything else traps here on the u32
    // conversion.
    let id = u32::try_from_val(e, &results.get(0).unwrap()).unwrap();
    (results, market, id)
}

/// Encodes an isolated fill leg's result as a single `Val`, exactly like
/// [`RouterContract::multicall_try`] encodes a per-call outcome. The value is
/// the payout (`i128`) on success, or the failure as a host `Error` value.
#[allow(clippy::type_complexity)]
fn try_fill_outcome(
    e: &Env,
    result: Result<
        Result<i128, soroban_sdk::Error>,
        Result<soroban_sdk::Error, soroban_sdk::InvokeError>,
    >,
) -> Val {
    match result {
        Ok(Ok(payout)) => payout.into_val(e),
        Err(Ok(error)) => error.into(),
        // Unreachable for the market: its fill returns an `i128`, so the
        // conversion succeeds, and every InvokeError converts to an Error. A
        // target with another return type lands on this synthetic sentinel.
        Ok(Err(_)) | Err(Err(_)) => {
            soroban_sdk::Error::from_type_and_code(ScErrorType::Context, ScErrorCode::InvalidAction)
                .into()
        }
    }
}

/// Returns the `u32` discriminant of a contract error, or `u32::MAX` for any
/// other failure.
#[cfg(test)]
fn error_code(error: soroban_sdk::Error) -> u32 {
    if error.is_type(ScErrorType::Contract) {
        error.get_code()
    } else {
        u32::MAX
    }
}
