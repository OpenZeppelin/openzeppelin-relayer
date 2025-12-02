pub mod evm;
pub mod solana;
pub mod stellar;

use crate::models::rpc::{
    SolanaFeeEstimateRequestParams, SolanaPrepareTransactionRequestParams,
    StellarFeeEstimateRequestParams, StellarPrepareTransactionRequestParams,
};
use crate::models::{ApiError, NetworkType, RelayerRepoModel};
use serde::{Deserialize, Serialize};

pub use evm::EvmTransactionRequest;
pub use solana::SolanaTransactionRequest;
pub use stellar::StellarTransactionRequest;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
#[serde(untagged)]
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

    pub fn validate(&self, relayer: &RelayerRepoModel) -> Result<(), ApiError> {
        match self {
            NetworkTransactionRequest::Evm(request) => request.validate(relayer),
            NetworkTransactionRequest::Stellar(request) => request.validate(),
            NetworkTransactionRequest::Solana(request) => request.validate(relayer),
        }
    }
}

/// Network-agnostic fee estimate request parameters for gasless transactions.
/// Contains network-specific request parameters for fee estimation.
/// The network type is inferred from the relayer's network configuration.
#[derive(Debug, Deserialize, Serialize, PartialEq, ToSchema, Clone)]
#[serde(untagged)]
#[schema(as = SponsoredTransactionQuoteRequest)]
pub enum SponsoredTransactionQuoteRequest {
    /// Solana-specific fee estimate request parameters
    Solana(SolanaFeeEstimateRequestParams),
    /// Stellar-specific fee estimate request parameters
    Stellar(StellarFeeEstimateRequestParams),
}

impl SponsoredTransactionQuoteRequest {
    pub fn validate(&self) -> Result<(), ApiError> {
        match self {
            SponsoredTransactionQuoteRequest::Stellar(request) => request.validate(),
            SponsoredTransactionQuoteRequest::Solana(_) => Ok(()),
        }
    }
}

/// Network-agnostic prepare transaction request parameters for gasless transactions.
/// Contains network-specific request parameters for preparing transactions with fee payments.
/// The network type is inferred from the relayer's network configuration.
#[derive(Debug, Deserialize, Serialize, PartialEq, ToSchema, Clone)]
#[serde(untagged)]
#[schema(as = SponsoredTransactionBuildRequest)]
pub enum SponsoredTransactionBuildRequest {
    /// Solana-specific prepare transaction request parameters
    Solana(SolanaPrepareTransactionRequestParams),
    /// Stellar-specific prepare transaction request parameters
    Stellar(StellarPrepareTransactionRequestParams),
}

impl SponsoredTransactionBuildRequest {
    pub fn validate(&self) -> Result<(), ApiError> {
        match self {
            SponsoredTransactionBuildRequest::Stellar(request) => request.validate(),
            SponsoredTransactionBuildRequest::Solana(_) => Ok(()),
        }
    }
}
