#![no_std]
#![allow(clippy::too_many_arguments)]

//! Fee forwarder: a thin fee layer in front of the market router. Each entry
//! point collects the relayer's fee in the fee token, then forwards to the
//! router's matching non-fee flow and returns its result.
//!
//! # Signed prefix and unsigned tail
//!
//! The user authorizes the prefix `(calls, max_fee_amount, fee_expiration)`
//! through `require_auth_for_args`. The submitter sets the tail after
//! signing: `fee_amount`, at most `max_fee_amount` and known only once the
//! final transaction is simulated, plus `keeper` and `price` for the fill
//! flows.
//!
//! The wallet signs one tree rooted at the forwarder call. For a
//! create-and-fill batch it is `approve(forwarder, max_fee_amount)`,
//! `approve(forwarder, 0)` and `market.create_order`, with the order's
//! `transfer(wallet, market, escrow)` under it. The router's non-fee flows
//! need no wallet authorization, so their frames are not part of the tree.
//!
//! # Fixed recipient and targets
//!
//! The router, the fee token and the fee recipient are fixed at deploy. A
//! submitter cannot redirect the fee, so a signature over a fee cap only ever
//! pays `fee_recipient`. The forwarder calls only the fee token (to collect
//! and wipe) and the router's three non-fee flows. It has no generic call of
//! its own.
//!
//! An allowance to the forwarder cannot be spent through the router's generic
//! calls. A contract is authorized only for the calls it makes directly, so a
//! `transfer_from` with the forwarder as spender that the router makes on a
//! batch's behalf fails: the forwarder never calls
//! `authorize_as_current_contract`. The fee allowance is wiped before the
//! router call in any case.

mod dependencies;
mod types;

#[cfg(test)]
mod test;

pub use types::{Call, Config};

use dependencies::RouterClient;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, token, vec, Address,
    Bytes, Env, IntoVal, Val, Vec,
};
use stellar_fee_abstraction::{collect_fee, FeeAbstractionApproval};

const DAY_IN_LEDGERS: u32 = 17280;
const INSTANCE_EXTEND_AMOUNT: u32 = 30 * DAY_IN_LEDGERS; // ~30 days
const INSTANCE_TTL_THRESHOLD: u32 = INSTANCE_EXTEND_AMOUNT - DAY_IN_LEDGERS; // refresh at ~29 days

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum FeeForwarderError {
    /// The router, the fee token and the fee recipient are not three
    /// distinct addresses.
    InvalidConfig = 6001,
}

#[contracttype]
enum StorageKey {
    Config,
}

/// Fee forwarder contract. It holds no funds; its config is fixed at deploy.
#[contract]
pub struct FeeForwarderContract;

#[contractimpl]
impl FeeForwarderContract {
    /// Fixes the router, the fee token and the fee recipient for the
    /// contract's lifetime. There is no admin and no setter.
    ///
    /// # Errors
    ///
    /// * `FeeForwarderError::InvalidConfig` - If any two of the three
    ///   addresses are equal.
    pub fn __constructor(e: Env, router: Address, fee_token: Address, fee_recipient: Address) {
        if router == fee_token || router == fee_recipient || fee_token == fee_recipient {
            panic_with_error!(&e, FeeForwarderError::InvalidConfig);
        }
        e.storage().instance().set(
            &StorageKey::Config,
            &Config {
                router,
                fee_token,
                fee_recipient,
            },
        );
    }

    /// Collects the relayer fee from `user`, then runs the router's
    /// `multicall(calls)` and returns its results. A failing call also
    /// reverts the fee.
    ///
    /// # Arguments
    ///
    /// * `e` - Access to the Soroban environment.
    /// * `calls` - The batch, executed by the router front to back.
    /// * `user` - The fee payer.
    /// * `max_fee_amount` - The fee cap (token-dec).
    /// * `fee_expiration` - The allowance's live-until ledger, at or after
    ///   execution.
    /// * `fee_amount` - The fee (token-dec). `0` skips it.
    ///
    /// # Errors
    ///
    /// * `FeeAbstractionError::InvalidFeeBounds` - If `fee_amount` is
    ///   negative or above `max_fee_amount`.
    ///
    /// # Events
    ///
    /// * topics - `["fee_collected", user: Address, recipient: Address]`
    /// * data - `[token: Address, amount: i128]`
    ///
    /// # Notes
    ///
    /// * Authorization for `user` is required over `(calls, max_fee_amount,
    ///   fee_expiration)`. The submitter sets `fee_amount`.
    pub fn multicall_with_fee(
        e: Env,
        calls: Vec<Call>,
        user: Address,
        max_fee_amount: i128,
        fee_expiration: u32,
        fee_amount: i128,
    ) -> Vec<Val> {
        let config = read_config(&e);
        authorize_and_collect_fee(
            &e,
            &config,
            &calls,
            &user,
            max_fee_amount,
            fee_expiration,
            fee_amount,
        );
        RouterClient::new(&e, &config.router).multicall(&calls)
    }

