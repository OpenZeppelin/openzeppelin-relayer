use crate::{
    jobs::JobProducerError,
    models::{SignerError, SignerFactoryError},
    services::{ProviderError, SolanaProviderError},
};

use super::{ApiError, RepositoryError};
use eyre::Report;
use serde::Serialize;
use thiserror::Error;

#[derive(Error, Debug, Serialize)]
pub enum TransactionError {
    #[error("Transaction validation error: {0}")]
    ValidationError(String),

    #[error("Network configuration error: {0}")]
    NetworkConfiguration(String),

    #[error("Job producer error: {0}")]
    JobProducerError(#[from] JobProducerError),

    #[error("Invalid transaction type: {0}")]
    InvalidType(String),

    #[error("Underlying provider error: {0}")]
    UnderlyingProvider(#[from] ProviderError),

    #[error("Underlying Solana provider error: {0}")]
    UnderlyingSolanaProvider(#[from] SolanaProviderError),

    #[error("Unexpected error: {0}")]
    UnexpectedError(String),

    #[error("Not supported: {0}")]
    NotSupported(String),

    #[error("Signer error: {0}")]
    SignerError(String),

    #[error("Insufficient balance: {0}")]
    InsufficientBalance(String),
}

impl From<TransactionError> for ApiError {
    fn from(error: TransactionError) -> Self {
        match error {
            TransactionError::ValidationError(msg) => ApiError::BadRequest(msg),
            TransactionError::NetworkConfiguration(msg) => ApiError::InternalError(msg),
            TransactionError::JobProducerError(msg) => ApiError::InternalError(msg.to_string()),
            TransactionError::InvalidType(msg) => ApiError::InternalError(msg),
            TransactionError::UnderlyingProvider(err) => ApiError::InternalError(err.to_string()),
            TransactionError::UnderlyingSolanaProvider(err) => {
                ApiError::InternalError(err.to_string())
            }
            TransactionError::NotSupported(msg) => ApiError::BadRequest(msg),
            TransactionError::UnexpectedError(msg) => ApiError::InternalError(msg),
            TransactionError::SignerError(msg) => ApiError::InternalError(msg),
            TransactionError::InsufficientBalance(msg) => ApiError::BadRequest(msg),
        }
    }
}

impl From<RepositoryError> for TransactionError {
    fn from(error: RepositoryError) -> Self {
        TransactionError::ValidationError(error.to_string())
    }
}

impl From<Report> for TransactionError {
    fn from(err: Report) -> Self {
        TransactionError::UnexpectedError(err.to_string())
    }
}

impl From<SignerFactoryError> for TransactionError {
    fn from(error: SignerFactoryError) -> Self {
        TransactionError::SignerError(error.to_string())
    }
}

impl From<SignerError> for TransactionError {
    fn from(error: SignerError) -> Self {
        TransactionError::SignerError(error.to_string())
    }
}
