use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(untagged)]
pub enum StellarRpcResult {
    GenericRpcResult(String),
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum StellarRpcRequest {
    GenericRpcRequest(String),
}
