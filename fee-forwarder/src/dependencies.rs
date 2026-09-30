//! Minimal market router client for the forwarded flows.

use crate::types::Call;
use soroban_sdk::{contractclient, Address, Bytes, Env, Val, Vec};

/// The market router interface the forwarder consumes: its three non-fee
/// flows. Refer to the market router for their full contracts.
#[allow(dead_code)] // consumed by the generated `RouterClient`
#[contractclient(name = "RouterClient")]
pub trait Router {
    /// Executes `calls` strictly and returns each raw result.
    fn multicall(e: Env, calls: Vec<Call>) -> Vec<Val>;

    /// Runs a create-and-fill batch strictly and fills `calls[0]`.
    fn create_and_fill(
        e: Env,
        calls: Vec<Call>,
        user: Address,
        keeper: Address,
        price: Bytes,
    ) -> Vec<Val>;

    /// Runs a create-and-fill batch strictly and attempts an isolated fill.
    fn create_and_try_fill(
        e: Env,
        calls: Vec<Call>,
        user: Address,
        keeper: Address,
        price: Bytes,
    ) -> Vec<Val>;
}
