//! Forwarder value types: the router's call descriptor and the fixed config.

use soroban_sdk::{contracttype, Address, Symbol, Val, Vec};

/// One contract invocation in a batch. The same shape as the market router's
/// `Call`, so a batch encodes identically for either contract.
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

/// The settings fixed at deploy.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    /// The market router whose non-fee flows every entry point forwards to.
    pub router: Address,
    /// The token the fee is paid in.
    pub fee_token: Address,
    /// The only address a fee is ever paid to.
    pub fee_recipient: Address,
}
