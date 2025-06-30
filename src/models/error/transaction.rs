use crate::{
    jobs::JobProducerError,
    models::{SignerError, SignerFactoryError},
    services::{ProviderError, SolanaProviderError},
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
        TransactionError::ValidationError(format!("XDR error: {}", error))
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
}
