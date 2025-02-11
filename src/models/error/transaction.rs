use crate::jobs::JobProducerError;

use super::{ApiError, RepositoryError};
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
}

impl From<TransactionError> for ApiError {
    fn from(error: TransactionError) -> Self {
        match error {
            TransactionError::ValidationError(msg) => ApiError::BadRequest(msg),
            TransactionError::NetworkConfiguration(msg) => ApiError::InternalError(msg),
            TransactionError::JobProducerError(msg) => ApiError::InternalError(msg.to_string()),
            TransactionError::InvalidType(msg) => ApiError::InternalError(msg),
        }
    }
}

impl From<RepositoryError> for TransactionError {
    fn from(error: RepositoryError) -> Self {
        TransactionError::ValidationError(error.to_string())
    }
}
