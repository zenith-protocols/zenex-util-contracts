#![no_std]

//! On-chain referral attestation. A wallet attests which wallet referred it.
//! The contract keeps no storage and has no admin: the `Attributed` event is
//! the record, and indexers replay it to build the referral graph.

#[cfg(test)]
mod test;

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, panic_with_error, Address, Env,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ReferralError {
    SelfReferral = 1,
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
    /// Attests that `caller` was referred by `referrer` and returns
    /// `(caller, referrer)`, which a simulation can read as a pre-flight.
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
    pub fn attribute(env: Env, caller: Address, referrer: Address) -> (Address, Address) {
        caller.require_auth();
        if caller == referrer {
            panic_with_error!(&env, ReferralError::SelfReferral);
        }

        Attributed {
            referee: caller.clone(),
            referrer: referrer.clone(),
        }
        .publish(&env);

        (caller, referrer)
    }
}
