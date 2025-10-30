use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(untagged)]
pub enum StellarRpcResult {
    GenericRpcResult(String),
    RawRpcResult(serde_json::Value),
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum StellarRpcRequest {
    /// Generic request where params can be any JSON value (string or structured).
    GenericRpcRequest {
        method: String,
        params: serde_json::Value,
    },
}
