#![no_std]
#![allow(clippy::too_many_arguments)]

//! Fee forwarder: a stateless fee-abstraction primitive. Deploy once, anyone
//! uses it. A user pays the relayer in a token instead of native XLM, and the
//! forwarder makes the user's call in the same invocation.
//!
//! # Signed projection
//!
//! The user authorizes a projection of the arguments through
//! `require_auth_for_args`:
//!
//! - [`FeeForwarderContract::forward`] signs `(fee_token, max_fee_amount,
//!   expiration_ledger, fee_recipient, target_contract, target_fn,
//!   target_args)`.
//! - [`FeeForwarderContract::forward_unsafe`] signs the same projection
//!   without `target_args`, so a relayer can refresh them after signing, such
//!   as a fresh price update.
//!
//! `fee_amount` stays outside both: the relayer sets it, at most
//! `max_fee_amount`, once the final transaction is simulated. `fee_recipient`
//! sits at index 3 of both projections.
//!
//! OpenZeppelin's forwarder leaves the recipient to the relayer. Here the
//! user signs it, so a signature pays only the recipient it names, and a
//! wallet policy can pin that recipient.
//!
//! # Allowances
//!
//! The fee is collected by OpenZeppelin's own `collect_fee` with Eager
//! approval, from the `stellar-fee-abstraction` crate pinned to the head of
//! OpenZeppelin's `v0.9.0` branch (commit `df602b6`). That code is UNRELEASED
//! and UNAUDITED. It is used because only that branch carries
//! [OpenZeppelin/stellar-contracts#873], the fix for issue #875: the user
//! approves `max_fee_amount`, the forwarder pulls the whole `max_fee_amount`
//! to itself, pays `fee_amount` to `fee_recipient` and refunds the rest. The
//! pull consumes the allowance, so it ends at zero, and the user's balance
//! must cover `max_fee_amount`, not only `fee_amount`. Every released version
//! (up to 0.7.2 and 0.8.0-rc.3) pulls only the fee and leaves the rest of the
//! allowance standing.
//!
//! The forwarder calls the target directly, and a contract is authorized for
//! the calls it makes directly, so a target of `token.transfer_from(forwarder,
//! victim, …)` would spend any allowance the victim holds to the forwarder.
//! Two rules close that:
//!
//! - The fee pull consumes the allowance before the target runs, so no fee
//!   allowance outlives the collection.
//! - `transfer_from` and `burn_from`, the token functions that spend an
//!   allowance, are refused as a target on every contract. An allowance a
//!   user grants the forwarder outside a forward can therefore never be
//!   spent through one.
//!
//! OpenZeppelin keeps the fee in the contract when the recipient is the
//! contract itself, for a later sweep. This forwarder has no sweep, and a
//! forward whose target is the token's `transfer` from the forwarder can move
//! any balance it holds, so such a fee would go to whoever takes it first.
//! The forwarder is therefore refused as `fee_recipient`.
//!
//! [OpenZeppelin/stellar-contracts#873]:
//!     https://github.com/OpenZeppelin/stellar-contracts/pull/873
//!
//! # `forward_unsafe`
//!
//! Unsigned `target_args` bind nothing at the root. The target flow must
//! protect the user's intent itself: every call that moves the user's funds
//! must require the user's own authorization on its exact arguments. A Zenex
//! order does: `create_order` calls `user.require_auth()`, so its arguments
//! are part of the signed tree whatever the relayer puts in `target_args`.

#[cfg(test)]
mod test;

use soroban_sdk::{
    contract, contracterror, contractimpl, panic_with_error, Address, Env, IntoVal, Symbol, Val,
    Vec,
};
use stellar_fee_abstraction::{collect_fee, emit_forward_executed, FeeAbstractionApproval};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum FeeForwarderError {
    /// `target_fn` is `transfer_from` or `burn_from`, which spend an
    /// allowance.
    TargetNotAllowed = 6001,
    /// `fee_recipient` is the forwarder, which could never pass the fee on.
    InvalidRecipient = 6002,
}

/// Fee forwarder contract. It holds no state, and no funds between calls.
#[contract]
pub struct FeeForwarderContract;

