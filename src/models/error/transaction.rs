use crate::{
    domain::solana::SolanaTransactionValidationError,
    jobs::JobProducerError,
    models::{SignerError, SignerFactoryError},
    services::provider::{ProviderError, SolanaProviderError},
};

use super::{ApiError, RepositoryError, StellarProviderError};
use eyre::Report;
use serde::Serialize;
use soroban_rs::xdr;
use thiserror::Error;

#[derive(Error, Debug, Serialize)]
pub enum TransactionError {
    #[error("Transaction validation error: {0}")]
    ValidationError(String),

    #[error("Solana transaction validation error: {0}")]
    SolanaValidation(#[from] SolanaTransactionValidationError),

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

    #[error("Stellar transaction simulation failed: {0}")]
    SimulationFailed(String),
}

impl TransactionError {
    /// Determines if this error is transient (can retry) or permanent (should fail).
    ///
    /// **Transient (can retry):**
    /// - `SolanaValidation`: Delegates to underlying error's is_transient()
    /// - `UnderlyingSolanaProvider`: Delegates to underlying error's is_transient()
    /// - `UnderlyingProvider`: Delegates to underlying error's is_transient()
    /// - `UnexpectedError`: Unexpected errors may resolve on retry
    /// - `JobProducerError`: Job queue issues are typically transient
    ///
    /// **Permanent (fail immediately):**
    /// - `ValidationError`: Malformed data, missing fields, invalid state transitions
    /// - `InsufficientBalance`: Balance issues won't resolve without funding
    /// - `NetworkConfiguration`: Configuration errors are permanent
    /// - `InvalidType`: Type mismatches are permanent
    /// - `NotSupported`: Unsupported operations won't change
    /// - `SignerError`: Signer issues are typically permanent
    /// - `SimulationFailed`: Transaction simulation failures are permanent
    pub fn is_transient(&self) -> bool {
        match self {
            // Delegate to underlying error's is_transient() method
            TransactionError::SolanaValidation(err) => err.is_transient(),
            TransactionError::UnderlyingSolanaProvider(err) => err.is_transient(),
            TransactionError::UnderlyingProvider(err) => err.is_transient(),

            // Transient errors - may resolve on retry
            TransactionError::UnexpectedError(_) => true,
            TransactionError::JobProducerError(_) => true,

            // Permanent errors - fail immediately
            TransactionError::ValidationError(_) => false,
            TransactionError::InsufficientBalance(_) => false,
            TransactionError::NetworkConfiguration(_) => false,
            TransactionError::InvalidType(_) => false,
            TransactionError::NotSupported(_) => false,
            TransactionError::SignerError(_) => false,
            TransactionError::SimulationFailed(_) => false,
        }
    }
}

impl From<TransactionError> for ApiError {
    fn from(error: TransactionError) -> Self {
        match error {
            TransactionError::ValidationError(msg) => ApiError::BadRequest(msg),
            TransactionError::SolanaValidation(err) => ApiError::BadRequest(err.to_string()),
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
            TransactionError::SimulationFailed(msg) => ApiError::BadRequest(msg),
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

impl From<StellarProviderError> for TransactionError {
    fn from(error: StellarProviderError) -> Self {
        match error {
            StellarProviderError::SimulationFailed(msg) => TransactionError::SimulationFailed(msg),
            StellarProviderError::InsufficientBalance(msg) => {
                TransactionError::InsufficientBalance(msg)
            }
            StellarProviderError::BadSeq(msg) => TransactionError::ValidationError(msg),
            StellarProviderError::RpcError(msg) | StellarProviderError::Unknown(msg) => {
                TransactionError::UnderlyingProvider(ProviderError::NetworkConfiguration(msg))
            }
        }
    }
}

impl From<xdr::Error> for TransactionError {
    fn from(error: xdr::Error) -> Self {
        TransactionError::ValidationError(format!("XDR error: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_error_display() {
        let test_cases = vec![
            (
                TransactionError::ValidationError("invalid input".to_string()),
                "Transaction validation error: invalid input",
            ),
            (
                TransactionError::NetworkConfiguration("wrong network".to_string()),
                "Network configuration error: wrong network",
            ),
            (
                TransactionError::InvalidType("unknown type".to_string()),
                "Invalid transaction type: unknown type",
            ),
            (
                TransactionError::UnexpectedError("something went wrong".to_string()),
                "Unexpected error: something went wrong",
            ),
            (
                TransactionError::NotSupported("feature unavailable".to_string()),
                "Not supported: feature unavailable",
            ),
            (
                TransactionError::SignerError("key error".to_string()),
                "Signer error: key error",
            ),
            (
                TransactionError::InsufficientBalance("not enough funds".to_string()),
                "Insufficient balance: not enough funds",
            ),
            (
                TransactionError::SimulationFailed("sim failed".to_string()),
                "Stellar transaction simulation failed: sim failed",
            ),
        ];

        for (error, expected_message) in test_cases {
            assert_eq!(error.to_string(), expected_message);
        }
    }

    #[test]
    fn test_transaction_error_to_api_error() {
        let test_cases = vec![
            (
                TransactionError::ValidationError("invalid input".to_string()),
                ApiError::BadRequest("invalid input".to_string()),
            ),
            (
                TransactionError::NetworkConfiguration("wrong network".to_string()),
                ApiError::InternalError("wrong network".to_string()),
            ),
            (
                TransactionError::InvalidType("unknown type".to_string()),
                ApiError::InternalError("unknown type".to_string()),
            ),
            (
                TransactionError::UnexpectedError("something went wrong".to_string()),
                ApiError::InternalError("something went wrong".to_string()),
            ),
            (
                TransactionError::NotSupported("feature unavailable".to_string()),
                ApiError::BadRequest("feature unavailable".to_string()),
            ),
            (
                TransactionError::SignerError("key error".to_string()),
                ApiError::InternalError("key error".to_string()),
            ),
            (
                TransactionError::InsufficientBalance("not enough funds".to_string()),
                ApiError::BadRequest("not enough funds".to_string()),
            ),
            (
                TransactionError::SimulationFailed("boom".to_string()),
                ApiError::BadRequest("boom".to_string()),
            ),
        ];

        for (tx_error, expected_api_error) in test_cases {
            let api_error = ApiError::from(tx_error);

            match (&api_error, &expected_api_error) {
                (ApiError::BadRequest(actual), ApiError::BadRequest(expected)) => {
                    assert_eq!(actual, expected);
                }
                (ApiError::InternalError(actual), ApiError::InternalError(expected)) => {
                    assert_eq!(actual, expected);
                }
                _ => panic!(
                    "Error types don't match: {:?} vs {:?}",
                    api_error, expected_api_error
                ),
            }
        }
    }

    #[test]
    fn test_repository_error_to_transaction_error() {
        let repo_error = RepositoryError::NotFound("record not found".to_string());
        let tx_error = TransactionError::from(repo_error);

        match tx_error {
            TransactionError::ValidationError(msg) => {
                assert_eq!(msg, "Entity not found: record not found");
            }
            _ => panic!("Expected TransactionError::ValidationError"),
        }
    }

    #[test]
    fn test_report_to_transaction_error() {
        let report = Report::msg("An unexpected error occurred");
        let tx_error = TransactionError::from(report);

        match tx_error {
            TransactionError::UnexpectedError(msg) => {
                assert!(msg.contains("An unexpected error occurred"));
            }
            _ => panic!("Expected TransactionError::UnexpectedError"),
        }
    }

    #[test]
    fn test_signer_factory_error_to_transaction_error() {
        let factory_error = SignerFactoryError::InvalidConfig("missing key".to_string());
        let tx_error = TransactionError::from(factory_error);

        match tx_error {
            TransactionError::SignerError(msg) => {
                assert!(msg.contains("missing key"));
            }
            _ => panic!("Expected TransactionError::SignerError"),
        }
    }

    #[test]
    fn test_signer_error_to_transaction_error() {
        let signer_error = SignerError::KeyError("invalid key format".to_string());
        let tx_error = TransactionError::from(signer_error);

        match tx_error {
            TransactionError::SignerError(msg) => {
                assert!(msg.contains("invalid key format"));
            }
            _ => panic!("Expected TransactionError::SignerError"),
        }
    }

    #[test]
    fn test_provider_error_conversion() {
        let provider_error = ProviderError::NetworkConfiguration("timeout".to_string());
        let tx_error = TransactionError::from(provider_error);

        match tx_error {
            TransactionError::UnderlyingProvider(err) => {
                assert!(err.to_string().contains("timeout"));
            }
            _ => panic!("Expected TransactionError::UnderlyingProvider"),
        }
    }

    #[test]
    fn test_solana_provider_error_conversion() {
        let solana_error = SolanaProviderError::RpcError("invalid response".to_string());
        let tx_error = TransactionError::from(solana_error);

        match tx_error {
            TransactionError::UnderlyingSolanaProvider(err) => {
                assert!(err.to_string().contains("invalid response"));
            }
            _ => panic!("Expected TransactionError::UnderlyingSolanaProvider"),
        }
    }

    #[test]
    fn test_job_producer_error_conversion() {
        let job_error = JobProducerError::QueueError("queue full".to_string());
        let tx_error = TransactionError::from(job_error);

        match tx_error {
            TransactionError::JobProducerError(err) => {
                assert!(err.to_string().contains("queue full"));
            }
            _ => panic!("Expected TransactionError::JobProducerError"),
        }
    }

    #[test]
    fn test_xdr_error_conversion() {
        use soroban_rs::xdr::{Limits, ReadXdr, TransactionEnvelope};

        // Create an XDR error by trying to parse invalid base64
        let xdr_error =
            TransactionEnvelope::from_xdr_base64("invalid_base64", Limits::none()).unwrap_err();

        let tx_error = TransactionError::from(xdr_error);

        match tx_error {
            TransactionError::ValidationError(msg) => {
                assert!(msg.contains("XDR error:"));
            }
            _ => panic!("Expected TransactionError::ValidationError"),
        }
    }

    #[test]
    fn test_is_transient_permanent_errors() {
        // Test permanent errors that should return false
        let permanent_errors = vec![
            TransactionError::ValidationError("invalid input".to_string()),
            TransactionError::InsufficientBalance("not enough funds".to_string()),
            TransactionError::NetworkConfiguration("wrong network".to_string()),
            TransactionError::InvalidType("unknown type".to_string()),
            TransactionError::NotSupported("feature unavailable".to_string()),
            TransactionError::SignerError("key error".to_string()),
            TransactionError::SimulationFailed("sim failed".to_string()),
        ];

        for error in permanent_errors {
            assert!(
                !error.is_transient(),
                "Error {:?} should be permanent",
                error
            );
        }
    }

    #[test]
    fn test_is_transient_transient_errors() {
        // Test transient errors that should return true
        let transient_errors = vec![
            TransactionError::UnexpectedError("something went wrong".to_string()),
            TransactionError::JobProducerError(JobProducerError::QueueError(
                "queue full".to_string(),
            )),
        ];

        for error in transient_errors {
            assert!(
                error.is_transient(),
                "Error {:?} should be transient",
                error
            );
        }
    }

    #[test]
    fn test_stellar_provider_error_conversion() {
        // Test SimulationFailed
        let sim_error = StellarProviderError::SimulationFailed("sim failed".to_string());
        let tx_error = TransactionError::from(sim_error);
        match tx_error {
            TransactionError::SimulationFailed(msg) => {
                assert_eq!(msg, "sim failed");
            }
            _ => panic!("Expected TransactionError::SimulationFailed"),
        }

        // Test InsufficientBalance
        let balance_error =
            StellarProviderError::InsufficientBalance("not enough funds".to_string());
        let tx_error = TransactionError::from(balance_error);
        match tx_error {
            TransactionError::InsufficientBalance(msg) => {
                assert_eq!(msg, "not enough funds");
            }
            _ => panic!("Expected TransactionError::InsufficientBalance"),
        }

        // Test BadSeq
        let seq_error = StellarProviderError::BadSeq("bad sequence".to_string());
        let tx_error = TransactionError::from(seq_error);
        match tx_error {
            TransactionError::ValidationError(msg) => {
                assert_eq!(msg, "bad sequence");
            }
            _ => panic!("Expected TransactionError::ValidationError"),
        }

        // Test RpcError
        let rpc_error = StellarProviderError::RpcError("rpc failed".to_string());
        let tx_error = TransactionError::from(rpc_error);
        match tx_error {
            TransactionError::UnderlyingProvider(ProviderError::NetworkConfiguration(msg)) => {
                assert_eq!(msg, "rpc failed");
            }
            _ => panic!("Expected TransactionError::UnderlyingProvider"),
        }

        // Test Unknown
        let unknown_error = StellarProviderError::Unknown("unknown error".to_string());
        let tx_error = TransactionError::from(unknown_error);
        match tx_error {
            TransactionError::UnderlyingProvider(ProviderError::NetworkConfiguration(msg)) => {
                assert_eq!(msg, "unknown error");
            }
            _ => panic!("Expected TransactionError::UnderlyingProvider"),
        }
    }

    #[test]
    fn test_is_transient_delegated_errors() {
        // Test errors that delegate to underlying error's is_transient() method
        // We need to create mock errors that have is_transient() methods

        // For SolanaValidation - create a mock error
        use crate::domain::solana::SolanaTransactionValidationError;
        let solana_validation_error =
            SolanaTransactionValidationError::ValidationError("bad validation".to_string());
        let tx_error = TransactionError::SolanaValidation(solana_validation_error);
        // This will delegate to the underlying error's is_transient method
        // We can't easily test the delegation without mocking, so we'll just ensure it doesn't panic
        let _ = tx_error.is_transient();

        // For UnderlyingSolanaProvider
        let solana_provider_error = SolanaProviderError::RpcError("rpc failed".to_string());
        let tx_error = TransactionError::UnderlyingSolanaProvider(solana_provider_error);
        let _ = tx_error.is_transient();

        // For UnderlyingProvider
        let provider_error = ProviderError::NetworkConfiguration("network issue".to_string());
        let tx_error = TransactionError::UnderlyingProvider(provider_error);
        let _ = tx_error.is_transient();
    }
}
