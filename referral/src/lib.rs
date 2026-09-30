#![no_std]

//! On-chain referral attestation. A wallet attests which wallet referred it.
//! The contract keeps no storage and has no admin: the `Attributed` event is
//! the record, and indexers replay it to build the referral graph.

#[cfg(test)]
mod test;

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, panic_with_error, Address, Env,
};

// Codes sit in the 7000 range, clear of every other contract that can share a
// call tree with this one (see "Error codes" in the README). Plain comments,
// not doc comments: those would land in the contract spec.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ReferralError {
    // `caller` names itself as `referrer`.
    SelfReferral = 7001,
}

/// Emitted on each successful attribution.
///
/// Topics are `("attributed", referee, referrer)`, so subscribers can filter
/// on either side.
#[contractevent]
#[derive(Clone)]
pub struct Attributed {
    #[topic]
    pub referee: Address,
    #[topic]
    pub referrer: Address,
}

/// Referral attestation contract. It owns nothing and stores nothing.
#[contract]
pub struct ReferralContract;

#[contractimpl]
impl ReferralContract {
    /// Attests that `caller` was referred by `referrer`. The event is the
    /// only output: a successful simulation is the pre-flight.
    ///
    /// # Arguments
    ///
    /// * `env` - Access to the Soroban environment.
    /// * `caller` - The referred wallet. Its authorization is required.
    /// * `referrer` - The referring wallet.
    ///
    /// # Errors
    ///
    /// * `ReferralError::SelfReferral` - If `caller` equals `referrer`.
    ///
    /// # Events
    ///
    /// * topics - `["attributed", referee: Address, referrer: Address]`
    pub fn attribute(env: Env, caller: Address, referrer: Address) {
        caller.require_auth();
        if caller == referrer {
            panic_with_error!(&env, ReferralError::SelfReferral);
        }

        Attributed {
            referee: caller,
            referrer,
        }
        .publish(&env);
    }
}
