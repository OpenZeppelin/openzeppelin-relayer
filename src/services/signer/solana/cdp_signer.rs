//! # Solana CDP Signer Implementation
//!
//! This module provides a Solana signer implementation that uses the CDP API
//! for secure wallet management and cryptographic operations.
//!
//! ## Features
//!
//! - Secure signing of Solana messages
//! - Remote key management through CDP's secure infrastructure
//! - Message signing with proper Solana signature format
//!
//! ## Security Notes
//!
//! Private keys never leave the CDP service, providing enhanced security
//! compared to local key storage solutions.
use crate::{
    domain::SignTransactionResponse,
    models::{Address, CdpSignerConfig, NetworkTransactionData, SignerError},
    services::{signer::Signer, CdpService, CdpServiceTrait},
};
use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use solana_sdk::signature::Signature;
use solana_sdk::{pubkey::Pubkey, transaction::Transaction};
use std::str::FromStr;

use super::SolanaSignTrait;

pub type DefaultCdpService = CdpService;

pub struct CdpSigner<T = DefaultCdpService>
where
    T: CdpServiceTrait,
{
    cdp_service: T,
}

impl CdpSigner<DefaultCdpService> {
    pub fn new(config: CdpSignerConfig) -> Result<Self, SignerError> {
        let cdp_service = DefaultCdpService::new(config).map_err(|e| {
            SignerError::Configuration(format!("Failed to create CDP service: {}", e))
        })?;

        Ok(Self { cdp_service })
    }
}

#[cfg(test)]
impl<T: CdpServiceTrait> CdpSigner<T> {
    pub fn new_with_service(cdp_service: T) -> Self {
        Self { cdp_service }
    }

    pub fn new_for_testing(cdp_service: T) -> Self {
        Self { cdp_service }
    }
}

#[async_trait]
impl<T: CdpServiceTrait> SolanaSignTrait for CdpSigner<T> {
    async fn pubkey(&self) -> Result<Address, SignerError> {
        let address = self
            .cdp_service
            .account_address()
            .await
            .map_err(SignerError::CdpError)?;

        Ok(address)
    }

    async fn sign(&self, message: &[u8]) -> Result<Signature, SignerError> {
        // The message bytes are bincode-serialized transaction message data
        // We need to reconstruct a full transaction for the CDP API

        // Deserialize the message from bincode
        let solana_message: solana_sdk::message::Message =
            bincode::deserialize(message).map_err(|e| {
                SignerError::SigningError(format!("Failed to deserialize message: {}", e))
            })?;

        // Create an unsigned transaction from the message
        let transaction = solana_sdk::transaction::Transaction::new_unsigned(solana_message);

        // Convert to EncodedSerializedTransaction (base64)
        let encoded_tx = crate::models::EncodedSerializedTransaction::try_from(&transaction)
            .map_err(|e| {
                SignerError::SigningError(format!("Failed to encode transaction: {}", e))
            })?;

        // Use the CDP transaction signing API instead of message signing
        let signed_tx_bytes = self
            .cdp_service
            .sign_solana_transaction(encoded_tx.into_inner())
            .await
            .map_err(SignerError::CdpError)?;

        // The CDP service returns raw serialized signed-transaction bytes.
        // Encode to base64 to reuse EncodedSerializedTransaction for parsing.
        let signed_tx_encoded = general_purpose::STANDARD.encode(signed_tx_bytes);

        let signed_tx_data = crate::models::EncodedSerializedTransaction::new(signed_tx_encoded);
        let signed_transaction: Transaction = signed_tx_data.try_into().map_err(|e| {
            SignerError::SigningError(format!("Failed to decode signed transaction: {}", e))
        })?;

        // Get the CDP signer's address to find the correct signature index
        let cdp_address = self
            .cdp_service
            .account_address()
            .await
            .map_err(SignerError::CdpError)?;

        let cdp_pubkey = match cdp_address {
            crate::models::Address::Solana(addr) => Pubkey::from_str(&addr)
                .map_err(|e| SignerError::SigningError(format!("Invalid CDP pubkey: {}", e)))?,
            _ => {
                return Err(SignerError::SigningError(
                    "CDP address is not a Solana address".to_string(),
                ))
            }
        };

        // Find the signature index for the CDP signer's pubkey
        let signer_index = signed_transaction
            .message
            .account_keys
            .iter()
            .position(|key| *key == cdp_pubkey)
            .ok_or_else(|| {
                SignerError::SigningError("CDP pubkey not found in transaction signers".to_string())
            })?;

        // Extract the signature at the correct index
        if signer_index >= signed_transaction.signatures.len() {
            return Err(SignerError::SigningError(
                "Signature index out of bounds".to_string(),
            ));
        }

        Ok(signed_transaction.signatures[signer_index])
    }
}

