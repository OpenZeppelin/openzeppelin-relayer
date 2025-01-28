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
    #[error("Relayer is disabled")]
    RelayerDisabled,
    #[error("Relayer is paused")]
    RelayerPaused,
}

impl From<RelayerError> for ApiError {
    fn from(error: RelayerError) -> Self {
        match error {
            RelayerError::NetworkConfiguration(msg) => ApiError::InternalError(msg),
            RelayerError::ProviderError(msg) => ApiError::InternalError(msg),
            RelayerError::QueueError(msg) => ApiError::InternalError(msg),
            RelayerError::RelayerDisabled => {
                ApiError::ForbiddenError("Relayer disabled".to_string())
            }
            RelayerError::RelayerPaused => ApiError::ForbiddenError("Relayer paused".to_string()),
        }
    }
}

impl From<RepositoryError> for RelayerError {
    fn from(error: RepositoryError) -> Self {
        RelayerError::NetworkConfiguration(error.to_string())
    }
}
