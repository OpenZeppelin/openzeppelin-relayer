use crate::{
    repositories::TransactionCounterError,
    services::{SignerError, SignerFactoryError},
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
        }
    }
}

impl From<RepositoryError> for RelayerError {
    fn from(error: RepositoryError) -> Self {
        RelayerError::NetworkConfiguration(error.to_string())
    }
}
