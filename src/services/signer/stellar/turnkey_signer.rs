//! # Stellar Turnkey Signer Implementation
//!
//! This module provides a Stellar signer implementation that uses the Turnkey API
//! for secure wallet management and cryptographic operations.

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use soroban_rs::xdr::{
    self, DecoratedSignature, Hash, Limits, ReadXdr, Transaction, TransactionEnvelope, WriteXdr,
};
use stellar_strkey;
use tracing::{debug, info};

use crate::{
    domain::{
        attach_signatures_to_envelope,
        stellar::{create_signature_payload, create_transaction_signature_payload},
        SignTransactionResponse,
    },
    models::{
        Address, NetworkTransactionData, SignerError, StellarTransactionData, TransactionInput,
    },
    services::{
        signer::{SignXdrTransactionResponseStellar, Signer},
        TurnkeyService, TurnkeyServiceTrait,
    },
};

use super::StellarSignTrait;

pub type DefaultTurnkeyService = TurnkeyService;

pub struct TurnkeySigner<T = DefaultTurnkeyService>
where
    T: TurnkeyServiceTrait,
{
    turnkey_service: T,
}

impl TurnkeySigner<DefaultTurnkeyService> {
    /// Creates a new TurnkeySigner with the default Turnkey service
    pub fn new(turnkey_service: DefaultTurnkeyService) -> Self {
        Self { turnkey_service }
    }
}

#[cfg(test)]
impl<T: TurnkeyServiceTrait> TurnkeySigner<T> {
    /// Creates a new TurnkeySigner with provided service for testing
    pub fn new_for_testing(turnkey_service: T) -> Self {
        Self { turnkey_service }
    }
}

impl<T: TurnkeyServiceTrait> TurnkeySigner<T> {
    /// Sign a transaction directly (for operations-based input)
    async fn sign_transaction_directly(
        &self,
        transaction: &xdr::Transaction,
        network_id: &Hash,
    ) -> Result<Vec<u8>, SignerError> {
        let signature_payload = create_transaction_signature_payload(transaction, network_id);

        let payload_bytes = signature_payload.to_xdr(Limits::none()).map_err(|e| {
            SignerError::SigningError(format!("Failed to serialize signature payload: {}", e))
        })?;

        debug!(
            "Signing transaction payload with {} bytes",
            payload_bytes.len()
        );

        self.turnkey_service
            .sign_stellar(&payload_bytes)
            .await
            .map_err(SignerError::TurnkeyError)
    }

    /// Sign an envelope (for XDR-based input)
    async fn sign_envelope(
        &self,
        envelope: &TransactionEnvelope,
        network_id: &Hash,
    ) -> Result<Vec<u8>, SignerError> {
        let signature_payload = create_signature_payload(envelope, network_id).map_err(|e| {
            SignerError::SigningError(format!("Failed to create signature payload: {}", e))
        })?;

        let payload_bytes = signature_payload.to_xdr(Limits::none()).map_err(|e| {
            SignerError::SigningError(format!("Failed to serialize signature payload: {}", e))
        })?;

        debug!(
            "Signing envelope payload with {} bytes",
            payload_bytes.len()
        );

        self.turnkey_service
            .sign_stellar(&payload_bytes)
            .await
            .map_err(SignerError::TurnkeyError)
    }

    /// Get the public key from the Turnkey service and parse it
    fn get_public_key_bytes(&self) -> Result<[u8; 32], SignerError> {
        let stellar_address = self
            .turnkey_service
            .address_stellar()
            .map_err(SignerError::TurnkeyError)?;

        match stellar_address {
            Address::Stellar(addr) => {
                let public_key =
                    stellar_strkey::ed25519::PublicKey::from_string(&addr).map_err(|e| {
                        SignerError::KeyError(format!("Invalid Stellar address: {}", e))
                    })?;

                let payload = public_key.0;
                let mut key_bytes = [0u8; 32];
                key_bytes.copy_from_slice(&payload);
                Ok(key_bytes)
            }
            _ => Err(SignerError::KeyError(
                "Expected Stellar address".to_string(),
            )),
        }
    }