#[async_trait]
impl<T: CdpServiceTrait> Signer for CdpSigner<T> {
    async fn address(&self) -> Result<Address, SignerError> {
        let address = self
            .cdp_service
            .account_address()
            .await
            .map_err(SignerError::CdpError)?;

        Ok(address)
    }

    async fn sign_transaction(
        &self,
        transaction: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        let solana_data = transaction.get_solana_transaction_data()?;

        let signed_transaction = self
            .cdp_service
            .sign_solana_transaction(solana_data.transaction)
            .await
            .map_err(SignerError::CdpError)?;

        Ok(SignTransactionResponse::Solana(signed_transaction))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::{CdpSignerConfig, SecretString, SolanaTransactionData},
        services::{signer::Signer as RelayerSigner, CdpError, MockCdpServiceTrait},
    };
    use mockall::predicate::*;
    use solana_sdk::signer::Signer;

    #[tokio::test]
    async fn test_address() {
        let mut mock_service = MockCdpServiceTrait::new();

        mock_service
            .expect_account_address()
            .times(1)
            .returning(|| {
                Box::pin(async {
                    Ok(Address::Solana(
                        "6s7RsvzcdXFJi1tXeDoGfSKZFzN3juVt9fTar6WEhEm2".to_string(),
                    ))
                })
            });

        let signer = CdpSigner::new_for_testing(mock_service);
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
        let mut mock_service = MockCdpServiceTrait::new();

        mock_service
            .expect_account_address()
            .times(1)
            .returning(|| {
                Box::pin(async {
                    Ok(Address::Solana(
                        "6s7RsvzcdXFJi1tXeDoGfSKZFzN3juVt9fTar6WEhEm2".to_string(),
                    ))
                })
            });

        let signer = CdpSigner::new_for_testing(mock_service);
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
        let mut mock_service = MockCdpServiceTrait::new();

        // Create a proper Solana message and serialize it with bincode
        use solana_sdk::{
            hash::Hash,
            message::Message,
            pubkey::Pubkey,
            signature::{Keypair, Signer},
        };
        use solana_system_interface::instruction;

        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let instruction = instruction::transfer(&payer.pubkey(), &recipient, 1000);
        let message = Message::new(&[instruction], Some(&payer.pubkey()));
        let test_message = bincode::serialize(&message).unwrap();

        // Mock a signed transaction response (base64-encoded)
        let transaction = solana_sdk::transaction::Transaction::new_unsigned(message.clone());
        let mut signed_transaction = transaction;
        signed_transaction.signatures = vec![Signature::from([1u8; 64])]; // Mock signature
        let signed_tx_bytes = bincode::serialize(&signed_transaction).unwrap();

        mock_service.expect_account_address().returning(move || {
            let addr = payer.pubkey().to_string();
            Box::pin(async move { Ok(Address::Solana(addr)) })
        });

        mock_service
            .expect_sign_solana_transaction()
            .times(1)
            .returning(move |_| {
                let signed_bytes = signed_tx_bytes.clone();
                Box::pin(async { Ok(signed_bytes) })
            });

        let signer = CdpSigner::new_for_testing(mock_service);
        let result = signer.sign(&test_message).await.unwrap();

        let expected_sig = Signature::from([1u8; 64]);
        assert_eq!(result, expected_sig);
    }

    #[tokio::test]
    async fn test_sign_error_handling() {
        let mut mock_service = MockCdpServiceTrait::new();

        // Create a proper Solana message and serialize it with bincode
        use solana_sdk::{
            hash::Hash,
            message::Message,
            pubkey::Pubkey,
            signature::{Keypair, Signer},
        };
        use solana_system_interface::instruction;

        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let instruction = instruction::transfer(&payer.pubkey(), &recipient, 1000);
        let message = Message::new(&[instruction], Some(&payer.pubkey()));
        let test_message = bincode::serialize(&message).unwrap();

        mock_service
            .expect_sign_solana_transaction()
            .times(1)
            .returning(move |_| {
                Box::pin(async { Err(CdpError::SigningError("Mock signing error".into())) })
            });

        let signer = CdpSigner::new_for_testing(mock_service);

        let result = signer.sign(&test_message).await;

        assert!(result.is_err());
        match result {
            Err(SignerError::CdpError(err)) => {
                assert_eq!(err.to_string(), "Signing error: Mock signing error");
            }
            _ => panic!("Expected CdpError error variant"),
        }
    }

