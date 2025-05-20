//! # Solana Google Cloud KMS Signer Implementation
//!
//! This module provides a Solana signer implementation that uses the Google Cloud KMS API
//! for secure wallet management and cryptographic operations.
use std::str::FromStr;

use async_trait::async_trait;
use base64::Engine;
use log::{debug, info};
use solana_sdk::{
    instruction::Instruction,
    message::Message,
    pubkey::{self, Pubkey},
    signature::{Keypair, Signature},
    signer::{SeedDerivable, Signer as SolanaSigner},
    transaction::Transaction,
};

use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignDataResponseEvm, SignTransactionResponse,
        SignTypedDataRequest,
    },
    models::{
        Address, GoogleCloudKmsSignerConfig, NetworkTransactionData, SignerError, SignerRepoModel,
        TransactionRepoModel,
    },
    services::{GoogleCloudKmsService, GoogleCloudKmsServiceTrait, Signer},
    utils::{base64_decode, base64_encode},
};

use super::SolanaSignTrait;

pub type DefaultGoogleCloudKmsService = GoogleCloudKmsService;
pub struct GoogleCloudKmsSigner<T = DefaultGoogleCloudKmsService>
where
    T: GoogleCloudKmsServiceTrait,
{
    google_cloud_kms_service: T,
}

impl GoogleCloudKmsSigner<DefaultGoogleCloudKmsService> {
    /// Creates a new GoogleCloudKmsSigner with the default GoogleCloudKmsService service
    pub fn new(google_cloud_kms_service: DefaultGoogleCloudKmsService) -> Self {
        Self {
            google_cloud_kms_service,
        }
    }
}

#[cfg(test)]
impl<T: GoogleCloudKmsServiceTrait> GoogleCloudKmsSigner<T> {
    /// Creates a new GoogleCloudKmsSigner from a signer model and custom service implementation
    pub fn new_with_service(google_cloud_kms_service: T) -> Self {
        Self {
            google_cloud_kms_service,
        }
    }

    /// Creates a new GoogleCloudKmsSigner with provided config and service for testing
    pub fn new_for_testing(google_cloud_kms_service: T) -> Self {
        Self {
            google_cloud_kms_service,
        }
    }
}

#[async_trait]
impl<T: GoogleCloudKmsServiceTrait> SolanaSignTrait for GoogleCloudKmsSigner<T> {
    async fn pubkey(&self) -> Result<Address, SignerError> {
        let pubkey = self
            .google_cloud_kms_service
            .get_solana_address()
            .await
            .map_err(|e| SignerError::SigningError(e.to_string()));

        let address = pubkey.map(|pubkey| Address::Solana(pubkey.to_string()))?;

        Ok(address)
    }

    async fn sign(&self, message: &[u8]) -> Result<Signature, SignerError> {
        let sig_bytes = self
            .google_cloud_kms_service
            .sign_solana(message)
            .await
            .map_err(|e| SignerError::SigningError(e.to_string()))?;

        Ok(Signature::try_from(sig_bytes.as_slice()).map_err(|e| {
            SignerError::SigningError(format!("Failed to create signature from bytes: {}", e))
        })?)
    }
}

#[async_trait]
impl<T: GoogleCloudKmsServiceTrait> Signer for GoogleCloudKmsSigner<T> {
    async fn address(&self) -> Result<Address, SignerError> {
        let pubkey = self
            .google_cloud_kms_service
            .get_solana_address()
            .await
            .map_err(|e| SignerError::SigningError(e.to_string()));

        let address = pubkey.map(|pubkey| Address::Solana(pubkey.to_string()))?;

        Ok(address)
    }

