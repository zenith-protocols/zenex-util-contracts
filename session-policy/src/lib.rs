#![no_std]

//! Session policy: limits a wallet's trading session key to the Zenex trade
//! flow, with relay fees paid only to the pinned recipient. See README.md.

#[cfg(test)]
mod test;

use soroban_sdk::{
    auth::{Context, ContractContext},
    contract, contracterror, contractimpl, panic_with_error, symbol_short, Address, Env, Symbol,
    TryFromVal, Val, Vec,
};
use stellar_accounts::{
    policies::Policy,
    smart_account::{ContextRule, Signer},
};

// ==========================================
// Storage
// ==========================================

// The configuration sits in instance storage, one entry per value, so each
// `enforce` branch reads only the values it checks. The keys are short
// symbols, which are constants: building one costs no host call.

/// The fee forwarder the key's relayed trades go through: an `Address`.
pub(crate) const FORWARDER: Symbol = symbol_short!("forwarder");
/// The markets the key may trade on: a `Vec<Address>`.
pub(crate) const MARKETS: Symbol = symbol_short!("markets");
/// The collateral token, which is also the fee token: an `Address`.
pub(crate) const TOKEN: Symbol = symbol_short!("token");
/// The only account relay fees may go to: an `Address`.
pub(crate) const FEE_RECIPIENT: Symbol = symbol_short!("recipient");

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum SessionPolicyError {
    // The constructor got empty markets or a repeated address.
    InvalidConfig = 4001,
    // The context is not a call to an allowed contract.
    ContractNotAllowed = 4002,
    // A token function other than `transfer` or `approve`.
    FunctionNotAllowed = 4003,
    // A token transfer goes somewhere other than a market.
    TransferNotAllowed = 4004,
    // A token approval names a spender other than the forwarder.
    ApproveNotAllowed = 4005,
    // A forward's signed fee recipient is not the pinned recipient.
    ForwardNotAllowed = 4006,
    // A signer of the session rule did not sign.
    SignerNotAuthenticated = 4007,
}

// ==========================================
// Constants
// ==========================================

const EXTEND_AMOUNT: u32 = 30 * 17280; // ~30 days
const TTL_THRESHOLD: u32 = EXTEND_AMOUNT - 17280; // refresh at ~29 days

/// Where `fee_recipient` sits in the forwarder's signed projection,
/// `[fee_token, max_fee_amount, expiration_ledger, fee_recipient,
/// target_contract, target_fn(, target_args)]`, the same in `forward` and
/// `forward_dynamic`.
const PROJECTION_FEE_RECIPIENT: u32 = 3;

// ==========================================
// Contract
// ==========================================

#[contract]
pub struct SessionPolicyContract;

#[contractimpl]
impl SessionPolicyContract {
    /// Fixes the policy's forwarder, markets, token and fee recipient. There
    /// is no way to change them later: a new configuration is a new
    /// deployment.
    ///
    /// # Errors
    /// - [`SessionPolicyError::InvalidConfig`] if `markets` is empty or any
    ///   two of the configured addresses are the same.
    pub fn __constructor(
        e: Env,
        forwarder: Address,
        markets: Vec<Address>,
        token: Address,
        fee_recipient: Address,
    ) {
        let mut seen: Vec<Address> = Vec::new(&e);
        for address in [&forwarder, &token, &fee_recipient]
            .into_iter()
            .cloned()
            .chain(markets.iter())
        {
            if seen.contains(&address) {
                panic_with_error!(&e, SessionPolicyError::InvalidConfig);
            }
            seen.push_back(address);
        }
        if markets.is_empty() {
            panic_with_error!(&e, SessionPolicyError::InvalidConfig);
        }
        let instance = e.storage().instance();
        instance.set(&FORWARDER, &forwarder);
        instance.set(&MARKETS, &markets);
        instance.set(&TOKEN, &token);
        instance.set(&FEE_RECIPIENT, &fee_recipient);
    }
}

#[contractimpl]
impl Policy for SessionPolicyContract {
    type AccountParams = ();

