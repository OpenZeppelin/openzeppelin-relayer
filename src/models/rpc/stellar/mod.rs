use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// feeEstimate
#[derive(Debug, Deserialize, Serialize, PartialEq, ToSchema)]
#[serde(deny_unknown_fields)]
#[derive(Clone)]
pub struct FeeEstimateRequestParams {
    /// Transaction XDR (base64 encoded) or operations array
    pub transaction: serde_json::Value,
    /// Asset identifier for fee token (e.g., "native" or "USDC:GA5Z...")
    pub fee_token: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, ToSchema)]
pub struct FeeEstimateResult {
    /// Estimated fee in token amount (as string for precision)
    pub estimated_fee: String,
    /// Conversion rate from XLM to token (as string)
    pub conversion_rate: String,
}

// prepareTransaction
#[derive(Debug, Deserialize, Serialize, PartialEq, ToSchema)]
#[serde(deny_unknown_fields)]
#[derive(Clone)]
pub struct PrepareTransactionRequestParams {
    /// Transaction XDR (base64 encoded) or operations array
    pub transaction: serde_json::Value,
    /// Asset identifier for fee token
    pub fee_token: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, ToSchema)]
pub struct PrepareTransactionResult {
    /// Extended transaction XDR (base64 encoded)
    pub transaction: String,
    /// Fee amount in token (as string)
    pub fee_in_token: String,
    /// Fee amount in stroops (as string)
    pub fee_in_stroops: String,
    /// Asset identifier for fee token
    pub fee_token: String,
    /// Transaction validity timestamp (ISO 8601 format)
    pub valid_until: String,
}

/// Stellar RPC method enum
pub enum StellarRpcMethod {
    Generic(String),
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq, Clone)]
#[serde(untagged)]
#[schema(as = StellarRpcRequest)]
pub enum StellarRpcRequest {
    #[serde(rename = "rawRpcRequest")]
    #[schema(example = "rawRpcRequest")]
    RawRpcRequest {
        method: String,
        params: serde_json::Value,
    },
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(untagged)]
pub enum StellarRpcResult {
    /// Raw JSON-RPC response value. Covers string or structured JSON values.
    RawRpcResult(serde_json::Value),
}
