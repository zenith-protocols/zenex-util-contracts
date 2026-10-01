//! Regression test: a session rule only authorizes what its session key
//! signed.
//!
//! OZ's `do_check_auth` authenticates every signer the payload provides, but
//! for a rule with policies it leaves "did the rule's signers sign?" to the
//! policies' `enforce`. A policy that ignores `authenticated_signers` lets an
//! `AuthPayload` with an empty `signers` map, naming the session rule id,
//! drive the wallet without any key.
//!
//! This suite runs the real canonical wallet (smart-account-kit, OZ v0.7.1,
//! `testdata/smart_account.wasm`, sha256 `1b5f4534…785a`) and ed25519 verifier
//! (`testdata/ed25519_verifier.wasm`, sha256 `60e8798d…b477`, the wasm of the
//! mainnet verifier `CBOOZV2B…UEMC`). It installs a session rule whose only
//! signer is a session key and whose policy is this contract, then authorizes
//! a market call with and without the key's signature.

extern crate std;

use ed25519_dalek::{Signer as _, SigningKey};
use session_policy::SessionPolicyContract;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::xdr::{
    Hash, HashIdPreimage, HashIdPreimageSorobanAuthorization, InvokeContractArgs, Limits,
    ScAddress, ScSymbol, ScVal, SorobanAddressCredentials, SorobanAuthorizationEntry,
    SorobanAuthorizedFunction, SorobanAuthorizedInvocation, SorobanCredentials, ToXdr as _,
    WriteXdr as _,
};
use soroban_sdk::{
    contract, contractimpl, vec, Address, Bytes, Env, IntoVal, Map, String, Symbol, TryFromVal,
    Val, Vec,
};
use stellar_accounts::smart_account::{AuthPayload, ContextRule, ContextRuleType, Signer};

const WALLET_WASM: &[u8] = include_bytes!("../testdata/smart_account.wasm");
const VERIFIER_WASM: &[u8] = include_bytes!("../testdata/ed25519_verifier.wasm");

/// A market whose `cancel_order` requires the wallet's authorization, like the
/// real one. The policy allows it on a configured market.
#[contract]
struct MockMarket;

#[contractimpl]
impl MockMarket {
    pub fn cancel_order(_e: Env, user: Address, id: u32) -> u32 {
        user.require_auth();
        id
    }
}

struct World {
    e: Env,
    wallet: Address,
    verifier: Address,
    market: Address,
    session_key: SigningKey,
    session_rule_id: u32,
}

/// Registers the wallet with an owner rule (rule 0, no policy) and installs a
/// session rule: the session key as its only signer, this policy attached.
fn setup() -> World {
    let e = Env::default();

    let verifier = e.register(VERIFIER_WASM, ());
    let owner = Address::generate(&e);
    let owner_signers: Vec<Signer> = vec![&e, Signer::Delegated(owner)];
    let wallet = e.register(WALLET_WASM, (owner_signers, Map::<Address, Val>::new(&e)));

    let market = e.register(MockMarket, ());
    let policy = e.register(
        SessionPolicyContract,
        (
            Address::generate(&e),
            vec![&e, market.clone()],
            Address::generate(&e),
            Address::generate(&e),
        ),
    );

    let session_key = SigningKey::from_bytes(&[9u8; 32]);
    let session_signer = Signer::External(
        verifier.clone(),
        Bytes::from_array(&e, &session_key.verifying_key().to_bytes()),
    );
    let mut policies: Map<Address, Val> = Map::new(&e);
    policies.set(policy, ().into_val(&e));
    let valid_until: Option<u32> = Some(e.ledger().sequence() + 100_000);

    // The owner adds the session rule; only this call is mocked.
    e.mock_all_auths();
    let rule: ContextRule = e.invoke_contract(
        &wallet,
        &Symbol::new(&e, "add_context_rule"),
        vec![
            &e,
            ContextRuleType::Default.into_val(&e),
            String::from_str(&e, "Trading Session").into_val(&e),
            valid_until.into_val(&e),
            vec![&e, session_signer].into_val(&e),
            policies.into_val(&e),
        ],
    );
    e.set_auths(&[]);

    World {
        e,
        wallet,
        verifier,
        market,
        session_key,
        session_rule_id: rule.id,
    }
}

fn sc_address(e: &Env, address: &Address) -> ScAddress {
    match ScVal::try_from_val(e, &address.to_val()).unwrap() {
        ScVal::Address(address) => address,
        other => panic!("not an address: {other:?}"),
    }
}

/// The authorized invocation `market.cancel_order(wallet, id)`.
fn cancel_invocation(w: &World, id: u32) -> SorobanAuthorizedInvocation {
    let e = &w.e;
    let args: std::vec::Vec<ScVal> = [w.wallet.to_val(), id.into_val(e)]
        .iter()
        .map(|v| ScVal::try_from_val(e, v).unwrap())
        .collect();
    SorobanAuthorizedInvocation {
        function: SorobanAuthorizedFunction::ContractFn(InvokeContractArgs {
            contract_address: sc_address(e, &w.market),
            function_name: ScSymbol("cancel_order".try_into().unwrap()),
            args: args.try_into().unwrap(),
        }),
        sub_invocations: std::vec::Vec::new().try_into().unwrap(),
    }
}

