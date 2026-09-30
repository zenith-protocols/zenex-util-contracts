//! # Session Policy Contract
//!
//! A policy for trading session keys. The frontend registers an ed25519
//! session key on the smart account under a `Default` context rule with a
//! `valid_until`, so trades sign without a passkey prompt until the rule
//! expires. The account calls `enforce` once for every auth context the key
//! signs.
//!
//! The markets, the collateral token, the router and the fee budget are fixed
//! at deploy. There is no admin and nothing to configure per wallet, so a rule
//! installs the policy with an empty parameter. The policy lets through only:
//!
//! - `create_order`, `cancel_order` and `claim_credit` on the markets;
//! - the router's relay-fee envelopes (`*_with_fee`);
//! - `transfer` of the token into a market, which is the order escrow;
//! - `approve` of the token to the router, within the session's fee budget.
//!
//! Everything else fails closed: other contracts (the wallet itself, the
//! vault, other tokens), other functions, and every non-contract context.
//!
//! ## What a stolen key can do
//!
//! It can trade on the markets. Losses and fees mostly go to the vault, plus
//! the execution fee (0.01 USDC on mainnet) of each order it fills itself. It
//! can also pay up to `fee_budget` in relay fees to anyone. It cannot
//! withdraw, move other tokens, or touch the wallet, the vault or any other
//! contract.
//!
//! ## Why the fee is budgeted, not pinned
//!
//! The router moves the relay fee by `transfer_from`, as the spender, to a
//! recipient the submitter picks after the wallet has signed. That transfer
//! needs no signature from the wallet, so it never reaches this policy. The
//! wallet signs only the router's allowance, and that allowance counts
//! against `fee_budget` at its full amount.
//!
//! The budget belongs to the context rule. A session that spends it renews
//! with a new rule, which starts again at zero.
use soroban_sdk::{
    auth::{Context, ContractContext},
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    Env, Symbol, TryFromVal, Vec,
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
    /// The markets the key may trade on.
    pub markets: Vec<Address>,
    /// The collateral token.
    pub token: Address,
    /// The router whose relay-fee envelopes the key may sign.
    pub router: Address,
    /// The most `token` the key may approve to the router per session
    /// (token-dec).
    pub fee_budget: i128,
}

#[contracttype]
enum StorageKey {
    Config,
    FeesSpent(Address, u32), // (smart_account, context_rule_id)
}

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SessionPolicyError {
    InvalidConfig = 4001,
    ContractNotAllowed = 4002,
    FunctionNotAllowed = 4003,
    TransferNotAllowed = 4004,
    ApproveNotAllowed = 4005,
    InvalidAmount = 4006,
    FeeBudgetExceeded = 4007,
}

// ==========================================
// Constants
// ==========================================

const EXTEND_AMOUNT: u32 = 30 * 17280; // ~30 days
const TTL_THRESHOLD: u32 = EXTEND_AMOUNT - 17280; // refresh at ~29 days

/// The market functions a session may call: trading only. Vault deposits and
/// redeems stay behind the passkey.
const MARKET_FUNCTIONS: [&str; 3] = ["create_order", "cancel_order", "claim_credit"];

/// The router functions that carry the wallet's signature. The plain router
/// functions never ask for it.
const ROUTER_FUNCTIONS: [&str; 3] = [
    "multicall_with_fee",
    "create_and_fill_with_fee",
    "create_and_try_fill_with_fee",
];

// ==========================================
// Contract
// ==========================================

#[contract]
pub struct SessionPolicyContract;

#[contractimpl]
impl SessionPolicyContract {
    /// Fixes the policy's markets, token, router and fee budget. There is no
    /// way to change them later: a new configuration is a new deployment.
    ///
    /// # Errors
    /// - [`SessionPolicyError::InvalidConfig`] if `markets` is empty,
    ///   `fee_budget` is not positive, or the token, the router and the
    ///   markets overlap.
    pub fn __constructor(
        e: Env,
        markets: Vec<Address>,
        token: Address,
        router: Address,
        fee_budget: i128,
    ) {
        if markets.is_empty()
            || fee_budget <= 0
            || token == router
            || markets.contains(&token)
            || markets.contains(&router)
        {
            panic_with_error!(&e, SessionPolicyError::InvalidConfig);
        }
        e.storage().instance().set(
            &StorageKey::Config,
            &Config {
                markets,
                token,
                router,
                fee_budget,
            },
        );
    }

    /// Returns the configuration fixed at deploy.
    pub fn get_config(e: Env) -> Config {
        read_config(&e)
    }

    /// Returns the relay fees the key has approved under
    /// `(smart_account, context_rule_id)` (token-dec), so the UI can show
    /// the remaining budget.
    pub fn get_fees_spent(e: Env, smart_account: Address, context_rule_id: u32) -> i128 {
        e.storage()
            .persistent()
            .get(&StorageKey::FeesSpent(smart_account, context_rule_id))
            .unwrap_or(0)
    }
}

#[contractimpl]
impl Policy for SessionPolicyContract {
    type AccountParams = ();

    fn enforce(
        e: &Env,
        context: Context,
        _authenticated_signers: Vec<Signer>,
        context_rule: ContextRule,
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

        if config.markets.contains(&contract) {
            require_function(e, &fn_name, &MARKET_FUNCTIONS);
        } else if contract == config.router {
            require_function(e, &fn_name, &ROUTER_FUNCTIONS);
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
                // router's fee allowance, counted against the fee budget.
                if address_arg(e, &args, 1) != Some(config.router) {
                    panic_with_error!(e, SessionPolicyError::ApproveNotAllowed);
                }
                let amount = args
                    .get(2)
                    .and_then(|val| i128::try_from_val(e, &val).ok())
                    .filter(|amount| *amount >= 0)
                    .unwrap_or_else(|| panic_with_error!(e, SessionPolicyError::InvalidAmount));
                let key = StorageKey::FeesSpent(smart_account, context_rule.id);
                let spent = e
                    .storage()
                    .persistent()
                    .get::<_, i128>(&key)
                    .unwrap_or(0)
                    .checked_add(amount)
                    .filter(|spent| *spent <= config.fee_budget)
                    .unwrap_or_else(|| panic_with_error!(e, SessionPolicyError::FeeBudgetExceeded));
                e.storage().persistent().set(&key, &spent);
                e.storage()
                    .persistent()
                    .extend_ttl(&key, TTL_THRESHOLD, EXTEND_AMOUNT);
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
        // Nothing to store: the configuration is fixed at deploy, and the
        // fee counter starts at zero on first use.
        smart_account.require_auth();
    }

    fn uninstall(e: &Env, context_rule: ContextRule, smart_account: Address) {
        smart_account.require_auth();

        e.storage()
            .persistent()
            .remove(&StorageKey::FeesSpent(smart_account, context_rule.id));
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

/// Traps with `FunctionNotAllowed` unless `fn_name` is one of `allowed`.
fn require_function(e: &Env, fn_name: &Symbol, allowed: &[&str]) {
    if !allowed.iter().any(|name| *fn_name == Symbol::new(e, name)) {
        panic_with_error!(e, SessionPolicyError::FunctionNotAllowed);
    }
}

/// Decodes `args[index]` as a plain address. A missing argument, a muxed
/// address or any other value is `None`.
fn address_arg(e: &Env, args: &Vec<soroban_sdk::Val>, index: u32) -> Option<Address> {
    args.get(index)
        .and_then(|val| Address::try_from_val(e, &val).ok())
}