    /// Create a decorated signature from raw signature bytes
    fn create_decorated_signature(
        &self,
        signature_bytes: Vec<u8>,
    ) -> Result<DecoratedSignature, SignerError> {
        if signature_bytes.len() != 64 {
            return Err(SignerError::SigningError(format!(
                "Invalid signature length: expected 64 bytes, got {}",
                signature_bytes.len()
            )));
        }

        let public_key_bytes = self.get_public_key_bytes()?;
        let signature_hint = xdr::SignatureHint(
            public_key_bytes[public_key_bytes.len() - 4..]
                .try_into()
                .unwrap(),
        );

        let signature = xdr::Signature(signature_bytes.try_into().map_err(|_| {
            SignerError::SigningError("Failed to convert signature bytes to array".to_string())
        })?);

        Ok(DecoratedSignature {
            hint: signature_hint,
            signature,
        })
    }
}

#[async_trait]
impl<T: TurnkeyServiceTrait> StellarSignTrait for TurnkeySigner<T> {
    async fn sign_xdr_transaction(
        &self,
        unsigned_xdr: &str,
        network_passphrase: &str,
    ) -> Result<SignXdrTransactionResponseStellar, SignerError> {
        info!(
            "Turnkey signing XDR transaction with network passphrase: {}",
            network_passphrase
        );

        let envelope =
            TransactionEnvelope::from_xdr_base64(unsigned_xdr, Limits::none()).map_err(|e| {
                SignerError::SigningError(format!(
                    "Failed to parse Stellar transaction XDR '{}...': {}",
                    &unsigned_xdr[..std::cmp::min(50, unsigned_xdr.len())],
                    e
                ))
            })?;

        let hash_bytes: [u8; 32] = Sha256::digest(network_passphrase.as_bytes()).into();
        let network_id = Hash(hash_bytes);

        let signature_bytes = self.sign_envelope(&envelope, &network_id).await?;
        let decorated_signature = self.create_decorated_signature(signature_bytes)?;

        let mut envelope = envelope;
        attach_signatures_to_envelope(&mut envelope, vec![decorated_signature.clone()])
            .map_err(|e| SignerError::SigningError(format!("Failed to attach signature: {}", e)))?;

        let signed_xdr = envelope.to_xdr_base64(Limits::none()).map_err(|e| {
            SignerError::SigningError(format!("Failed to serialize signed transaction: {}", e))
        })?;

        Ok(SignXdrTransactionResponseStellar {
            signed_xdr,
            signature: decorated_signature,
        })
    }
}

#[async_trait]
impl<T: TurnkeyServiceTrait> Signer for TurnkeySigner<T> {
    async fn address(&self) -> Result<Address, SignerError> {
        self.turnkey_service
            .address_stellar()
            .map_err(SignerError::TurnkeyError)
    }