    fn enforce(
        e: &Env,
        context: Context,
        authenticated_signers: Vec<Signer>,
        context_rule: ContextRule,
        smart_account: Address,
    ) {
        smart_account.require_auth();

        // The wallet checks a rule's signers itself only when the rule has no
        // policies; with one, it leaves the check here. Every signer of the
        // rule, the session key, must have signed. The wallet passes the
        // rule's signers found in the payload, filtered from the rule's own
        // list, so they all signed exactly when the counts match. Comparing
        // counts keeps the check from decoding each signer.
        if authenticated_signers.is_empty()
            || authenticated_signers.len() != context_rule.signers.len()
        {
            panic_with_error!(e, SessionPolicyError::SignerNotAuthenticated);
        }

        e.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, EXTEND_AMOUNT);

        // Only contract calls. Contract creation matches nothing below.
        let Context::Contract(ContractContext {
            contract,
            fn_name,
            args,
        }) = context
        else {
            panic_with_error!(e, SessionPolicyError::ContractNotAllowed);
        };

        // The constructor makes every configured address distinct, so at
        // most one branch matches. Each reads only the values it checks.
        let forwarder: Address = read(e, &FORWARDER);
        if contract == forwarder {
            // The root of a relayed trade; its args are the signed
            // projection. The fee leaves the wallet through the forwarder's
            // own `transfer_from`, which never reaches this policy, so the
            // signed recipient is the one thing to pin here. Whatever the
            // forward calls, every call that needs the wallet's
            // authorization is a context of its own.
            if address_arg(e, &args, PROJECTION_FEE_RECIPIENT) != Some(read(e, &FEE_RECIPIENT)) {
                panic_with_error!(e, SessionPolicyError::ForwardNotAllowed);
            }
            return;
        }

        // A market call that needs the wallet's authorization acts on the
        // wallet's own funds and pays back to the wallet, so any function is
        // allowed.
        let markets: Vec<Address> = read(e, &MARKETS);
        if markets.contains(&contract) {
            return;
        }

        if contract == read::<Address>(e, &TOKEN) {
            if fn_name == symbol_short!("transfer") {
                // `transfer(from, to, amount)`: only escrow into a market. A
                // muxed `to` does not decode as an address, so it fails here
                // too.
                if !address_arg(e, &args, 1).is_some_and(|to| markets.contains(&to)) {
                    panic_with_error!(e, SessionPolicyError::TransferNotAllowed);
                }
            } else if fn_name == symbol_short!("approve") {
                // `approve(from, spender, amount, expiration_ledger)`: only the
                // forwarder's fee allowance, at any amount.
                if address_arg(e, &args, 1) != Some(forwarder) {
                    panic_with_error!(e, SessionPolicyError::ApproveNotAllowed);
                }
            } else {
                panic_with_error!(e, SessionPolicyError::FunctionNotAllowed);
            }
            return;
        }

        panic_with_error!(e, SessionPolicyError::ContractNotAllowed);
    }

    fn install(
        _e: &Env,
        _params: Self::AccountParams,
        _context_rule: ContextRule,
        smart_account: Address,
    ) {
        // Nothing to store: the configuration is fixed at deploy.
        smart_account.require_auth();
    }

    fn uninstall(_e: &Env, _context_rule: ContextRule, smart_account: Address) {
        smart_account.require_auth();
    }
}

// ==========================================
// Helpers
// ==========================================

/// Reads the configured value under `key`. The constructor sets every key,
/// so a deployed policy never misses one.
fn read<V: TryFromVal<Env, Val>>(e: &Env, key: &Symbol) -> V {
    e.storage()
        .instance()
        .get(key)
        .unwrap_or_else(|| panic_with_error!(e, SessionPolicyError::InvalidConfig))
}

/// Decodes `args[index]` as a plain address. A missing argument, a muxed
/// address or any other value is `None`.
fn address_arg(e: &Env, args: &Vec<Val>, index: u32) -> Option<Address> {
    args.get(index)
        .and_then(|val| Address::try_from_val(e, &val).ok())
}