    /// Collects the relayer fee from `user`, then runs the router's
    /// `create_and_fill(calls, user, keeper, price)` and returns its results.
    /// A failing fill also unwinds the fee.
    ///
    /// # Arguments
    ///
    /// * `e` - Access to the Soroban environment.
    /// * `calls` - The create-and-fill batch. `calls[0]` is the order to
    ///   fill.
    /// * `user` - The trader and fee payer.
    /// * `max_fee_amount` - The fee cap (token-dec).
    /// * `fee_expiration` - The allowance's live-until ledger.
    /// * `fee_amount` - The fee (token-dec). `0` skips it.
    /// * `keeper` - The fill-reward recipient.
    /// * `price` - The price update.
    ///
    /// # Errors
    ///
    /// * `FeeAbstractionError::InvalidFeeBounds` - If `fee_amount` is
    ///   negative or above `max_fee_amount`.
    ///
    /// # Events
    ///
    /// * topics - `["fee_collected", user: Address, recipient: Address]`
    /// * data - `[token: Address, amount: i128]`
    ///
    /// # Notes
    ///
    /// * Authorization for `user` is required over `(calls, max_fee_amount,
    ///   fee_expiration)`. The submitter sets `fee_amount`, `keeper` and
    ///   `price`.
    pub fn create_and_fill_with_fee(
        e: Env,
        calls: Vec<Call>,
        user: Address,
        max_fee_amount: i128,
        fee_expiration: u32,
        fee_amount: i128,
        keeper: Address,
        price: Bytes,
    ) -> Vec<Val> {
        let config = read_config(&e);
        authorize_and_collect_fee(
            &e,
            &config,
            &calls,
            &user,
            max_fee_amount,
            fee_expiration,
            fee_amount,
        );
        RouterClient::new(&e, &config.router).create_and_fill(&calls, &user, &keeper, &price)
    }

    /// Collects the relayer fee from `user`, then runs the router's
    /// `create_and_try_fill(calls, user, keeper, price)` and returns its
    /// results. A resting fill keeps the fee.
    ///
    /// # Arguments
    ///
    /// * `e` - Access to the Soroban environment.
    /// * `calls` - The create-and-fill batch. `calls[0]` is the order to
    ///   fill.
    /// * `user` - The trader and fee payer.
    /// * `max_fee_amount` - The fee cap (token-dec).
    /// * `fee_expiration` - The allowance's live-until ledger.
    /// * `fee_amount` - The fee (token-dec). `0` skips it.
    /// * `keeper` - The fill-reward recipient.
    /// * `price` - The price update.
    ///
    /// # Errors
    ///
    /// * `FeeAbstractionError::InvalidFeeBounds` - If `fee_amount` is
    ///   negative or above `max_fee_amount`.
    ///
    /// # Events
    ///
    /// * topics - `["fee_collected", user: Address, recipient: Address]`
    /// * data - `[token: Address, amount: i128]`
    ///
    /// # Notes
    ///
    /// * Authorization for `user` is required over `(calls, max_fee_amount,
    ///   fee_expiration)`. The submitter sets `fee_amount`, `keeper` and
    ///   `price`.
    pub fn create_and_try_fill_with_fee(
        e: Env,
        calls: Vec<Call>,
        user: Address,
        max_fee_amount: i128,
        fee_expiration: u32,
        fee_amount: i128,
        keeper: Address,
        price: Bytes,
    ) -> Vec<Val> {
        let config = read_config(&e);
        authorize_and_collect_fee(
            &e,
            &config,
            &calls,
            &user,
            max_fee_amount,
            fee_expiration,
            fee_amount,
        );
        RouterClient::new(&e, &config.router).create_and_try_fill(&calls, &user, &keeper, &price)
    }

    /// Returns the router, the fee token and the fee recipient fixed at
    /// deploy.
    pub fn get_config(e: Env) -> Config {
        read_config(&e)
    }
}

/// Returns the config and keeps the instance, with its code, alive.
fn read_config(e: &Env) -> Config {
    e.storage()
        .instance()
        .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_EXTEND_AMOUNT);
    // The constructor always writes the config.
    e.storage().instance().get(&StorageKey::Config).unwrap()
}

/// Binds `user`'s authorization to the signed prefix and collects the
/// relayer fee into the fixed recipient. A zero `fee_amount` collects
/// nothing.
///
/// The authorization pins `(calls, max_fee_amount, fee_expiration)`. The
/// whole batch signs as one value, so touching any call forces a re-sign.
/// `fee_amount`, `keeper` and `price` stay outside the signature for the
/// submitter to set.
///
/// # Errors
/// - Refer to [`collect_fee`] errors.
///
/// # Events
/// - [`FeeCollected`](stellar_fee_abstraction::FeeCollected): the fee moved
///   from `user` to the fixed recipient.
fn authorize_and_collect_fee(
    e: &Env,
    config: &Config,
    calls: &Vec<Call>,
    user: &Address,
    max_fee_amount: i128,
    fee_expiration: u32,
    fee_amount: i128,
) {
    user.require_auth_for_args(vec![
        e,
        calls.into_val(e),
        max_fee_amount.into_val(e),
        fee_expiration.into_val(e),
    ]);
    if fee_amount == 0 {
        return;
    }
    // The allowance lives until the signed `fee_expiration`, not a
    // ledger-derived value, which would drift between the auth-discovery
    // simulation and execution and break the signed `approve`.
    collect_fee(
        e,
        &config.fee_token,
        fee_amount,
        max_fee_amount,
        fee_expiration,
        user,
        &config.fee_recipient,
        FeeAbstractionApproval::Eager,
    );
    // The residual allowance, `max_fee_amount - fee_amount`, is wiped before
    // the router runs, so no fee allowance outlives the collection.
    let forwarder = e.current_contract_address();
    token::Client::new(e, &config.fee_token).approve(user, &forwarder, &0, &fee_expiration);
}
