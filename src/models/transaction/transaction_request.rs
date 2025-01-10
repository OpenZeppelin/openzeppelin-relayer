use serde::{Deserialize, Serialize};
use std::convert::TryFrom;

use crate::models::{ApiError, NetworkType};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Speed {
    Fastest,
    Fast,
    Average,
    Slow,
}

#[derive(Deserialize, Serialize)]
pub struct EvmTransactionRequest {
    pub to: String,
    pub value: u64,
    pub data: String,
    pub gas_limit: u64,
    pub gas_price: u64,
    pub speed: Option<Speed>,
}

impl TryFrom<serde_json::Value> for EvmTransactionRequest {
    type Error = ApiError;

    fn try_from(value: serde_json::Value) -> Result<Self, Self::Error> {
        serde_json::from_value(value).map_err(|e| ApiError::BadRequest(e.to_string()))
    }
}

#[derive(Deserialize, Serialize)]
pub struct SolanaTransactionRequest {
    pub to: String,
    pub lamports: u64,
    pub recent_blockhash: String,
}

impl TryFrom<serde_json::Value> for SolanaTransactionRequest {
    type Error = ApiError;

    fn try_from(value: serde_json::Value) -> Result<Self, Self::Error> {
        serde_json::from_value(value).map_err(|e| ApiError::BadRequest(e.to_string()))
    }
}

#[derive(Deserialize, Serialize)]
pub struct StellarTransactionRequest {
    pub source_account: String,
    pub destination_account: String,
    pub amount: String,
    pub asset_code: String,
    pub asset_issuer: Option<String>,
    pub memo: Option<String>,
    pub fee: u32,
    pub sequence_number: String,
}

impl TryFrom<serde_json::Value> for StellarTransactionRequest {
    type Error = ApiError;

    fn try_from(value: serde_json::Value) -> Result<Self, Self::Error> {
        serde_json::from_value(value).map_err(|e| ApiError::BadRequest(e.to_string()))
    }
}

pub enum NetworkTransactionRequest {
    Evm(EvmTransactionRequest),
    Solana(SolanaTransactionRequest),
    Stellar(StellarTransactionRequest),
}

impl NetworkTransactionRequest {
    pub fn from_json(
        network_type: &NetworkType,
        json: serde_json::Value,
    ) -> Result<Self, ApiError> {
        match network_type {
            NetworkType::Evm => Ok(Self::Evm(EvmTransactionRequest::try_from(json)?)),
            NetworkType::Solana => Ok(Self::Solana(SolanaTransactionRequest::try_from(json)?)),
            NetworkType::Stellar => Ok(Self::Stellar(StellarTransactionRequest::try_from(json)?)),
        }
    }
}
