use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(untagged)]
pub enum StellarRpcResult {
    /// Raw JSON-RPC response value. Covers string or structured JSON values.
    RawRpcResult(serde_json::Value),
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(untagged)]
pub enum StellarRpcRequest {
    /// Raw request where params can be any JSON value (string or structured).
    RawRpcRequest {
        method: String,
        params: serde_json::Value,
    },
}
