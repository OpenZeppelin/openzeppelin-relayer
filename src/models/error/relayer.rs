use crate::{
    models::{SignerError, SignerFactoryError},
    repositories::TransactionCounterError,
    services::{ProviderError, SolanaProviderError},
};

use super::{ApiError, RepositoryError};
use serde::Serialize;
use thiserror::Error;

#[derive(Error, Debug, Serialize)]
pub enum RelayerError {
    #[error("Network configuration error: {0}")]
    NetworkConfiguration(String),
    #[error("Provider error: {0}")]
    ProviderError(String),
    #[error("Underlying provider error: {0}")]
    UnderlyingProvider(#[from] ProviderError),
    #[error("Underlying Solana provider error: {0}")]
    UnderlyingSolanaProvider(#[from] SolanaProviderError),
    #[error("Queue error: {0}")]
    QueueError(String),
    #[error("Signer factory error: {0}")]
    SignerFactoryError(#[from] SignerFactoryError),
    #[error("Signer error: {0}")]
    SignerError(#[from] SignerError),
    #[error("Not supported: {0}")]
    NotSupported(String),
    #[error("Relayer is disabled")]
    RelayerDisabled,
    #[error("Relayer is paused")]
    RelayerPaused,
    #[error("Transaction sequence error: {0}")]
    TransactionSequenceError(#[from] TransactionCounterError),
    #[error("Insufficient balance error: {0}")]
    InsufficientBalanceError(String),
    #[error("Relayer Policy configuration error: {0}")]
    PolicyConfigurationError(String),
}

impl From<RelayerError> for ApiError {
    fn from(error: RelayerError) -> Self {
        match error {
            RelayerError::NetworkConfiguration(msg) => ApiError::InternalError(msg),
            RelayerError::ProviderError(msg) => ApiError::InternalError(msg),
            RelayerError::QueueError(msg) => ApiError::InternalError(msg),
            RelayerError::SignerError(err) => ApiError::InternalError(err.to_string()),
            RelayerError::SignerFactoryError(err) => ApiError::InternalError(err.to_string()),
            RelayerError::NotSupported(msg) => ApiError::BadRequest(msg),
            RelayerError::RelayerDisabled => {
                ApiError::ForbiddenError("Relayer disabled".to_string())
            }
            RelayerError::RelayerPaused => ApiError::ForbiddenError("Relayer paused".to_string()),
            RelayerError::TransactionSequenceError(err) => ApiError::InternalError(err.to_string()),
            RelayerError::InsufficientBalanceError(msg) => ApiError::BadRequest(msg),
            RelayerError::UnderlyingProvider(err) => ApiError::InternalError(err.to_string()),
            RelayerError::UnderlyingSolanaProvider(err) => ApiError::InternalError(err.to_string()),
            RelayerError::PolicyConfigurationError(msg) => ApiError::InternalError(msg),
        }
    }
}

impl From<RepositoryError> for RelayerError {
    fn from(error: RepositoryError) -> Self {
        RelayerError::NetworkConfiguration(error.to_string())
    }
}