/// The wallet's authorization entry over `root`, carrying `payload`.
fn entry(
    w: &World,
    root: SorobanAuthorizedInvocation,
    payload: AuthPayload,
    nonce: i64,
    expiration: u32,
) -> SorobanAuthorizationEntry {
    let e = &w.e;
    let signature: Val = payload.into_val(e);
    SorobanAuthorizationEntry {
        credentials: SorobanCredentials::Address(SorobanAddressCredentials {
            address: sc_address(e, &w.wallet),
            nonce,
            signature_expiration_ledger: expiration,
            signature: ScVal::try_from_val(e, &signature).unwrap(),
        }),
        root_invocation: root,
    }
}

/// The session key's signature over `root`, as the wallet verifies it: the
/// ed25519 signature of `sha256(payload_hash || xdr(context_rule_ids))`.
fn session_payload(
    w: &World,
    root: &SorobanAuthorizedInvocation,
    nonce: i64,
    expiration: u32,
) -> AuthPayload {
    let e = &w.e;
    let preimage = HashIdPreimage::SorobanAuthorization(HashIdPreimageSorobanAuthorization {
        network_id: Hash(e.ledger().network_id().to_array()),
        nonce,
        signature_expiration_ledger: expiration,
        invocation: root.clone(),
    });
    let payload_hash = e.crypto().sha256(&Bytes::from_slice(
        e,
        &preimage.to_xdr(Limits::none()).unwrap(),
    ));
    let rule_ids: Vec<u32> = vec![e, w.session_rule_id];
    let mut digest = payload_hash.to_bytes().to_bytes();
    digest.append(&rule_ids.clone().to_xdr(e));
    let digest = e.crypto().sha256(&digest).to_array();
    let signature = w.session_key.sign(&digest).to_bytes();

    let signer = Signer::External(
        w.verifier.clone(),
        Bytes::from_array(e, &w.session_key.verifying_key().to_bytes()),
    );
    let mut signers: Map<Signer, Bytes> = Map::new(e);
    signers.set(signer, Bytes::from_array(e, &signature));
    AuthPayload {
        signers,
        context_rule_ids: rule_ids,
    }
}

/// Calls `market.cancel_order(wallet, id)` and returns the panic message if
/// authorization fails.
fn cancel(w: &World, id: u32) -> Result<u32, std::string::String> {
    let client = MockMarketClient::new(&w.e, &w.market);
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.cancel_order(&w.wallet, &id)
    }))
    .map_err(|payload| {
        payload
            .downcast_ref::<std::string::String>()
            .cloned()
            .or_else(|| {
                payload
                    .downcast_ref::<&str>()
                    .map(|s| std::string::String::from(*s))
            })
            .unwrap_or_default()
    })
}

#[test]
fn signed_session_key_authorizes_a_trade() {
    let w = setup();
    let expiration = w.e.ledger().sequence() + 100;
    let root = cancel_invocation(&w, 1);
    let payload = session_payload(&w, &root, 1, expiration);
    w.e.set_auths(&[entry(&w, root, payload, 1, expiration)]);
    assert_eq!(cancel(&w, 1), Ok(1));
}

#[test]
fn missing_authorization_is_rejected() {
    let w = setup();
    w.e.set_auths(&[]);
    assert!(cancel(&w, 1).is_err());
}

#[test]
fn empty_signature_on_the_owner_rule_is_rejected() {
    // Rule 0 has no policy, so the wallet itself requires its signer.
    let w = setup();
    let expiration = w.e.ledger().sequence() + 100;
    let payload = AuthPayload {
        signers: Map::new(&w.e),
        context_rule_ids: vec![&w.e, 0u32],
    };
    w.e.set_auths(&[entry(&w, cancel_invocation(&w, 1), payload, 2, expiration)]);
    assert!(cancel(&w, 1).is_err());
}

#[test]
fn empty_signature_on_the_session_rule_is_rejected_by_the_policy() {
    // The bypass: no signature at all, naming the session rule id, over a
    // context the policy allows. The wallet defers to the policy, which must
    // reject it with `SignerNotAuthenticated` (4007).
    let w = setup();
    let expiration = w.e.ledger().sequence() + 100;
    let payload = AuthPayload {
        signers: Map::new(&w.e),
        context_rule_ids: vec![&w.e, w.session_rule_id],
    };
    w.e.set_auths(&[entry(&w, cancel_invocation(&w, 1), payload, 3, expiration)]);
    let error = cancel(&w, 1).expect_err("an empty signature must not authorize a trade");
    assert!(
        error.contains("Error(Contract, #4007)"),
        "expected the policy's SignerNotAuthenticated (4007), got: {error}"
    );
}
