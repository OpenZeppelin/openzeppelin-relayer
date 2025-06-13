use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(untagged)]
pub enum EvmRpcResult {
    GenericRpcResult(String),
    RawRpcResult(serde_json::Value),
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(untagged)]
pub enum EvmRpcRequest {
    GenericRpcRequest {
        method: String,
        params: String,
    },
    RawRpcRequest {
        method: String,
        params: serde_json::Value,
    },
}
