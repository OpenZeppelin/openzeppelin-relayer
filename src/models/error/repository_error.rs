use thiserror::Error;

use crate::RelayerApiError;

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("Entity not found: {0}")]
    NotFound(String),

    #[error("Failed to connect to the database: {0}")]
    ConnectionError(String),

    #[error("Constraint violated: {0}")]
    ConstraintViolation(String),

    #[error("Invalid data: {0}")]
    InvalidData(String),

    #[error("Transaction failure: {0}")]
    TransactionFailure(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("An unknown error occurred: {0}")]
    Unknown(String),
}

impl From<RepositoryError> for RelayerApiError {
    fn from(error: RepositoryError) -> Self {
        match error {
            RepositoryError::NotFound(msg) => RelayerApiError::NotFound(msg),
            RepositoryError::ConnectionError(msg) => RelayerApiError::InternalError,
            RepositoryError::ConstraintViolation(msg) => RelayerApiError::InternalError,
            RepositoryError::InvalidData(msg) => RelayerApiError::BadRequest(msg),
            RepositoryError::TransactionFailure(msg) => RelayerApiError::InternalError,
            RepositoryError::PermissionDenied(msg) => RelayerApiError::Unauthorized(msg),
            RepositoryError::Unknown(msg) => RelayerApiError::InternalError,
            _ => RelayerApiError::InternalError,
        }
    }
}
