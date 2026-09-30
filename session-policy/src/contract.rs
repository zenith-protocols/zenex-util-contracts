//! # Session Policy Contract
//!
//! A policy that restricts a smart account signer (e.g. a passkey) to only
//! interact with whitelisted contracts, and ensures token transfers can only
//! flow to a single allowed destination.
//!
//! This solves the DeFi composability problem where `open_position` triggers a
//! sub-auth for `token.transfer`. Without this policy, allowing the token
//! contract would let the signer drain funds to any address. With it, transfers
//! are locked to the trading contract.
//!
//! ## Setup
//!
//! ```text
//! Rule 0 (Default, "owner")   — owner keypair, no policies, full access
//! Rule 1 (Default, "session") — passkey + SessionPolicy, restricted
//! ```
//!
//! The owner bypasses this policy entirely (rule 0 has no policies).
//! The passkey's rule 1 enforces the session policy on every auth context.
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
    pub allowed_contracts: Vec<Address>,
    pub allowed_transfer_to: Address,
}

#[contracttype]
enum StorageKey {
    Config(Address, u32), // (smart_account, context_rule_id)
}

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SessionPolicyError {
    ContractNotAllowed = 4001,
    TransferDestinationNotAllowed = 4002,
    AlreadyInstalled = 4003,
    NotInstalled = 4004,
    EmptyAllowedContracts = 4005,
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

        let config = get_config(e, context_rule.id, &smart_account);

        match &context {
            Context::Contract(ContractContext {
                contract,
                fn_name,
                args,
                ..
            }) => {
                // 1. Called contract must be in the whitelist
                if !config.allowed_contracts.contains(contract) {
                    panic_with_error!(e, SessionPolicyError::ContractNotAllowed);
                }

                // 2. For token transfers, the destination must match
                if fn_name == &symbol_short!("transfer") {
                    if let Some(to_val) = args.get(1) {
                        if let Ok(to) = Address::try_from_val(e, &to_val) {
                            if to != config.allowed_transfer_to {
                                panic_with_error!(
                                    e,
                                    SessionPolicyError::TransferDestinationNotAllowed
                                );
                            }
                        }
                    }
                }
            }
            _ => {
                // Non-contract contexts (e.g. CreateContract) are blocked —
                // they cannot match any allowed contract address.
                panic_with_error!(e, SessionPolicyError::ContractNotAllowed);
            }
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

        let key = StorageKey::Config(smart_account.clone(), context_rule.id);

        if e.storage().persistent().has(&key) {
            panic_with_error!(e, SessionPolicyError::AlreadyInstalled);
        }

        e.storage().persistent().set(&key, &config);
        e.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, EXTEND_AMOUNT);
    }

    fn uninstall(e: &Env, context_rule: ContextRule, smart_account: Address) {
        smart_account.require_auth();

        let key = StorageKey::Config(smart_account.clone(), context_rule.id);

        if !e.storage().persistent().has(&key) {
            panic_with_error!(e, SessionPolicyError::NotInstalled);
        }

        e.storage().persistent().remove(&key);
    }
}

// ==========================================
// Helpers
// ==========================================

fn get_config(e: &Env, context_rule_id: u32, smart_account: &Address) -> SessionConfig {
    let key = StorageKey::Config(smart_account.clone(), context_rule_id);

    e.storage()
        .persistent()
        .extend_ttl(&key, TTL_THRESHOLD, EXTEND_AMOUNT);

    e.storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| panic_with_error!(e, SessionPolicyError::NotInstalled))
}
