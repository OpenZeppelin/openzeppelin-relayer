//! # Solana AWS KMS Signer Implementation
//!
//! This module provides a Solana signer implementation that uses the AWS KMS API
//! for secure wallet management and cryptographic operations.
use async_trait::async_trait;
use solana_sdk::signature::Signature;

use crate::{
    models::{Address, SignerError},
    services::{AwsKmsService, AwsKmsSolanaService},
};

use super::SolanaSignTrait;

pub type DefaultAwsKmsService = AwsKmsService;

#[derive(Debug)]
pub struct AwsKmsSigner<T = DefaultAwsKmsService>
where
    T: AwsKmsSolanaService,
{
    aws_kms_service: T,
}

impl AwsKmsSigner<DefaultAwsKmsService> {
    /// Creates a new AwsKmsSigner with the default AwsKmsService
    pub fn new(aws_kms_service: DefaultAwsKmsService) -> Self {
        Self { aws_kms_service }
    }
}

#[cfg(test)]
impl<T: AwsKmsSolanaService> AwsKmsSigner<T> {
    /// Creates a new AwsKmsSigner with a custom service implementation for testing
    pub fn new_for_testing(aws_kms_service: T) -> Self {
        Self { aws_kms_service }
    }
}

#[async_trait]
impl<T: AwsKmsSolanaService> SolanaSignTrait for AwsKmsSigner<T> {
    async fn pubkey(&self) -> Result<Address, SignerError> {
        self.aws_kms_service
            .get_solana_address()
            .await
            .map_err(|e| SignerError::SigningError(e.to_string()))
    }

    async fn sign(&self, message: &[u8]) -> Result<Signature, SignerError> {
        let sig_bytes = self
            .aws_kms_service
            .sign_solana(message)
            .await
            .map_err(|e| SignerError::SigningError(e.to_string()))?;

        Signature::try_from(sig_bytes.as_slice()).map_err(|e| {
            SignerError::SigningError(format!("Failed to create signature from bytes: {e}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{AwsKmsError, MockAwsKmsSolanaService};
    use mockall::predicate::*;

    #[tokio::test]
    async fn test_pubkey() {
        let mut mock_service = MockAwsKmsSolanaService::new();

        mock_service
            .expect_get_solana_address()
            .times(1)
            .returning(|| {
                Box::pin(async {
                    Ok(Address::Solana(
                        "BavUBpkD77FABnevMkBVqV8BDHv7gX8sSoYYJY9WU9L5".to_string(),
                    ))
                })
            });

        let signer = AwsKmsSigner::new_for_testing(mock_service);
        let result = signer.pubkey().await.unwrap();

        match result {
            Address::Solana(addr) => {
                assert_eq!(addr, "BavUBpkD77FABnevMkBVqV8BDHv7gX8sSoYYJY9WU9L5");
            }
            _ => panic!("Expected Solana address"),
        }
    }

    #[tokio::test]
    async fn test_sign() {
        let mut mock_service = MockAwsKmsSolanaService::new();
        let test_message = b"Test message";

        let mock_sig_bytes = vec![1u8; 64];

        mock_service
            .expect_sign_solana()
            .times(1)
            .returning(move |_| {
                let sig_clone = mock_sig_bytes.clone();
                Box::pin(async move { Ok(sig_clone) })
            });

        let signer = AwsKmsSigner::new_for_testing(mock_service);
        let result = signer.sign(test_message).await.unwrap();

        let expected_sig = Signature::from([1u8; 64]);
        assert_eq!(result, expected_sig);
    }

    #[tokio::test]
    async fn test_sign_error_handling() {
        let mut mock_service = MockAwsKmsSolanaService::new();
        let test_message = b"Test message";

        mock_service.expect_sign_solana().times(1).returning(|_| {
            Box::pin(async { Err(AwsKmsError::SignError("Mock signing error".into())) })
        });

        let signer = AwsKmsSigner::new_for_testing(mock_service);

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
        let mut mock_service = MockAwsKmsSolanaService::new();
        let test_message = b"Test message";

        mock_service.expect_sign_solana().times(1).returning(|_| {
            let invalid_sig = vec![1u8; 32]; // Invalid: 32 bytes instead of 64
            Box::pin(async move { Ok(invalid_sig) })
        });

        let signer = AwsKmsSigner::new_for_testing(mock_service);

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
    async fn test_pubkey_error_propagation() {
        let mut mock_service = MockAwsKmsSolanaService::new();

        mock_service
            .expect_get_solana_address()
            .times(1)
            .returning(|| {
                Box::pin(async { Err(AwsKmsError::GetError("API call failed".to_string())) })
            });

        let signer = AwsKmsSigner::new_for_testing(mock_service);
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
