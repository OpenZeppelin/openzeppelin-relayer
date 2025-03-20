use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(untagged)]
pub enum EvmRpcResult {
    GenericRpcResult(String),
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum EvmRpcRequest {
    GenericRpcRequest(String),
}
