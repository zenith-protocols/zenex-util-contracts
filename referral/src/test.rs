#![cfg(test)]
extern crate std;
use super::*;
use soroban_sdk::{
    testutils::{Address as _, AuthorizedFunction, AuthorizedInvocation, Events},
    Address, Env, IntoVal, Symbol,
};

#[test]
fn attribute_returns_caller_and_referrer() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SorobanReferral, ());
    let client = SorobanReferralClient::new(&env, &contract_id);

    let caller = Address::generate(&env);
    let referrer = Address::generate(&env);

    let result = client.attribute(&caller, &referrer);

    assert_eq!(result, (caller.clone(), referrer.clone()));
}

#[test]
fn attribute_requires_caller_auth() {
    let env = Env::default();
    let contract_id = env.register(SorobanReferral, ());
    let client = SorobanReferralClient::new(&env, &contract_id);

    let caller = Address::generate(&env);
    let referrer = Address::generate(&env);

    // No mock_all_auths. Calling without auth should panic.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.attribute(&caller, &referrer);
    }));
    assert!(result.is_err(), "attribute must require caller auth");
}

#[test]
fn attribute_rejects_self_referral() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SorobanReferral, ());
    let client = SorobanReferralClient::new(&env, &contract_id);

    let caller = Address::generate(&env);

    // caller == referrer must error with ReferralError::SelfReferral.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.attribute(&caller, &caller);
    }));
    assert!(result.is_err(), "self-referral must be rejected");
}

#[test]
fn attribute_emits_event_with_indexable_topics() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SorobanReferral, ());
    let client = SorobanReferralClient::new(&env, &contract_id);

    let caller = Address::generate(&env);
    let referrer = Address::generate(&env);
    client.attribute(&caller, &referrer);

    // ContractEvents.events() returns &[xdr::ContractEvent]. filter_by_contract
    // narrows to events emitted by us specifically.
    let our_events = env.events().all().filter_by_contract(&contract_id);
    assert_eq!(
        our_events.events().len(),
        1,
        "exactly one event should be emitted by the contract"
    );

    // Topics produced by #[contractevent] macro on the `Attributed` struct:
    //   [0] event name symbol (auto-derived from struct name)
    //   [1] referee  (the #[topic] field)
    //   [2] referrer (the #[topic] field)
    let topics = match &our_events.events()[0].body {
        soroban_sdk::xdr::ContractEventBody::V0(v0) => &v0.topics,
    };
    assert_eq!(
        topics.len(),
        3,
        "expected 3 topics: (event_name, referee, referrer)"
    );
}

#[test]
fn attribute_records_required_auth_in_auths() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SorobanReferral, ());
    let client = SorobanReferralClient::new(&env, &contract_id);

    let caller = Address::generate(&env);
    let referrer = Address::generate(&env);
    client.attribute(&caller, &referrer);

    let auths = env.auths();
    assert_eq!(auths.len(), 1, "exactly one auth required");
    assert_eq!(auths[0].0, caller, "caller must be the auth address");

    let expected_invocation = AuthorizedInvocation {
        function: AuthorizedFunction::Contract((
            contract_id.clone(),
            Symbol::new(&env, "attribute"),
            (caller.clone(), referrer.clone()).into_val(&env),
        )),
        sub_invocations: std::vec![],
    };
    assert_eq!(auths[0].1, expected_invocation);
}
