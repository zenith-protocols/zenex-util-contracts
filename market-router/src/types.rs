//! Router value types: the generic call descriptor.

use soroban_sdk::{contracttype, Address, Symbol, Val, Vec};

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
