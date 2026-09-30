#![no_std]
//! On-chain referral attestation. See README.md.

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
#[contractevent]
#[derive(Clone)]
pub struct Attributed {
    #[topic]
    pub referee: Address,
    #[topic]
    pub referrer: Address,
}

#[contract]
pub struct SorobanReferral;

#[contractimpl]
impl SorobanReferral {
    /// Attest that `caller` was referred by `referrer`. Errors on self-referral.
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

mod test;
