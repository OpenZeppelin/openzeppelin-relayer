/// The `evm` module provides functionality for interacting with
/// Ethereum Virtual Machine (EVM) based blockchains. It includes
/// the `evm_relayer` submodule which contains the core logic for
/// relaying transactions and events between different EVM networks.
mod evm_relayer;
mod rpc_utils;
mod validations;

pub use evm_relayer::*;
pub use rpc_utils::*;
pub use validations::*;
