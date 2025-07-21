use crate::{
    models::{SignerError, SignerFactoryError},
    repositories::TransactionCounterError,
    services::{ProviderError, SolanaProviderError},
};

use super::{ApiError, RepositoryError};
use crate::models::NetworkError;
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
    #[error("Insufficient relayer balance: {0}")]
    InsufficientRelayerBalance(String),
    #[error("Relayer Policy configuration error: {0}")]
    PolicyConfigurationError(String),
    #[error("Invalid Dex name : {0}")]
    InvalidDexName(String),
    #[error("Dex error : {0}")]
    DexError(String),
    #[error("Transaction validation error: {0}")]
    ValidationError(String),
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
            RelayerError::InsufficientRelayerBalance(msg) => ApiError::BadRequest(msg),
            RelayerError::UnderlyingProvider(err) => ApiError::InternalError(err.to_string()),
            RelayerError::UnderlyingSolanaProvider(err) => ApiError::InternalError(err.to_string()),
            RelayerError::PolicyConfigurationError(msg) => ApiError::InternalError(msg),
            RelayerError::InvalidDexName(msg) => ApiError::InternalError(msg),
            RelayerError::DexError(msg) => ApiError::InternalError(msg),
            RelayerError::ValidationError(msg) => ApiError::BadRequest(msg),
        }
    }
}

impl From<RepositoryError> for RelayerError {
    fn from(error: RepositoryError) -> Self {
        RelayerError::NetworkConfiguration(error.to_string())
    }
}

impl From<NetworkError> for RelayerError {
    fn from(err: NetworkError) -> Self {
        RelayerError::NetworkConfiguration(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SignerError;
    use crate::repositories::TransactionCounterError;
    use crate::services::{ProviderError, SolanaProviderError};

    #[test]
    fn test_relayer_error_variants() {
        let network_error = RelayerError::NetworkConfiguration("Invalid network".to_string());
        assert_eq!(
            network_error.to_string(),
            "Network configuration error: Invalid network"
        );

        let provider_error = RelayerError::ProviderError("Connection failed".to_string());
        assert_eq!(
            provider_error.to_string(),
            "Provider error: Connection failed"
        );

        let queue_error = RelayerError::QueueError("Queue full".to_string());
        assert_eq!(queue_error.to_string(), "Queue error: Queue full");

        let not_supported = RelayerError::NotSupported("Feature unavailable".to_string());
        assert_eq!(
            not_supported.to_string(),
            "Not supported: Feature unavailable"
        );

        let disabled = RelayerError::RelayerDisabled;
        assert_eq!(disabled.to_string(), "Relayer is disabled");

        let paused = RelayerError::RelayerPaused;
        assert_eq!(paused.to_string(), "Relayer is paused");

        let insufficient_balance =
            RelayerError::InsufficientBalanceError("Not enough ETH".to_string());
        assert_eq!(
            insufficient_balance.to_string(),
            "Insufficient balance error: Not enough ETH"
        );

        let policy_error = RelayerError::PolicyConfigurationError("Invalid policy".to_string());
        assert_eq!(
            policy_error.to_string(),
            "Relayer Policy configuration error: Invalid policy"
        );
    }

    #[test]
    fn test_from_provider_error() {
        let provider_error = ProviderError::NetworkConfiguration("RPC timeout".to_string());
        let relayer_error: RelayerError = provider_error.into();

        assert!(matches!(relayer_error, RelayerError::UnderlyingProvider(_)));
        assert!(relayer_error.to_string().contains("RPC timeout"));
    }

    #[test]
    fn test_from_solana_provider_error() {
        let solana_error = SolanaProviderError::RpcError("Solana RPC down".to_string());
        let relayer_error: RelayerError = solana_error.into();

        assert!(matches!(
            relayer_error,
            RelayerError::UnderlyingSolanaProvider(_)
        ));
        assert!(relayer_error.to_string().contains("Solana RPC down"));
    }

    #[test]
    fn test_from_signer_factory_error() {
        let factory_error = SignerFactoryError::InvalidConfig("Unknown chain".to_string());
        let relayer_error: RelayerError = factory_error.into();

        assert!(matches!(relayer_error, RelayerError::SignerFactoryError(_)));
        assert!(relayer_error.to_string().contains("Unknown chain"));
    }

    #[test]
    fn test_from_signer_error() {
        let signer_error = SignerError::SigningError("Invalid key".to_string());
        let relayer_error: RelayerError = signer_error.into();

        assert!(matches!(relayer_error, RelayerError::SignerError(_)));
        assert!(relayer_error.to_string().contains("Invalid key"));
    }

    #[test]
    fn test_from_transaction_counter_error() {
        let counter_error = TransactionCounterError::NotFound("Nonce not found".to_string());
        let relayer_error: RelayerError = counter_error.into();

        assert!(matches!(
            relayer_error,
            RelayerError::TransactionSequenceError(_)
        ));
        assert!(relayer_error.to_string().contains("Nonce not found"));
    }

    #[test]
    fn test_conversion_to_api_error() {
        let network_error = RelayerError::NetworkConfiguration("Invalid network".to_string());
        let api_error: ApiError = network_error.into();
        assert!(matches!(api_error, ApiError::InternalError(_)));

        let not_supported = RelayerError::NotSupported("Feature unavailable".to_string());
        let api_error: ApiError = not_supported.into();
        assert!(matches!(api_error, ApiError::BadRequest(_)));

        let disabled = RelayerError::RelayerDisabled;
        let api_error: ApiError = disabled.into();
        assert!(matches!(api_error, ApiError::ForbiddenError(_)));
        assert_eq!(api_error.to_string(), "Forbidden: Relayer disabled");

        let insufficient = RelayerError::InsufficientBalanceError("Not enough funds".to_string());
        let api_error: ApiError = insufficient.into();
        assert!(matches!(api_error, ApiError::BadRequest(_)));
    }

    #[test]
    fn test_from_repository_error() {
        let repo_error = RepositoryError::ConnectionError("Connection failed".to_string());
        let relayer_error: RelayerError = repo_error.into();

        assert!(matches!(
            relayer_error,
            RelayerError::NetworkConfiguration(_)
        ));
        assert!(relayer_error.to_string().contains("Connection failed"));
    }
}
