#![no_std]

//! Market router: stateless batching in one transaction, plus the Zenex
//! create-and-fill flows. See README.md.

#[cfg(test)]
mod test;

use soroban_sdk::{
    contract, contractimpl, contracttype, vec, Address, Bytes, Env, Error, IntoVal, Symbol,
    TryFromVal, Val, Vec,
};

/// One contract invocation in a batch.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Call {
    /// The target contract.
    pub contract: Address,
    /// The entry-point name.
    pub func: Symbol,
    /// The positional arguments, host-encoded.
    pub args: Vec<Val>,
}

/// Stateless call router. It owns nothing and holds nothing.
#[contract]
pub struct RouterContract;

#[contractimpl]
impl RouterContract {
    /// Executes `calls` in order and returns each call's raw return value. Any
    /// failing call traps the whole invocation, so every call lands or none
    /// does.
    ///
    /// # Arguments
    ///
    /// * `e` - Access to the Soroban environment.
    /// * `calls` - The calls, executed front to back.
    pub fn multicall(e: Env, calls: Vec<Call>) -> Vec<Val> {
        let mut results = Vec::new(&e);
        for call in calls.iter() {
            results.push_back(e.invoke_contract::<Val>(&call.contract, &call.func, call.args));
        }
        results
    }

    /// Executes `calls` in order and returns one outcome per call: the raw
    /// return value when it lands, or the failure as a host `Error` value. A
    /// failing call rolls back only its own effects; a budget or footprint
    /// limit still aborts the whole transaction.
    ///
    /// # Arguments
    ///
    /// * `e` - Access to the Soroban environment.
    /// * `calls` - The calls, executed front to back.
    pub fn multicall_try(e: Env, calls: Vec<Call>) -> Vec<Val> {
        let mut outcomes = Vec::new(&e);
        for call in calls.iter() {
            outcomes.push_back(
                match e.try_invoke_contract::<Val, Error>(&call.contract, &call.func, call.args) {
                    Ok(Ok(value)) => value,
                    Ok(Err(conversion)) => Error::from(conversion).into(),
                    Err(Ok(error)) => error.into(),
                    Err(Err(_)) => unreachable!(),
                },
            );
        }
        outcomes
    }

    /// Runs `calls` like `multicall`, then fills the order `calls[0]` created
    /// and returns the batch results with the fill payout appended. `calls[0]`
    /// must target the market and return the `u32` order id; a failing fill
    /// unwinds the whole batch.
    ///
    /// # Arguments
    ///
    /// * `e` - Access to the Soroban environment.
    /// * `calls` - The batch; `calls[0]` creates the order to fill.
    /// * `user` - The order owner, passed to `execute_order`.
    /// * `keeper` - The fill-reward recipient.
    /// * `price` - The price update for the fill.
    pub fn create_and_fill(
        e: Env,
        calls: Vec<Call>,
        user: Address,
        keeper: Address,
        price: Bytes,
    ) -> Vec<Val> {
        let market = calls.get(0).unwrap().contract;
        let mut results = Self::multicall(e.clone(), calls);
        let id = u32::try_from_val(&e, &results.get(0).unwrap()).unwrap();
        let payout: i128 = e.invoke_contract(
            &market,
            &Symbol::new(&e, "execute_order"),
            vec![
                &e,
                keeper.into_val(&e),
                user.into_val(&e),
                id.into_val(&e),
                price.into_val(&e),
            ],
        );
        results.push_back(payout.into_val(&e));
        results
    }

    /// Runs `calls` like `multicall`, then attempts to fill the order
    /// `calls[0]` created and returns the batch results with the fill outcome
    /// appended: the payout on a fill, or the failure as a host `Error` value,
    /// in which case the orders rest for a later keeper fill.
    ///
    /// # Arguments
    ///
    /// * `e` - Access to the Soroban environment.
    /// * `calls` - The batch; `calls[0]` creates the order to fill.
    /// * `user` - The order owner, passed to `execute_order`.
    /// * `keeper` - The fill-reward recipient.
    /// * `price` - The price update for the fill.
    pub fn create_and_try_fill(
        e: Env,
        calls: Vec<Call>,
        user: Address,
        keeper: Address,
        price: Bytes,
    ) -> Vec<Val> {
        let market = calls.get(0).unwrap().contract;
        let mut results = Self::multicall(e.clone(), calls);
        let id = u32::try_from_val(&e, &results.get(0).unwrap()).unwrap();
        results.push_back(
            match e.try_invoke_contract::<i128, Error>(
                &market,
                &Symbol::new(&e, "execute_order"),
                vec![
                    &e,
                    keeper.into_val(&e),
                    user.into_val(&e),
                    id.into_val(&e),
                    price.into_val(&e),
                ],
            ) {
                Ok(Ok(payout)) => payout.into_val(&e),
                Ok(Err(error)) | Err(Ok(error)) => error.into(),
                Err(Err(_)) => unreachable!(),
            },
        );
        results
    }
}