    async fn sign_transaction(
        &self,
        _transaction: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        Err(SignerError::NotImplemented(
            "sign_transaction is not implemented".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::{GoogleCloudKmsSignerConfig, SecretString, SignerConfig, SolanaTransactionData},
        services::{GoogleCloudKmsError, MockGoogleCloudKmsServiceTrait},
    };
    use mockall::predicate::*;

    #[tokio::test]
    async fn test_address() {
        let mut mock_service = MockGoogleCloudKmsServiceTrait::new();

        mock_service
            .expect_get_solana_address()
            .times(1)
            .returning(|| {
                Box::pin(async { Ok("6s7RsvzcdXFJi1tXeDoGfSKZFzN3juVt9fTar6WEhEm2".to_string()) })
            });

        let signer = GoogleCloudKmsSigner::new_for_testing(mock_service);
        let result = signer.address().await.unwrap();

        match result {
            Address::Solana(addr) => {
                assert_eq!(addr, "6s7RsvzcdXFJi1tXeDoGfSKZFzN3juVt9fTar6WEhEm2");
            }
            _ => panic!("Expected Solana address"),
        }
    }

    #[tokio::test]
    async fn test_pubkey() {
        let mut mock_service = MockGoogleCloudKmsServiceTrait::new();

        mock_service
            .expect_get_solana_address()
            .times(1)
            .returning(|| {
                Box::pin(async { Ok("6s7RsvzcdXFJi1tXeDoGfSKZFzN3juVt9fTar6WEhEm2".to_string()) })
            });

        let signer = GoogleCloudKmsSigner::new_for_testing(mock_service);
        let result = signer.pubkey().await.unwrap();

        match result {
            Address::Solana(addr) => {
                assert_eq!(addr, "6s7RsvzcdXFJi1tXeDoGfSKZFzN3juVt9fTar6WEhEm2");
            }
            _ => panic!("Expected Solana address"),
        }
    }

    #[tokio::test]
    async fn test_sign() {
        let mut mock_service = MockGoogleCloudKmsServiceTrait::new();
        let test_message = b"Test message";

        let mock_sig_bytes = vec![1u8; 64];

        mock_service
            .expect_sign_solana()
            .times(1)
            .returning(move |_| {
                let sig_clone = mock_sig_bytes.clone();
                Box::pin(async move { Ok(sig_clone) })
            });

        let signer = GoogleCloudKmsSigner::new_for_testing(mock_service);
        let result = signer.sign(test_message).await.unwrap();

        let expected_sig = Signature::from([1u8; 64]);
        assert_eq!(result, expected_sig);
    }

    #[tokio::test]
    async fn test_sign_error_handling() {
        let mut mock_service = MockGoogleCloudKmsServiceTrait::new();
        let test_message = b"Test message";

        mock_service
            .expect_sign_solana()
            .times(1)
            .returning(move |_| {
                Box::pin(async { Err(GoogleCloudKmsError::ApiError("Mock signing error".into())) })
            });

        let signer = GoogleCloudKmsSigner::new_for_testing(mock_service);

        let result = signer.sign(test_message).await;

        assert!(result.is_err());
        match result {
            Err(SignerError::SigningError(msg)) => {
                assert!(msg.contains("Mock signing error"));
            }
            _ => panic!("Expected SigningError error variant"),
        }
    }

    #[tokio::test]
    async fn test_sign_invalid_signature_length() {
        let mut mock_service = MockGoogleCloudKmsServiceTrait::new();
        let test_message = b"Test message";

        mock_service
            .expect_sign_solana()
            .times(1)
            .returning(move |_| {
                let invalid_sig = vec![1u8; 32];
                Box::pin(async move { Ok(invalid_sig) })
            });

        let signer = GoogleCloudKmsSigner::new_for_testing(mock_service);

        let result = signer.sign(test_message).await;
        assert!(result.is_err());
        match result {
            Err(SignerError::SigningError(msg)) => {
                assert!(msg.contains("Failed to create signature from bytes"));
            }
            _ => panic!("Expected SigningError error variant"),
        }
    }

    #[tokio::test]
    async fn test_address_error_handling() {
        let mut mock_service = MockGoogleCloudKmsServiceTrait::new();

        mock_service
            .expect_get_solana_address()
            .times(1)
            .returning(|| {
                Box::pin(async {
                    Err(GoogleCloudKmsError::ConfigError(
                        "Invalid public key".to_string(),
                    ))
                })
            });

        let signer = GoogleCloudKmsSigner::new_for_testing(mock_service);
        let result = signer.address().await;

        assert!(result.is_err());
        match result {
            Err(SignerError::SigningError(msg)) => {
                assert!(msg.contains("Invalid public key"));
            }
            _ => panic!("Expected SigningError error variant"),
        }
    }

    #[tokio::test]
    async fn test_pubkey_error_propagation() {
        let mut mock_service = MockGoogleCloudKmsServiceTrait::new();

        mock_service
            .expect_get_solana_address()
            .times(1)
            .returning(|| {
                Box::pin(async {
                    Err(GoogleCloudKmsError::ApiError("API call failed".to_string()))
                })
            });

        let signer = GoogleCloudKmsSigner::new_for_testing(mock_service);
        let result = signer.pubkey().await;

        assert!(result.is_err());
        match result {
            Err(SignerError::SigningError(msg)) => {
                assert!(msg.contains("API call failed"));
            }
            _ => panic!("Expected SigningError error variant"),
        }
    }
}
