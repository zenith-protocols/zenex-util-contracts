#![no_std]
#![allow(clippy::too_many_arguments)]

//! Fee forwarder: a stateless fee-abstraction contract. A user pays the relayer
//! in a token, and the forwarder makes the user's call in the same invocation.
//! See README.md.

#[cfg(test)]
mod test;

use soroban_sdk::{
    contract, contracterror, contractimpl, panic_with_error, Address, Env, IntoVal, Symbol, Val,
    Vec,
};
use stellar_fee_abstraction::{collect_fee, FeeAbstractionApproval};

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
    /// `user` authorizes `(fee_token, max_fee_amount, expiration_ledger,
    /// fee_recipient, target_contract, target_fn, target_args)`.
    ///
    /// # Arguments
    ///
    /// * `e` - Access to the Soroban environment.
    /// * `fee_token` - The token the fee is paid in.
    /// * `fee_amount` - The fee, above zero and at most `max_fee_amount`.
    /// * `max_fee_amount` - The fee cap the user signs.
    /// * `expiration_ledger` - The fee allowance's live-until ledger, at or
    ///   after execution.
    /// * `target_contract`, `target_fn`, `target_args` - The call to make.
    /// * `user` - The fee payer.
    /// * `fee_recipient` - The fee payee.
    ///
    /// Errors and events: see README.md.
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
    /// # Errors and events
    ///
    /// * See README.md, the same as [`Self::forward`].
    ///
    /// # Notes
    ///
    /// * Authorization for `user` is required over `(fee_token,
    ///   max_fee_amount, expiration_ledger, fee_recipient, target_contract,
    ///   target_fn)`.
    /// * The target flow must require the user's own authorization on every
    ///   call that moves the user's funds. Otherwise a relayer can supply
    ///   arbitrary `target_args` and still collect the fee.
    pub fn forward_dynamic(
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

    e.invoke_contract::<Val>(target_contract, target_fn, target_args.clone())
}