    #[tokio::test]
    async fn test_sign_invalid_transaction_data() {
        let mut mock_service = MockCdpServiceTrait::new();

        // Create a proper Solana message and serialize it with bincode
        use solana_sdk::{
            hash::Hash,
            message::Message,
            pubkey::Pubkey,
            signature::{Keypair, Signer},
        };
        use solana_system_interface::instruction;

        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let instruction = instruction::transfer(&payer.pubkey(), &recipient, 1000);
        let message = Message::new(&[instruction], Some(&payer.pubkey()));
        let test_message = bincode::serialize(&message).unwrap();

        // Return invalid transaction data (not a valid serialized transaction)
        mock_service
            .expect_sign_solana_transaction()
            .times(1)
            .returning(move |_| {
                let invalid_tx_data = vec![1u8; 32]; // Invalid transaction data
                Box::pin(async { Ok(invalid_tx_data) })
            });

        let signer = CdpSigner::new_for_testing(mock_service);

        let result = signer.sign(&test_message).await;
        assert!(result.is_err());
        match result {
            Err(SignerError::SigningError(msg)) => {
                assert!(msg.contains("Failed to decode signed transaction"));
            }
            _ => panic!("Expected SigningError error variant"),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_success() {
        let mut mock_service = MockCdpServiceTrait::new();

        let test_transaction = "transaction_123".to_string();
        let mock_signed_transaction = vec![1u8; 64]; // Mock signed transaction bytes

        mock_service
            .expect_sign_solana_transaction()
            .times(1)
            .with(eq(test_transaction.clone()))
            .returning(move |_| {
                let signed_tx = mock_signed_transaction.clone();
                Box::pin(async { Ok(signed_tx) })
            });

        let signer = CdpSigner::new_for_testing(mock_service);

        let tx_data = SolanaTransactionData {
            transaction: test_transaction,
            signature: None,
        };

        let result = signer
            .sign_transaction(NetworkTransactionData::Solana(tx_data))
            .await;

        assert!(result.is_ok());
        match result.unwrap() {
            SignTransactionResponse::Solana(signed_tx) => {
                assert_eq!(signed_tx, vec![1u8; 64]);
            }
            _ => panic!("Expected Solana SignTransactionResponse"),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_error() {
        let mut mock_service = MockCdpServiceTrait::new();

        let test_transaction = "transaction_123".to_string();

        mock_service
            .expect_sign_solana_transaction()
            .times(1)
            .with(eq(test_transaction.clone()))
            .returning(move |_| {
                Box::pin(async { Err(CdpError::SigningError("Mock signing error".into())) })
            });

        let signer = CdpSigner::new_for_testing(mock_service);

        let tx_data = SolanaTransactionData {
            transaction: test_transaction,
            signature: None,
        };

        let result = signer
            .sign_transaction(NetworkTransactionData::Solana(tx_data))
            .await;

        assert!(result.is_err());
        match result {
            Err(SignerError::CdpError(err)) => {
                assert_eq!(err.to_string(), "Signing error: Mock signing error");
            }
            _ => panic!("Expected CdpError error variant"),
        }
    }

    #[tokio::test]
    async fn test_address_error_handling() {
        let mut mock_service = MockCdpServiceTrait::new();

        mock_service
            .expect_account_address()
            .times(1)
            .returning(|| {
                Box::pin(async { Err(CdpError::ConfigError("Invalid public key".to_string())) })
            });

        let signer = CdpSigner::new_for_testing(mock_service);
        let result = signer.address().await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_sign_missing_cdp_pubkey() {
        let mut mock = MockCdpServiceTrait::new();

        // Build a tx whose required signer is NOT the CDP pubkey
        use solana_sdk::{message::Message, pubkey::Pubkey, signature::Keypair};
        use solana_system_interface::instruction;

        let payer = Keypair::new();
        let other = Pubkey::new_unique();
        let ix = instruction::transfer(&payer.pubkey(), &other, 1);
        let msg = Message::new(&[ix], Some(&payer.pubkey()));
        let msg_bytes = bincode::serialize(&msg).unwrap();

        // Return a signed tx with a signature but a different signer key ordering
        let mut tx = Transaction::new_unsigned(msg.clone());
        tx.signatures = vec![Signature::from([2u8; 64])];
        let tx_bytes = bincode::serialize(&tx).unwrap();

        let other_str = other.to_string();
        mock.expect_account_address().returning(move || {
            let other_clone = other_str.clone();
            Box::pin(async move { Ok(Address::Solana(other_clone)) })
        });
        mock.expect_sign_solana_transaction().returning(move |_| {
            let tx_bytes_clone = tx_bytes.clone();
            Box::pin(async move { Ok(tx_bytes_clone) })
        });

        let signer = CdpSigner::new_for_testing(mock);
        let res = signer.sign(&msg_bytes).await;
        assert!(matches!(res, Err(SignerError::SigningError(_))));
    }
}
