//! # Solana Turnkey Signer Implementation
//!
//! This module provides a Solana signer implementation that uses the Turnkey API
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
        Address, NetworkTransactionData, SignerError, SignerRepoModel, TransactionRepoModel,
        TurnkeySignerConfig,
    },
    services::{Signer, TurnkeyService, TurnkeyServiceTrait},
    utils::{base64_decode, base64_encode},
};

use super::SolanaSignTrait;

pub type DefaultTurnkeyService = TurnkeyService;

pub struct TurnkeySigner<T = DefaultTurnkeyService>
where
    T: TurnkeyServiceTrait,
{
    turnkey_service: T,
    config: TurnkeySignerConfig,
}

impl TurnkeySigner<DefaultTurnkeyService> {
    /// Creates a new TurnkeySigner with the default Turnkey service
    pub fn new(config: &TurnkeySignerConfig, turnkey_service: DefaultTurnkeyService) -> Self {
        Self {
            turnkey_service,
            config: config.clone(),
        }
    }
}

#[cfg(test)]
impl<T: TurnkeyServiceTrait> TurnkeySigner<T> {
    /// Creates a new TurnkeySigner from a signer model and custom service implementation
    pub fn new_with_service(signer_model: &SignerRepoModel, turnkey_service: T) -> Self {
        let config = signer_model
            .config
            .get_turnkey()
            .expect("Failed to get Turnkey config");

        Self {
            turnkey_service,
            config: config.clone(),
        }
    }

    /// Creates a new TurnkeySigner with provided config and service for testing
    pub fn new_for_testing(config: TurnkeySignerConfig, turnkey_service: T) -> Self {
        Self {
            turnkey_service,
            config,
        }
    }
}

#[async_trait]
impl<T: TurnkeyServiceTrait> SolanaSignTrait for TurnkeySigner<T> {
    fn pubkey(&self) -> Result<Address, SignerError> {
        let pubkey = self.turnkey_service.address_solana()?;

        Ok(pubkey)
    }

    async fn sign(&self, message: &[u8]) -> Result<Signature, SignerError> {
        let sig_bytes = self.turnkey_service.sign_solana(message).await?;

        Ok(Signature::try_from(sig_bytes.as_slice()).map_err(|e| {
            SignerError::SigningError(format!("Failed to create signature from bytes: {}", e))
        })?)
    }
}

#[async_trait]
impl<T: TurnkeyServiceTrait> Signer for TurnkeySigner<T> {
    async fn address(&self) -> Result<Address, SignerError> {
        let address = self.turnkey_service.address_solana()?;

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
        models::{SecretString, SignerConfig, SolanaTransactionData, TurnkeySignerConfig},
        services::{MockTurnkeyServiceTrait, TurnkeyError},
    };
    use mockall::predicate::*;

    fn create_test_config() -> TurnkeySignerConfig {
        TurnkeySignerConfig {
            api_public_key: "test-api-public-key".to_string(),
            api_private_key: SecretString::new("test-api-private-key"),
            organization_id: "test-org-id".to_string(),
            private_key_id: "test-private-key-id".to_string(),
            public_key: "5720be8aa9d2bb4be8e91f31d2c44c8629e42da16981c2cebabd55cafa0b76bd"
                .to_string(),
        }
    }

    #[test]
    fn test_new_with_service() {
        let mock_service = MockTurnkeyServiceTrait::new();
        let config = create_test_config();

        let signer = TurnkeySigner::new_for_testing(config.clone(), mock_service);

        assert_eq!(signer.config.api_public_key, "test-api-public-key");
        assert_eq!(signer.config.organization_id, "test-org-id");
        assert_eq!(
            signer.config.public_key,
            "5720be8aa9d2bb4be8e91f31d2c44c8629e42da16981c2cebabd55cafa0b76bd"
        );
    }

