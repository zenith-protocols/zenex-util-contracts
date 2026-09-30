//! # Session Policy Contract
//!
//! A policy for trading session keys. The frontend registers an ed25519
//! session key on the smart account under a `Default` context rule with a
//! `valid_until`, so trades sign without a passkey prompt until the rule
//! expires. The account calls `enforce` once for every auth context the key
//! signs.
//!
//! The fee forwarder, the router, the markets, the collateral token and the
//! fee recipient are fixed at deploy, one instance-storage entry each. There
//! is no admin and no per-account state: a rule installs the policy with an
//! empty parameter, and install stores nothing.
//!
//! Every signer of the session rule must have signed. A wallet whose rule has
//! a policy leaves that check to the policy, so without it an authorization
//! with no signature at all would pass. Given the signature, the policy lets
//! through only:
//!
//! - `forward` / `forward_dynamic` on the forwarder, when the signed
//!   projection pays `fee_token = token` to `fee_recipient` and targets the
//!   router's `multicall`, `create_and_fill` or `create_and_try_fill`;
//! - `create_order`, `cancel_order` and `claim_credit` on the markets;
//! - `transfer` of the token into a market, which is the order escrow;
//! - `approve` of the token to the forwarder, the fee allowance.
//!
//! Everything else fails closed: other contracts (the wallet itself, the
//! router, the vault, other tokens), other functions, and every non-contract
//! context.
//!
//! ## What a stolen key can do
//!
//! It can trade on the markets. Losses and fees mostly go to the vault, plus
//! the execution fee of each order it fills itself. It can pay relay fees,
//! but only to `fee_recipient`. It cannot withdraw, move other tokens,
//! approve anyone but the forwarder, or call the wallet, the router, the
//! vault or any other contract.
//!
//! ## Why a forwarder allowance needs no cap
//!
//! The forwarder refuses `transfer_from` and `burn_from` targets, so only its
//! own fee step can spend an allowance it holds. That step pulls from the
//! wallet only under the wallet's root authorization, and pays the
//! `fee_recipient` in the signed projection, which this policy pins.
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
/// The router the forwarder may target: an `Address`.
pub(crate) const ROUTER: Symbol = symbol_short!("router");
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
    // The function is not allowed on that contract.
    FunctionNotAllowed = 4003,
    // A token transfer goes somewhere other than a market.
    TransferNotAllowed = 4004,
    // A token approval names a spender other than the forwarder.
    ApproveNotAllowed = 4005,
    // A forward's signed projection is not the pinned relayed trade.
    ForwardNotAllowed = 4006,
    // A signer of the session rule did not sign.
    SignerNotAuthenticated = 4007,
}

// ==========================================
// Constants
// ==========================================

const EXTEND_AMOUNT: u32 = 30 * 17280; // ~30 days
const TTL_THRESHOLD: u32 = EXTEND_AMOUNT - 17280; // refresh at ~29 days

/// The market functions a session may call: trading only. Vault deposits and
/// redeems stay behind the passkey.
const MARKET_FUNCTIONS: [&str; 3] = ["create_order", "cancel_order", "claim_credit"];

/// The router functions a forward may target.
const ROUTER_TARGETS: [&str; 3] = ["multicall", "create_and_fill", "create_and_try_fill"];

/// The forwarder's signed projection:
/// `[fee_token, max_fee_amount, expiration_ledger, fee_recipient,
/// target_contract, target_fn]`, plus `target_args` for `forward`.
const PROJECTION_FEE_TOKEN: u32 = 0;
const PROJECTION_FEE_RECIPIENT: u32 = 3;
const PROJECTION_TARGET_CONTRACT: u32 = 4;
const PROJECTION_TARGET_FN: u32 = 5;
const PROJECTION_LEN_UNSAFE: u32 = 6;
const PROJECTION_LEN_SAFE: u32 = 7;

// ==========================================
// Contract
// ==========================================

#[contract]
pub struct SessionPolicyContract;

#[contractimpl]
impl SessionPolicyContract {
    /// Fixes the policy's forwarder, router, markets, token and fee
    /// recipient. There is no way to change them later: a new configuration
    /// is a new deployment.
    ///
    /// # Errors
    /// - [`SessionPolicyError::InvalidConfig`] if `markets` is empty or any
    ///   two of the configured addresses are the same.
    pub fn __constructor(
        e: Env,
        forwarder: Address,
        router: Address,
        markets: Vec<Address>,
        token: Address,
        fee_recipient: Address,
    ) {
        let mut seen: Vec<Address> = Vec::new(&e);
        for address in [&forwarder, &router, &token, &fee_recipient]
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
        instance.set(&ROUTER, &router);
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
            // The root of a relayed trade. Its args are the signed
            // projection, not the full call.
            let len = if fn_name == symbol_short!("forward") {
                PROJECTION_LEN_SAFE
            } else if fn_name == Symbol::new(e, "forward_dynamic") {
                PROJECTION_LEN_UNSAFE
            } else {
                panic_with_error!(e, SessionPolicyError::FunctionNotAllowed);
            };
            let target_fn = args
                .get(PROJECTION_TARGET_FN)
                .and_then(|val| Symbol::try_from_val(e, &val).ok());
            if args.len() != len
                || address_arg(e, &args, PROJECTION_FEE_TOKEN) != Some(read(e, &TOKEN))
                || address_arg(e, &args, PROJECTION_FEE_RECIPIENT) != Some(read(e, &FEE_RECIPIENT))
                || address_arg(e, &args, PROJECTION_TARGET_CONTRACT) != Some(read(e, &ROUTER))
                || !target_fn.is_some_and(|name| is_one_of(e, &name, &ROUTER_TARGETS))
            {
                panic_with_error!(e, SessionPolicyError::ForwardNotAllowed);
            }
            return;
        }

        let markets: Vec<Address> = read(e, &MARKETS);
        if markets.contains(&contract) {
            if !is_one_of(e, &fn_name, &MARKET_FUNCTIONS) {
                panic_with_error!(e, SessionPolicyError::FunctionNotAllowed);
            }
        } else if contract == read::<Address>(e, &TOKEN) {
            if fn_name == symbol_short!("transfer") {
                // `transfer(from, to, amount)`: only the order escrow into a
                // market. A muxed `to` does not decode as an address, so it
                // fails here too.
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
        } else {
            panic_with_error!(e, SessionPolicyError::ContractNotAllowed);
        }
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

/// Whether `fn_name` is one of `allowed`.
fn is_one_of(e: &Env, fn_name: &Symbol, allowed: &[&str]) -> bool {
    allowed.iter().any(|name| *fn_name == Symbol::new(e, name))
}

/// Decodes `args[index]` as a plain address. A missing argument, a muxed
/// address or any other value is `None`.
fn address_arg(e: &Env, args: &Vec<Val>, index: u32) -> Option<Address> {
    args.get(index)
        .and_then(|val| Address::try_from_val(e, &val).ok())
}
