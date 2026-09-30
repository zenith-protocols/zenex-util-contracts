//! Minimal market contract client for the typed router fill leg.

use soroban_sdk::{contractclient, Address, Bytes, Env};

/// The market contract interface the router fill leg consumes.
///
/// The create leg rides the generic [`crate::Call`] batch, so only
/// `execute_order`, and its generated `try_execute_order`, is bound here.
#[allow(dead_code)] // consumed by the generated `MarketClient`
#[contractclient(name = "MarketClient")]
pub trait Market {
    /// Fills the order `(user, id)` at `price` and returns the keeper payout
    /// (token-dec). Refer to the market crate for the full contract.
    fn execute_order(e: Env, keeper: Address, user: Address, id: u32, price: Bytes) -> i128;
}
