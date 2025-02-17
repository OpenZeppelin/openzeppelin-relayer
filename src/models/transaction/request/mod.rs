pub mod evm;
pub mod solana;
pub mod stellar;

use crate::models::{ApiError, NetworkType};
use serde::Serialize;

pub use evm::EvmTransactionRequest;
pub use solana::SolanaTransactionRequest;
pub use stellar::StellarTransactionRequest;

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

    pub fn validate(&self) -> Result<(), ApiError> {
        match self {
            NetworkTransactionRequest::Evm(request) => {
                evm::EvmTransactionRequest::validate_evm_transaction_request(request)
            }
            _ => Ok(()),
        }
    }
}
