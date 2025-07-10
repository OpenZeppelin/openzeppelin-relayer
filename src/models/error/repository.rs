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

    #[error("Other error: {0}")]
    Other(String),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repository_error_to_api_error_not_found() {
        let repo_error = RepositoryError::NotFound("User not found".to_string());
        let api_error = ApiError::from(repo_error);

        match api_error {
            ApiError::NotFound(msg) => assert_eq!(msg, "User not found"),
            _ => panic!("Expected ApiError::NotFound, got something else"),
        }
    }

    #[test]
    fn test_repository_error_to_api_error_unknown() {
        let repo_error = RepositoryError::Unknown("Database error".to_string());
        let api_error = ApiError::from(repo_error);

        match api_error {
            ApiError::InternalError(msg) => assert_eq!(msg, "Database error"),
            _ => panic!("Expected ApiError::InternalError, got something else"),
        }
    }

    #[test]
    fn test_repository_error_to_api_error_other_errors() {
        let test_cases = vec![
            RepositoryError::LockError("Lock error".to_string()),
            RepositoryError::ConnectionError("Connection error".to_string()),
            RepositoryError::ConstraintViolation("Constraint error".to_string()),
            RepositoryError::InvalidData("Invalid data".to_string()),
            RepositoryError::TransactionFailure("Transaction failed".to_string()),
            RepositoryError::TransactionValidationFailed("Validation failed".to_string()),
            RepositoryError::PermissionDenied("Permission denied".to_string()),
            RepositoryError::NotSupported("Not supported".to_string()),
        ];

        for repo_error in test_cases {
            let api_error = ApiError::from(repo_error);

            match api_error {
                ApiError::InternalError(msg) => assert_eq!(msg, "An unknown error occurred"),
                _ => panic!("Expected ApiError::InternalError, got something else"),
            }
        }
    }
}
