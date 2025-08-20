use serde::Serialize;
use thiserror::Error;

use crate::services::{AwsKmsError, GoogleCloudKmsError, TurnkeyError, VaultError};

use super::TransactionError;

#[derive(Error, Debug, Serialize)]
#[allow(clippy::enum_variant_names)]
pub enum SignerError {
    #[error("Failed to sign transaction: {0}")]
    SigningError(String),

    #[error("Invalid key format: {0}")]
    KeyError(String),

    #[error("Provider error: {0}")]
    ProviderError(String),

    #[error("Unsupported signer type: {0}")]
    UnsupportedTypeError(String),

    #[error("Invalid transaction: {0}")]
    InvalidTransaction(#[from] TransactionError),

    #[error("Vault error: {0}")]
    VaultError(#[from] VaultError),

    #[error("Turnkey error: {0}")]
    TurnkeyError(#[from] TurnkeyError),

    #[error("AWS KMS error: {0}")]
    AwsKmsError(#[from] AwsKmsError),

    #[error("Google Cloud KMS error: {0}")]
    GoogleCloudKmsError(#[from] GoogleCloudKmsError),

    #[error("Not implemented: {0}")]
    NotImplemented(String),

    #[error("Invalid configuration: {0}")]
    ConfigError(String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Authentication error: {0}")]
    AuthenticationError(String),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Invalid configuration: {0}")]
    Configuration(String),

    #[error("Transaction conversion error: {0}")]
    ConversionError(String),
}

#[derive(Error, Debug, Serialize)]
pub enum SignerFactoryError {
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("Signer creation failed: {0}")]
    CreationFailed(String),
    #[error("Unsupported signer type: {0}")]
    UnsupportedType(String),
    #[error("Signer error: {0}")]
    SignerError(#[from] SignerError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signer_error_display() {
        let test_cases = vec![
            (
                SignerError::SigningError("failed to sign".to_string()),
                "Failed to sign transaction: failed to sign",
            ),
            (
                SignerError::KeyError("invalid key".to_string()),
                "Invalid key format: invalid key",
            ),
            (
                SignerError::ProviderError("connection failed".to_string()),
                "Provider error: connection failed",
            ),
            (
                SignerError::UnsupportedTypeError("unknown type".to_string()),
                "Unsupported signer type: unknown type",
            ),
            (
                SignerError::NotImplemented("feature not ready".to_string()),
                "Not implemented: feature not ready",
            ),
            (
                SignerError::Configuration("missing parameter".to_string()),
                "Invalid configuration: missing parameter",
            ),
            (
                SignerError::ConversionError("conversion failed".to_string()),
                "Transaction conversion error: conversion failed",
            ),
        ];

        for (error, expected_message) in test_cases {
            assert_eq!(error.to_string(), expected_message);
        }
    }

    #[test]
    fn test_signer_error_from_transaction_error() {
        let tx_error = TransactionError::ValidationError("bad format".to_string());
        let signer_error = SignerError::from(tx_error);

        match signer_error {
            SignerError::InvalidTransaction(e) => {
                assert_eq!(e.to_string(), "Transaction validation error: bad format");
            }
            _ => panic!("Expected SignerError::InvalidTransaction"),
        }
    }

    #[test]
    fn test_signer_error_from_vault_error() {
        let vault_error = VaultError::AuthenticationFailed("no permission".to_string());
        let signer_error = SignerError::from(vault_error);

        match signer_error {
            SignerError::VaultError(e) => {
                assert_eq!(e.to_string(), "Authentication failed: no permission");
            }
            _ => panic!("Expected SignerError::VaultError"),
        }
    }

    #[test]
    fn test_signer_error_from_google_cloud_kms_error() {
        let gcp_error = GoogleCloudKmsError::ApiError("authentication failed".to_string());
        let signer_error = SignerError::from(gcp_error);

        match signer_error {
            SignerError::GoogleCloudKmsError(e) => {
                assert_eq!(e.to_string(), "KMS API error: authentication failed");
            }
            _ => panic!("Expected SignerError::GoogleCloudKmsError"),
        }
    }

    #[test]
    fn test_signer_error_from_turnkey_error() {
        let turnkey_error = TurnkeyError::AuthenticationFailed("api failure".to_string());
        let signer_error = SignerError::from(turnkey_error);

        match signer_error {
            SignerError::TurnkeyError(e) => {
                assert_eq!(e.to_string(), "Authentication failed: api failure");
            }
            _ => panic!("Expected SignerError::TurnkeyError"),
        }
    }

    #[test]
    fn test_signer_error_from_aws_kms_error() {
        let aws_error = AwsKmsError::ConfigError("invalid credentials".to_string());
        let signer_error = SignerError::from(aws_error);

        match signer_error {
            SignerError::AwsKmsError(e) => {
                assert_eq!(e.to_string(), "AWS KMS config error: invalid credentials");
            }
            _ => panic!("Expected SignerError::AwsKmsError"),
        }
    }

    #[test]
    fn test_signer_factory_error_display() {
        let test_cases = vec![
            (
                SignerFactoryError::InvalidConfig("missing key".to_string()),
                "Invalid configuration: missing key",
            ),
            (
                SignerFactoryError::CreationFailed("initialization error".to_string()),
                "Signer creation failed: initialization error",
            ),
            (
                SignerFactoryError::UnsupportedType("unknown signer".to_string()),
                "Unsupported signer type: unknown signer",
            ),
        ];

        for (error, expected_message) in test_cases {
            assert_eq!(error.to_string(), expected_message);
        }
    }

    #[test]
    fn test_signer_factory_error_from_signer_error() {
        let signer_error = SignerError::KeyError("invalid key format".to_string());
        let factory_error = SignerFactoryError::from(signer_error);

        match factory_error {
            SignerFactoryError::SignerError(e) => {
                assert_eq!(e.to_string(), "Invalid key format: invalid key format");
            }
            _ => panic!("Expected SignerFactoryError::SignerError"),
        }
    }
}
