use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

mod solana;
pub use solana::*;

mod stellar;
pub use stellar::*;

mod evm;
pub use evm::*;

mod midnight;
pub use midnight::*;

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(untagged)]
pub enum NetworkRpcResult {
    Solana(SolanaRpcResult),
    Stellar(StellarRpcResult),
    Evm(EvmRpcResult),
    Midnight(MidnightRpcResult),
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(untagged)]
#[serde(deny_unknown_fields)]
pub enum NetworkRpcRequest {
    Solana(SolanaRpcRequest),
    Stellar(StellarRpcRequest),
    Evm(EvmRpcRequest),
    Midnight(MidnightRpcRequest),
}
