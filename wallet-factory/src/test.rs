extern crate std;

use soroban_sdk::{vec, Address, Bytes, BytesN, Env, IntoVal, Map, Symbol, Val, Vec};
use stellar_accounts::smart_account::{ContextRule, Signer};

use crate::{WalletFactoryContract, WalletFactoryContractClient};

/// The canonical smart-account wasm (smart-account-kit, OZ v0.7.1) and the
/// ed25519 verifier it canonicalizes external keys with.
const WALLET_WASM: &[u8] = include_bytes!("../../session-policy/testdata/smart_account.wasm");
const VERIFIER_WASM: &[u8] = include_bytes!("../../session-policy/testdata/ed25519_verifier.wasm");

fn setup(e: &Env) -> (WalletFactoryContractClient<'_>, Address) {
    let wasm_hash: BytesN<32> = e
        .deployer()
        .upload_contract_wasm(Bytes::from_slice(e, WALLET_WASM));
    let factory = e.register(WalletFactoryContract, (wasm_hash,));
    let verifier = e.register(VERIFIER_WASM, ());
    (WalletFactoryContractClient::new(e, &factory), verifier)
}

/// An external signer like the backend's passkey: a verifier plus key data.
fn signer(e: &Env, verifier: &Address, byte: u8) -> Vec<Signer> {
    vec![
        e,
        Signer::External(verifier.clone(), Bytes::from_array(e, &[byte; 32])),
    ]
}

#[test]
fn deploy_creates_the_wallet_at_the_predicted_address() {
    let e = Env::default();
    let (factory, verifier) = setup(&e);
    let signers = signer(&e, &verifier, 1);
    let policies = Map::<Address, Val>::new(&e);

    let predicted = factory.predict_address(&signers, &policies);
    let wallet = factory.deploy(&signers, &policies);
    assert_eq!(wallet, predicted);

    // The wallet's default rule (id 0) holds exactly the passed signers.
    let rule: ContextRule = e.invoke_contract(
        &wallet,
        &Symbol::new(&e, "get_context_rule"),
        vec![&e, 0u32.into_val(&e)],
    );
    assert_eq!(rule.signers, signers);
}

#[test]
fn different_signers_give_a_different_address() {
    let e = Env::default();
    let (factory, verifier) = setup(&e);
    let policies = Map::<Address, Val>::new(&e);

    assert_ne!(
        factory.predict_address(&signer(&e, &verifier, 1), &policies),
        factory.predict_address(&signer(&e, &verifier, 2), &policies)
    );
}

#[test]
fn deploying_the_same_signers_twice_fails() {
    let e = Env::default();
    let (factory, verifier) = setup(&e);
    let signers = signer(&e, &verifier, 1);
    let policies = Map::<Address, Val>::new(&e);

    factory.deploy(&signers, &policies);
    assert!(factory.try_deploy(&signers, &policies).is_err());
}
