extern crate std;
use super::*;
use soroban_sdk::{
    testutils::{Address as _, AuthorizedFunction, AuthorizedInvocation, Events},
    Address, Env, Event as _, IntoVal, Symbol,
};

#[test]
fn attribute_returns_nothing() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(ReferralContract, ());
    let client = ReferralContractClient::new(&env, &contract_id);

    let caller = Address::generate(&env);
    let referrer = Address::generate(&env);

    // The call succeeds with a unit result: the event is the only output.
    let () = client.attribute(&caller, &referrer);
}

#[test]
fn attribute_requires_caller_auth() {
    let env = Env::default();
    let contract_id = env.register(ReferralContract, ());
    let client = ReferralContractClient::new(&env, &contract_id);

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
    let contract_id = env.register(ReferralContract, ());
    let client = ReferralContractClient::new(&env, &contract_id);

    let caller = Address::generate(&env);

    // caller == referrer must error with ReferralError::SelfReferral, 7001.
    let self_referral = soroban_sdk::Error::from_contract_error(7001);
    assert_eq!(
        soroban_sdk::Error::from(ReferralError::SelfReferral),
        self_referral
    );
    assert_eq!(
        client.try_attribute(&caller, &caller),
        Err(Ok(self_referral))
    );
}

#[test]
fn attribute_emits_event_with_indexable_topics() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(ReferralContract, ());
    let client = ReferralContractClient::new(&env, &contract_id);

    let caller = Address::generate(&env);
    let referrer = Address::generate(&env);
    client.attribute(&caller, &referrer);

    let expected = Attributed {
        referee: caller.clone(),
        referrer: referrer.clone(),
    }
    .to_xdr(&env, &contract_id);
    assert_eq!(
        env.events().all().filter_by_contract(&contract_id),
        [expected],
        "exactly one event, topics (\"attributed\", caller, referrer)"
    );
}

#[test]
fn attribute_records_required_auth_in_auths() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(ReferralContract, ());
    let client = ReferralContractClient::new(&env, &contract_id);

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