    async fn sign_transaction(
        &self,
        transaction: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        let stellar_data = transaction.get_stellar_transaction_data().map_err(|e| {
            SignerError::SigningError(format!("Failed to get Stellar transaction data: {}", e))
        })?;

        let network_passphrase = &stellar_data.network_passphrase;
        let hash_bytes: [u8; 32] = Sha256::digest(network_passphrase.as_bytes()).into();
        let network_id = Hash(hash_bytes);

        info!(
            "Turnkey signing transaction with network passphrase: {}",
            network_passphrase
        );

        // Sign based on transaction input type
        let signature = match &stellar_data.transaction_input {
            TransactionInput::Operations(_) => {
                let transaction = Transaction::try_from(stellar_data).map_err(|e| {
                    SignerError::SigningError(format!(
                        "Failed to build Stellar transaction from operations: {}",
                        e
                    ))
                })?;
                self.sign_transaction_directly(&transaction, &network_id)
                    .await?
            }
            TransactionInput::UnsignedXdr(xdr) | TransactionInput::SignedXdr { xdr, .. } => {
                let envelope =
                    TransactionEnvelope::from_xdr_base64(xdr, Limits::none()).map_err(|e| {
                        SignerError::SigningError(format!(
                            "Failed to parse Stellar transaction XDR '{}...': {}",
                            &xdr[..std::cmp::min(50, xdr.len())],
                            e
                        ))
                    })?;
                self.sign_envelope(&envelope, &network_id).await?
            }
        };

        let decorated_signature = self.create_decorated_signature(signature)?;

        Ok(SignTransactionResponse::Stellar(
            crate::domain::SignTransactionResponseStellar {
                signature: decorated_signature,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::{AssetSpec, OperationSpec},
        services::{MockTurnkeyServiceTrait, TurnkeyError},
    };
    use mockall::predicate::*;
    use soroban_rs::xdr::{SequenceNumber, Uint256};

    fn create_test_address() -> String {
        // Generate a valid Stellar address using stellar_strkey
        let test_key_bytes = [1u8; 32];
        stellar_strkey::ed25519::PublicKey::from_payload(&test_key_bytes)
            .unwrap()
            .to_string()
    }

    fn create_test_operations() -> Vec<OperationSpec> {
        vec![OperationSpec::Payment {
            destination: create_test_address(),
            amount: 1000,
            asset: AssetSpec::Native,
        }]
    }

    fn create_test_stellar_data_with_operations() -> crate::models::StellarTransactionData {
        crate::models::StellarTransactionData {
            source_account: create_test_address(),
            fee: Some(100),
            sequence_number: Some(123),
            memo: None,
            valid_until: None,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            signatures: vec![],
            hash: Some("test_hash".to_string()),
            simulation_transaction_data: None,
            transaction_input: TransactionInput::Operations(create_test_operations()),
            signed_envelope_xdr: None,
        }
    }

    fn create_test_stellar_data_with_xdr() -> crate::models::StellarTransactionData {
        // Create a minimal valid XDR transaction
        let tx = xdr::Transaction {
            source_account: xdr::MuxedAccount::Ed25519(Uint256([1u8; 32])),
            fee: 100,
            seq_num: SequenceNumber(123),
            cond: xdr::Preconditions::None,
            memo: xdr::Memo::None,
            operations: vec![].try_into().unwrap(),
            ext: xdr::TransactionExt::V0,
        };

        let envelope = xdr::TransactionEnvelope::Tx(xdr::TransactionV1Envelope {
            tx,
            signatures: vec![].try_into().unwrap(),
        });

        let xdr_string = envelope.to_xdr_base64(Limits::none()).unwrap();

        crate::models::StellarTransactionData {
            source_account: create_test_address(),
            fee: Some(100),
            sequence_number: Some(123),
            memo: None,
            valid_until: None,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            signatures: vec![],
            hash: Some("test_hash".to_string()),
            simulation_transaction_data: None,
            transaction_input: TransactionInput::UnsignedXdr(xdr_string),
            signed_envelope_xdr: None,
        }
    }

    #[tokio::test]
    async fn test_address() {
        let mut mock_service = MockTurnkeyServiceTrait::new();
        let test_address = create_test_address();

        mock_service
            .expect_address_stellar()
            .times(1)
            .returning(move || Ok(Address::Stellar(test_address.clone())));

        let signer = TurnkeySigner::new_for_testing(mock_service);
        let result = signer.address().await.unwrap();

        match result {
            Address::Stellar(addr) => {
                assert!(addr.starts_with('G'));
                assert_eq!(addr.len(), 56); // Standard Stellar address length
            }
            _ => panic!("Expected Stellar address"),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_with_operations() {
        let mut mock_service = MockTurnkeyServiceTrait::new();
        let test_address = create_test_address();

        // Mock signature (64 bytes for Ed25519)
        let mock_signature = vec![1u8; 64];

        mock_service
            .expect_address_stellar()
            .times(1)
            .returning(move || Ok(Address::Stellar(test_address.clone())));

        mock_service
            .expect_sign_stellar()
            .times(1)
            .returning(move |_| {
                let sig_clone = mock_signature.clone();
                Box::pin(async move { Ok(sig_clone) })
            });

        let signer = TurnkeySigner::new_for_testing(mock_service);
        let tx_data = create_test_stellar_data_with_operations();

        let result = signer
            .sign_transaction(NetworkTransactionData::Stellar(tx_data))
            .await;

        assert!(result.is_ok());
        if let Ok(SignTransactionResponse::Stellar(signed_data)) = result {
            // Check that the signature was created
            assert_eq!(signed_data.signature.signature.0.len(), 64);
        } else {
            panic!("Expected Stellar transaction response");
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_with_xdr() {
        let mut mock_service = MockTurnkeyServiceTrait::new();
        let test_address = create_test_address();

        // Mock signature (64 bytes for Ed25519)
        let mock_signature = vec![2u8; 64];

        mock_service
            .expect_address_stellar()
            .times(1)
            .returning(move || Ok(Address::Stellar(test_address.clone())));

        mock_service
            .expect_sign_stellar()
            .times(1)
            .returning(move |_| {
                let sig_clone = mock_signature.clone();
                Box::pin(async move { Ok(sig_clone) })
            });

        let signer = TurnkeySigner::new_for_testing(mock_service);
        let tx_data = create_test_stellar_data_with_xdr();

        let result = signer
            .sign_transaction(NetworkTransactionData::Stellar(tx_data))
            .await;

        assert!(result.is_ok());
        if let Ok(SignTransactionResponse::Stellar(signed_data)) = result {
            // Check that the signature was created
            assert_eq!(signed_data.signature.signature.0.len(), 64);
        } else {
            panic!("Expected Stellar transaction response");
        }
    }

    #[tokio::test]
    async fn test_sign_xdr_transaction() {
        let mut mock_service = MockTurnkeyServiceTrait::new();
        let test_address = create_test_address();

        // Mock signature (64 bytes for Ed25519)
        let mock_signature = vec![3u8; 64];

        mock_service
            .expect_address_stellar()
            .times(1)
            .returning(move || Ok(Address::Stellar(test_address.clone())));

        mock_service
            .expect_sign_stellar()
            .times(1)
            .returning(move |_| {
                let sig_clone = mock_signature.clone();
                Box::pin(async move { Ok(sig_clone) })
            });

        let signer = TurnkeySigner::new_for_testing(mock_service);
        let test_data = create_test_stellar_data_with_xdr();

        let unsigned_xdr = match &test_data.transaction_input {
            TransactionInput::UnsignedXdr(xdr) => xdr,
            _ => panic!("Expected unsigned XDR"),
        };

        let result = signer
            .sign_xdr_transaction(unsigned_xdr, &test_data.network_passphrase)
            .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(!response.signed_xdr.is_empty());
    }

    #[tokio::test]
    async fn test_sign_error_handling() {
        let mut mock_service = MockTurnkeyServiceTrait::new();

        mock_service
            .expect_address_stellar()
            .times(1)
            .returning(|| Err(TurnkeyError::SigningError("Mock signing error".into())));

        let signer = TurnkeySigner::new_for_testing(mock_service);

        let result = signer.address().await;
        assert!(result.is_err());
        match result {
            Err(SignerError::TurnkeyError(err)) => {
                assert_eq!(err.to_string(), "Signing error: Mock signing error");
            }
            _ => panic!("Expected TurnkeyError error variant"),
        }
    }

    #[tokio::test]
    async fn test_invalid_signature_length() {
        let mut mock_service = MockTurnkeyServiceTrait::new();
        mock_service
            .expect_sign_stellar()
            .times(1)
            .returning(|_| Box::pin(async { Ok(vec![1u8; 32]) })); // Invalid length

        let signer = TurnkeySigner::new_for_testing(mock_service);
        let tx_data = create_test_stellar_data_with_operations();

        let result = signer
            .sign_transaction(NetworkTransactionData::Stellar(tx_data))
            .await;

        assert!(result.is_err());
        match result {
            Err(SignerError::SigningError(msg)) => {
                assert!(msg.contains("Invalid signature length"));
            }
            _ => panic!("Expected SigningError error variant"),
        }
    }
}
