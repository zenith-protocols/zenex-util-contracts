//! # Session Policy Contract
//!
//! A policy for trading session keys. The frontend registers an ed25519
//! session key on the smart account under a `Default` context rule with a
//! `valid_until`, so trades sign without a passkey prompt until the rule
//! expires. The account calls `enforce` once for every auth context the key
//! signs.
//!
//! The fee forwarder, the router, the markets, the collateral token and the
//! fee recipient are fixed at deploy. There is no admin and no storage: a
//! rule installs the policy with an empty parameter. The policy lets through
//! only:
//!
//! - `forward` / `forward_unsafe` on the forwarder, when the signed
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
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    Env, Symbol, TryFromVal, Val, Vec,
};
use stellar_accounts::{
    policies::Policy,
    smart_account::{ContextRule, Signer},
};

// ==========================================
// Types
// ==========================================

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// The fee forwarder the key's relayed trades go through.
    pub forwarder: Address,
    /// The router the forwarder may target.
    pub router: Address,
    /// The markets the key may trade on.
    pub markets: Vec<Address>,
    /// The collateral token, which is also the fee token.
    pub token: Address,
    /// The only account relay fees may go to.
    pub fee_recipient: Address,
}

#[contracttype]
enum StorageKey {
    Config,
}

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SessionPolicyError {
    InvalidConfig = 4001,
    ContractNotAllowed = 4002,
    FunctionNotAllowed = 4003,
    TransferNotAllowed = 4004,
    ApproveNotAllowed = 4005,
    ForwardNotAllowed = 4006,
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
        e.storage().instance().set(
            &StorageKey::Config,
            &Config {
                forwarder,
                router,
                markets,
                token,
                fee_recipient,
            },
        );
    }

    /// Returns the configuration fixed at deploy.
    pub fn get_config(e: Env) -> Config {
        read_config(&e)
    }
}

#[contractimpl]
impl Policy for SessionPolicyContract {
    type AccountParams = ();

    fn enforce(
        e: &Env,
        context: Context,
        _authenticated_signers: Vec<Signer>,
        _context_rule: ContextRule,
        smart_account: Address,
    ) {
        smart_account.require_auth();

        let config = read_config(e);
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

        if contract == config.forwarder {
            // The root of a relayed trade. Its args are the signed
            // projection, not the full call.
            let len = if fn_name == symbol_short!("forward") {
                PROJECTION_LEN_SAFE
            } else if fn_name == Symbol::new(e, "forward_unsafe") {
                PROJECTION_LEN_UNSAFE
            } else {
                panic_with_error!(e, SessionPolicyError::FunctionNotAllowed);
            };
            let target_fn = args
                .get(PROJECTION_TARGET_FN)
                .and_then(|val| Symbol::try_from_val(e, &val).ok());
            if args.len() != len
                || address_arg(e, &args, PROJECTION_FEE_TOKEN) != Some(config.token)
                || address_arg(e, &args, PROJECTION_FEE_RECIPIENT) != Some(config.fee_recipient)
                || address_arg(e, &args, PROJECTION_TARGET_CONTRACT) != Some(config.router)
                || !target_fn.is_some_and(|name| is_one_of(e, &name, &ROUTER_TARGETS))
            {
                panic_with_error!(e, SessionPolicyError::ForwardNotAllowed);
            }
        } else if config.markets.contains(&contract) {
            if !is_one_of(e, &fn_name, &MARKET_FUNCTIONS) {
                panic_with_error!(e, SessionPolicyError::FunctionNotAllowed);
            }
        } else if contract == config.token {
            if fn_name == symbol_short!("transfer") {
                // `transfer(from, to, amount)`: only the order escrow into a
                // market. A muxed `to` does not decode as an address, so it
                // fails here too.
                if !address_arg(e, &args, 1).is_some_and(|to| config.markets.contains(&to)) {
                    panic_with_error!(e, SessionPolicyError::TransferNotAllowed);
                }
            } else if fn_name == symbol_short!("approve") {
                // `approve(from, spender, amount, expiration_ledger)`: only the
                // forwarder's fee allowance, at any amount.
                if address_arg(e, &args, 1) != Some(config.forwarder) {
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

fn read_config(e: &Env) -> Config {
    e.storage()
        .instance()
        .get(&StorageKey::Config)
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
