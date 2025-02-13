use thiserror::Error;

use crate::models::ApiError;

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("Entity not found: {0}")]
    NotFound(String),

    #[error("Entity already exists: {0}")]
    LockError(String),

    #[error("Failed to connect to the database: {0}")]
    ConnectionError(String),

    #[error("Constraint violated: {0}")]
    ConstraintViolation(String),

    #[error("Invalid data: {0}")]
    InvalidData(String),

    #[error("Transaction failure: {0}")]
    TransactionFailure(String),

    #[error("Transaction validation failed: {0}")]
    TransactionValidationFailed(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("An unknown error occurred: {0}")]
    Unknown(String),

    #[error("Not supported: {0}")]
    NotSupported(String),
}

impl From<RepositoryError> for ApiError {
    fn from(error: RepositoryError) -> Self {
        match error {
            RepositoryError::NotFound(msg) => ApiError::NotFound(msg),
            RepositoryError::Unknown(msg) => ApiError::InternalError(msg),
            _ => ApiError::InternalError("An unknown error occurred".to_string()),
        }
    }
}
