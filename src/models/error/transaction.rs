use super::{ApiError, RepositoryError};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum TransactionError {
    #[error("Transaction validation error: {0}")]
    ValidationError(String),

    #[error("Network configuration error: {0}")]
    NetworkConfiguration(String),
}

impl From<TransactionError> for ApiError {
    fn from(error: TransactionError) -> Self {
        match error {
            TransactionError::ValidationError(msg) => ApiError::BadRequest(msg),
            TransactionError::NetworkConfiguration(msg) => ApiError::InternalError(msg),
        }
    }
}

impl From<RepositoryError> for TransactionError {
    fn from(error: RepositoryError) -> Self {
        TransactionError::ValidationError(error.to_string())
    }
}