#[contractimpl]
impl FeeForwarderContract {
    /// Collects `fee_amount` of `fee_token` from `user` to `fee_recipient`,
    /// then calls `target_contract.target_fn(target_args)` and returns its
    /// result. A failing target also reverts the fee.
    ///
    /// # Arguments
    ///
    /// * `e` - Access to the Soroban environment.
    /// * `fee_token` - The token the fee is paid in.
    /// * `fee_amount` - The fee, above zero and at most `max_fee_amount`.
    /// * `max_fee_amount` - The fee cap the user signs.
    /// * `expiration_ledger` - The fee allowance's live-until ledger, at or
    ///   after execution.
    /// * `target_contract` - The contract to call.
    /// * `target_fn` - The function to call.
    /// * `target_args` - The call's arguments.
    /// * `user` - The fee payer.
    /// * `fee_recipient` - The fee payee.
    ///
    /// # Errors
    ///
    /// * `FeeForwarderError::TargetNotAllowed` - If `target_fn` is
    ///   `transfer_from` or `burn_from`.
    /// * `FeeForwarderError::InvalidRecipient` - If `fee_recipient` is the
    ///   forwarder.
    /// * `FeeAbstractionError::InvalidFeeBounds` - If `fee_amount` is not
    ///   above zero or exceeds `max_fee_amount`.
    /// * `FeeAbstractionError::InvalidUser` - If `user` is the forwarder.
    /// * The token's own error if `user` holds less than `max_fee_amount`.
    ///
    /// # Events
    ///
    /// * topics - `["fee_collected", user: Address, recipient: Address]`
    /// * data - `[token: Address, amount: i128]`
    /// * topics - `["forward_executed", user: Address, target_contract:
    ///   Address]`
    /// * data - `[target_fn: Symbol, target_args: Vec<Val>]`
    ///
    /// # Notes
    ///
    /// * Authorization for `user` is required over `(fee_token,
    ///   max_fee_amount, expiration_ledger, fee_recipient, target_contract,
    ///   target_fn, target_args)`.
    pub fn forward(
        e: Env,
        fee_token: Address,
        fee_amount: i128,
        max_fee_amount: i128,
        expiration_ledger: u32,
        target_contract: Address,
        target_fn: Symbol,
        target_args: Vec<Val>,
        user: Address,
        fee_recipient: Address,
    ) -> Val {
        user.require_auth_for_args(
            (
                fee_token.clone(),
                max_fee_amount,
                expiration_ledger,
                fee_recipient.clone(),
                target_contract.clone(),
                target_fn.clone(),
                target_args.clone(),
            )
                .into_val(&e),
        );
        collect_and_invoke(
            &e,
            &fee_token,
            fee_amount,
            max_fee_amount,
            expiration_ledger,
            &target_contract,
            &target_fn,
            &target_args,
            &user,
            &fee_recipient,
        )
    }

    /// Acts as [`Self::forward`], but the user's authorization leaves out
    /// `target_args`, so the relayer can refresh them after signing.
    ///
    /// # Arguments
    ///
    /// * Refer to [`Self::forward`].
    ///
    /// # Errors
    ///
    /// * Refer to [`Self::forward`].
    ///
    /// # Events
    ///
    /// * Refer to [`Self::forward`].
    ///
    /// # Notes
    ///
    /// * Authorization for `user` is required over `(fee_token,
    ///   max_fee_amount, expiration_ledger, fee_recipient, target_contract,
    ///   target_fn)`.
    /// * The target flow must require the user's own authorization on every
    ///   call that moves the user's funds. Otherwise a relayer can supply
    ///   arbitrary `target_args` and still collect the fee.
    pub fn forward_unsafe(
        e: Env,
        fee_token: Address,
        fee_amount: i128,
        max_fee_amount: i128,
        expiration_ledger: u32,
        target_contract: Address,
        target_fn: Symbol,
        target_args: Vec<Val>,
        user: Address,
        fee_recipient: Address,
    ) -> Val {
        user.require_auth_for_args(
            (
                fee_token.clone(),
                max_fee_amount,
                expiration_ledger,
                fee_recipient.clone(),
                target_contract.clone(),
                target_fn.clone(),
            )
                .into_val(&e),
        );
        collect_and_invoke(
            &e,
            &fee_token,
            fee_amount,
            max_fee_amount,
            expiration_ledger,
            &target_contract,
            &target_fn,
            &target_args,
            &user,
            &fee_recipient,
        )
    }
}

/// Refuses the allowance-spending token functions and a forwarder recipient,
/// collects the fee with OpenZeppelin's Eager `collect_fee`, then calls the
/// target and returns its result.
///
/// # Errors
/// - `FeeForwarderError::TargetNotAllowed` if `target_fn` is `transfer_from`
///   or `burn_from`.
/// - `FeeForwarderError::InvalidRecipient` if `fee_recipient` is the
///   forwarder.
/// - Refer to [`collect_fee`] errors.
///
/// # Events
/// - [`FeeCollected`](stellar_fee_abstraction::FeeCollected) and
///   [`ForwardExecuted`](stellar_fee_abstraction::ForwardExecuted).
fn collect_and_invoke(
    e: &Env,
    fee_token: &Address,
    fee_amount: i128,
    max_fee_amount: i128,
    expiration_ledger: u32,
    target_contract: &Address,
    target_fn: &Symbol,
    target_args: &Vec<Val>,
    user: &Address,
    fee_recipient: &Address,
) -> Val {
    // The forwarder calls the target directly, so it would be the authorized
    // spender of any `transfer_from` or `burn_from` the target names.
    if *target_fn == Symbol::new(e, "transfer_from") || *target_fn == Symbol::new(e, "burn_from") {
        panic_with_error!(e, FeeForwarderError::TargetNotAllowed);
    }

    // OpenZeppelin would keep a fee addressed to the forwarder, and nothing
    // here could ever pass it on.
    if *fee_recipient == e.current_contract_address() {
        panic_with_error!(e, FeeForwarderError::InvalidRecipient);
    }
    collect_fee(
        e,
        fee_token,
        fee_amount,
        max_fee_amount,
        expiration_ledger,
        user,
        fee_recipient,
        FeeAbstractionApproval::Eager,
    );

    let result = e.invoke_contract::<Val>(target_contract, target_fn, target_args.clone());
    emit_forward_executed(e, user, target_contract, target_fn, target_args);
    result
}
