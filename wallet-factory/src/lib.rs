#![no_std]

//! Wallet factory: deploys the pinned smart-account wasm at an address bound
//! to its constructor arguments. See README.md.

#[cfg(test)]
mod test;

use soroban_sdk::{
    contract, contractimpl, symbol_short, unwrap::UnwrapOptimized, xdr::ToXdr, Address, BytesN,
    Env, IntoVal, Map, Symbol, Val, Vec,
};
use stellar_accounts::smart_account::Signer;

/// The smart-account wasm every wallet runs: a `BytesN<32>`.
const WASM_HASH: Symbol = symbol_short!("wasm");

const EXTEND_AMOUNT: u32 = 30 * 17280; // ~30 days
const TTL_THRESHOLD: u32 = EXTEND_AMOUNT - 17280; // refresh at ~29 days

#[contract]
pub struct WalletFactoryContract;

#[contractimpl]
impl WalletFactoryContract {
    /// Pins the smart-account wasm, which must already be uploaded. There is
    /// no way to change it: a new wasm is a new factory.
    pub fn __constructor(e: Env, account_wasm_hash: BytesN<32>) {
        e.storage().instance().set(&WASM_HASH, &account_wasm_hash);
    }

    /// Deploys a wallet with `(signers, policies)` as its constructor
    /// arguments and returns its address. The salt is the SHA-256 of their
    /// XDR, so the address commits to the wallet's signers: anyone may call
    /// this, and a second deploy of the same arguments fails.
    pub fn deploy(e: Env, signers: Vec<Signer>, policies: Map<Address, Val>) -> Address {
        let args: Vec<Val> = (signers.clone(), policies.clone()).into_val(&e);
        let salt = e.crypto().sha256(&args.to_xdr(&e)).to_bytes();
        let instance = e.storage().instance();
        instance.extend_ttl(TTL_THRESHOLD, EXTEND_AMOUNT);
        let wasm_hash: BytesN<32> = instance.get(&WASM_HASH).unwrap_optimized();
        e.deployer()
            .with_current_contract(salt)
            .deploy_v2(wasm_hash, (signers, policies))
    }

    /// Returns the address `deploy` creates for these arguments, whether or
    /// not the wallet exists yet.
    pub fn predict_address(e: Env, signers: Vec<Signer>, policies: Map<Address, Val>) -> Address {
        let args: Vec<Val> = (signers, policies).into_val(&e);
        let salt = e.crypto().sha256(&args.to_xdr(&e)).to_bytes();
        e.deployer().with_current_contract(salt).deployed_address()
    }
}
