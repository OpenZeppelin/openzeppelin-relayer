use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(untagged)]
pub enum MidnightRpcResult {
    GenericRpcResult(String),
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum MidnightRpcRequest {
    GenericRpcRequest(String),
}