    #[tokio::test]
    async fn test_address() {
        let mut mock_service = MockTurnkeyServiceTrait::new();
        let config = create_test_config();

        mock_service.expect_address_solana().times(1).returning(|| {
            Ok(Address::Solana(
                "6s7RsvzcdXFJi1tXeDoGfSKZFzN3juVt9fTar6WEhEm2".to_string(),
            ))
        });

        let signer = TurnkeySigner::new_for_testing(config, mock_service);
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
        let mut mock_service = MockTurnkeyServiceTrait::new();

        mock_service.expect_address_solana().times(1).returning(|| {
            Ok(Address::Solana(
                "6s7RsvzcdXFJi1tXeDoGfSKZFzN3juVt9fTar6WEhEm2".to_string(),
            ))
        });

        let signer = TurnkeySigner::new_for_testing(create_test_config(), mock_service);
        let result = signer.pubkey().unwrap();

        match result {
            Address::Solana(addr) => {
                assert_eq!(addr, "6s7RsvzcdXFJi1tXeDoGfSKZFzN3juVt9fTar6WEhEm2");
            }
            _ => panic!("Expected Solana address"),
        }
    }

    #[tokio::test]
    async fn test_sign() {
        let mut mock_service = MockTurnkeyServiceTrait::new();
        let test_message = b"Test message";

        // Create a valid mock signature (must be exactly 64 bytes for Solana)
        let mock_sig_bytes = vec![1u8; 64];

        mock_service
            .expect_sign_solana()
            .times(1)
            .returning(move |message| {
                assert_eq!(message, test_message);
                let sig_clone = mock_sig_bytes.clone();
                Box::pin(async { Ok(sig_clone) })
            });

        let signer = TurnkeySigner::new_for_testing(create_test_config(), mock_service);
        let result = signer.sign(test_message).await.unwrap();

        let expected_sig = Signature::try_from([1u8; 64]).unwrap();
        assert_eq!(result, expected_sig);
    }

    #[tokio::test]
    async fn test_sign_error_handling() {
        let mut mock_service = MockTurnkeyServiceTrait::new();
        let test_message = b"Test message";

        mock_service
            .expect_sign_solana()
            .times(1)
            .returning(move |_| {
                Box::pin(async { Err(TurnkeyError::SigningError("Mock signing error".into())) })
            });

        let signer = TurnkeySigner::new_for_testing(create_test_config(), mock_service);

        let result = signer.sign(test_message).await;

        assert!(result.is_err());
        match result {
            Err(SignerError::TurnkeyError(err)) => {
                assert_eq!(err.to_string(), "Signing error: Mock signing error");
            }
            _ => panic!("Expected SigningError error variant"),
        }
    }

    #[tokio::test]
    async fn test_sign_invalid_signature_length() {
        let mut mock_service = MockTurnkeyServiceTrait::new();
        let test_message = b"Test message";

        // Return invalid signature length (not 64 bytes)
        mock_service
            .expect_sign_solana()
            .times(1)
            .returning(move |_| {
                let invalid_sig = vec![1u8; 32]; // Only 32 bytes instead of 64
                Box::pin(async { Ok(invalid_sig) })
            });

        let signer = TurnkeySigner::new_for_testing(create_test_config(), mock_service);

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
    async fn test_sign_transaction_not_implemented() {
        let mock_service = MockTurnkeyServiceTrait::new();
        let signer = TurnkeySigner::new_for_testing(create_test_config(), mock_service);

        let tx_data = SolanaTransactionData {
            recent_blockhash: Some("hash".to_string()),
            fee_payer: "payer".to_string(),
            instructions: vec![],
            hash: None,
        };

        let result = signer
            .sign_transaction(NetworkTransactionData::Solana(tx_data))
            .await;
        assert!(result.is_err());
        match result {
            Err(SignerError::NotImplemented(_)) => {}
            _ => panic!("Expected NotImplemented error variant"),
        }
    }

    #[tokio::test]
    async fn test_address_error_handling() {
        let mut mock_service = MockTurnkeyServiceTrait::new();
        let config = create_test_config();

        mock_service
            .expect_address_solana()
            .times(1)
            .returning(|| Err(TurnkeyError::ConfigError("Invalid public key".to_string())));

        let signer = TurnkeySigner::new_for_testing(config, mock_service);
        let result = signer.address().await;

        assert!(result.is_err());
    }
}
