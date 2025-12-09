use alloy::network::{AnyRpcBlock, AnyTransactionReceipt};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(untagged)]
pub enum EvmRpcResult {
    /// Raw JSON-RPC response value. Covers string, array or object results.
    RawRpcResult(serde_json::Value),
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq, Clone)]
#[serde(untagged)]
pub enum EvmRpcRequest {
    /// Unified raw request variant where params may be a JSON string or structured JSON value.
    RawRpcRequest {
        method: String,
        params: serde_json::Value,
    },
}

pub type BlockResponse = AnyRpcBlock;
pub type TransactionReceipt = AnyTransactionReceipt;
