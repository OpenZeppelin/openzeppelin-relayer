use crate::models::{ApiError, NetworkType};
use serde::{Deserialize, Serialize};

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
    pub gas_limit: u128,
    pub gas_price: u128,
    pub speed: Option<Speed>,
}

#[derive(Deserialize, Serialize)]
pub struct SolanaTransactionRequest {
    pub fee_payer: String,
    pub instructions: Vec<String>,
}

#[derive(Deserialize, Serialize)]
pub struct StellarTransactionRequest {
    pub source_account: String,
    pub destination_account: String,
    pub amount: String,
    pub asset_code: String,
    pub asset_issuer: Option<String>,
    pub memo: Option<String>,
    pub fee: u128,
    pub sequence_number: String,
}

#[derive(Serialize)]
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
            NetworkType::Evm => Ok(Self::Evm(
                serde_json::from_value(json).map_err(|e| ApiError::BadRequest(e.to_string()))?,
            )),
            NetworkType::Solana => Ok(Self::Solana(
                serde_json::from_value(json).map_err(|e| ApiError::BadRequest(e.to_string()))?,
            )),
            NetworkType::Stellar => Ok(Self::Stellar(
                serde_json::from_value(json).map_err(|e| ApiError::BadRequest(e.to_string()))?,
            )),
        }
    }
}
