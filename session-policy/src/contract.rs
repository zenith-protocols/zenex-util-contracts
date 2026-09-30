//! # Session Policy Contract
//!
//! A policy for trading session keys. The frontend registers an ed25519
//! session key on the smart account under a `Default` context rule with a
//! `valid_until`, so trades sign without a passkey prompt until the rule
//! expires. The account calls `enforce` once for every auth context the key
//! signs, and this policy lets through only:
//!
//! - calls to `allowed_contracts` (the markets and the router), and
//! - `transfer` and `approve` on `token` (the collateral), within a budget.
//!
//! Everything else fails closed: any other contract (the wallet itself, other
//! tokens, the vault), any other token function, and every non-contract
//! context.
//!
//! ## Budget
//!
//! `spend_limit` is the budget the user sets for the session. Every
//! `transfer` and `approve` of `token` adds its full amount to `spent`,
//! wherever it goes, and a context that would take `spent` past the limit is
//! rejected. Destinations are never decoded, so muxed addresses and
//! third-party allowances count like any other amount.
//!
//! A stolen key can trade on the allowed markets, which pay out only to the
//! wallet, and move at most the remaining budget out of the wallet. It cannot
//! touch other tokens, change the wallet's signers or rules, or upgrade it.
//!
//! Trade-offs:
//! - The budget counts what the key commits (margin, execution fees, the
//!   relay fee cap), not the net loss.
//! - Closing a position does not refill the budget; a new session starts a
//!   new one.
//! - An `approve` counts at its full amount, although the router spends at
//!   most the fee and wipes the rest.
use soroban_sdk::{
    auth::{Context, ContractContext},
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    Env, TryFromVal, Vec,
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
pub struct SessionConfig {
    /// The contracts the key may call: the markets and the router.
    pub allowed_contracts: Vec<Address>,
    /// The only token the key may move: the collateral.
    pub token: Address,
    /// The session budget: the most `token` the key may transfer or approve
    /// in total (token-dec).
    pub spend_limit: i128,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub config: SessionConfig,
    /// The `token` the key has transferred or approved so far (token-dec).
    pub spent: i128,
}

#[contracttype]
enum StorageKey {
    Session(Address, u32), // (smart_account, context_rule_id)
}

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SessionPolicyError {
    ContractNotAllowed = 4001,
    // 4002 was v1's TransferDestinationNotAllowed. v2 caps amounts instead
    // of checking destinations, so the code is retired.
    AlreadyInstalled = 4003,
    NotInstalled = 4004,
    EmptyAllowedContracts = 4005,
    FunctionNotAllowed = 4006,
    SpendLimitExceeded = 4007,
    InvalidSpendLimit = 4008,
    InvalidAmount = 4009,
}

// ==========================================
// Constants
// ==========================================

const EXTEND_AMOUNT: u32 = 30 * 17280; // ~30 days
const TTL_THRESHOLD: u32 = EXTEND_AMOUNT - 17280; // refresh at ~29 days

// ==========================================
// Contract
// ==========================================

#[contract]
pub struct SessionPolicyContract;

#[contractimpl]
impl Policy for SessionPolicyContract {
    type AccountParams = SessionConfig;

    fn enforce(
        e: &Env,
        context: Context,
        _authenticated_signers: Vec<Signer>,
        context_rule: ContextRule,
        smart_account: Address,
    ) {
        smart_account.require_auth();

        let key = StorageKey::Session(smart_account, context_rule.id);
        let mut session = read_session(e, &key);
        e.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, EXTEND_AMOUNT);

        // Only contract calls. Contract creation cannot match an allowed
        // contract.
        let Context::Contract(ContractContext {
            contract,
            fn_name,
            args,
        }) = context
        else {
            panic_with_error!(e, SessionPolicyError::ContractNotAllowed);
        };

        if contract == session.config.token {
            // The token leaves only by `transfer(from, to, amount)` or
            // `approve(from, spender, amount, expiration_ledger)`. Both carry
            // the amount third, and every unit counts wherever it goes.
            if fn_name != symbol_short!("transfer") && fn_name != symbol_short!("approve") {
                panic_with_error!(e, SessionPolicyError::FunctionNotAllowed);
            }
            let amount = args
                .get(2)
                .and_then(|val| i128::try_from_val(e, &val).ok())
                .filter(|amount| *amount >= 0)
                .unwrap_or_else(|| panic_with_error!(e, SessionPolicyError::InvalidAmount));
            session.spent = session
                .spent
                .checked_add(amount)
                .filter(|spent| *spent <= session.config.spend_limit)
                .unwrap_or_else(|| panic_with_error!(e, SessionPolicyError::SpendLimitExceeded));
            e.storage().persistent().set(&key, &session);
        } else if !session.config.allowed_contracts.contains(&contract) {
            panic_with_error!(e, SessionPolicyError::ContractNotAllowed);
        }
    }

    fn install(
        e: &Env,
        config: Self::AccountParams,
        context_rule: ContextRule,
        smart_account: Address,
    ) {
        smart_account.require_auth();

        if config.allowed_contracts.is_empty() {
            panic_with_error!(e, SessionPolicyError::EmptyAllowedContracts);
        }
        // The key never reaches the wallet itself: no signer, rule or
        // upgrade calls.
        if config.allowed_contracts.contains(&smart_account) || config.token == smart_account {
            panic_with_error!(e, SessionPolicyError::ContractNotAllowed);
        }
        if config.spend_limit <= 0 {
            panic_with_error!(e, SessionPolicyError::InvalidSpendLimit);
        }

        let key = StorageKey::Session(smart_account, context_rule.id);

        if e.storage().persistent().has(&key) {
            panic_with_error!(e, SessionPolicyError::AlreadyInstalled);
        }

        e.storage()
            .persistent()
            .set(&key, &Session { config, spent: 0 });
        e.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, EXTEND_AMOUNT);
    }

    fn uninstall(e: &Env, context_rule: ContextRule, smart_account: Address) {
        smart_account.require_auth();

        let key = StorageKey::Session(smart_account, context_rule.id);

        if !e.storage().persistent().has(&key) {
            panic_with_error!(e, SessionPolicyError::NotInstalled);
        }

        e.storage().persistent().remove(&key);
    }
}

#[contractimpl]
impl SessionPolicyContract {
    /// Returns the session installed for `(smart_account, context_rule_id)`:
    /// its config and what the key has spent, so the UI can show the
    /// remaining budget.
    pub fn get_session(e: Env, smart_account: Address, context_rule_id: u32) -> Session {
        read_session(&e, &StorageKey::Session(smart_account, context_rule_id))
    }
}

// ==========================================
// Helpers
// ==========================================

fn read_session(e: &Env, key: &StorageKey) -> Session {
    e.storage()
        .persistent()
        .get(key)
        .unwrap_or_else(|| panic_with_error!(e, SessionPolicyError::NotInstalled))
}
