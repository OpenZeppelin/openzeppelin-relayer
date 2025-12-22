//! # Stellar AWS KMS Signer Implementation
//!
//! This module provides a Stellar signer implementation that uses the AWS KMS API
//! for secure key management and cryptographic operations.

use super::StellarSignTrait;
use crate::{
    domain::{
        attach_signatures_to_envelope, parse_transaction_xdr,
        stellar::{create_signature_payload, create_transaction_signature_payload},
        SignTransactionResponse, SignXdrTransactionResponseStellar,
    },
    models::{Address, NetworkTransactionData, SignerError},
    services::{signer::Signer, AwsKmsService, AwsKmsStellarService},
};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use soroban_rs::xdr::{
    DecoratedSignature, Hash, Limits, ReadXdr, Signature, SignatureHint, Transaction,
    TransactionEnvelope, WriteXdr,
};
use tracing::debug;

pub type DefaultAwsKmsService = AwsKmsService;

pub struct AwsKmsSigner<T = DefaultAwsKmsService>
where
    T: AwsKmsStellarService,
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
impl<T: AwsKmsStellarService> AwsKmsSigner<T> {
    /// Creates a new AwsKmsSigner with a custom service implementation for testing
    pub fn new_for_testing(aws_kms_service: T) -> Self {
        Self { aws_kms_service }
    }
}

#[async_trait]
impl<T: AwsKmsStellarService> Signer for AwsKmsSigner<T> {
    async fn address(&self) -> Result<Address, SignerError> {
        self.aws_kms_service
            .get_stellar_address()
            .await
            .map_err(|e| SignerError::KeyError(e.to_string()))
    }

    async fn sign_transaction(
        &self,
        tx: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        let stellar_data = tx
            .get_stellar_transaction_data()
            .map_err(|e| SignerError::SigningError(format!("Failed to get tx data: {e}")))?;

        let passphrase = &stellar_data.network_passphrase;
        let hash_bytes: [u8; 32] = Sha256::digest(passphrase.as_bytes()).into();
        let network_id = Hash(hash_bytes);

        // Sign based on transaction input type
        let signature = match &stellar_data.transaction_input {
            crate::models::TransactionInput::Operations(_) => {
                // Build transaction from operations and sign
                let transaction = Transaction::try_from(stellar_data).map_err(|e| {
                    SignerError::SigningError(format!(
                        "Failed to build Stellar transaction from operations: {e}"
                    ))
                })?;

                self.sign_transaction_directly(&transaction, &network_id)
                    .await?
            }
            crate::models::TransactionInput::UnsignedXdr(xdr)
            | crate::models::TransactionInput::SignedXdr { xdr, .. } => {
                // Parse the XDR envelope and sign
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

        Ok(SignTransactionResponse::Stellar(
            crate::domain::SignTransactionResponseStellar { signature },
        ))
    }
}

impl<T: AwsKmsStellarService> AwsKmsSigner<T> {
    /// Sign a transaction envelope
    async fn sign_envelope(
        &self,
        envelope: &TransactionEnvelope,
        network_id: &Hash,
    ) -> Result<DecoratedSignature, SignerError> {
        // Create the appropriate signature payload based on envelope type
        let payload = create_signature_payload(envelope, network_id)
            .map_err(|e| SignerError::SigningError(format!("Failed to create payload: {e}")))?;

        // Serialize and hash the payload
        let payload_bytes = payload
            .to_xdr(Limits::none())
            .map_err(|e| SignerError::SigningError(format!("Failed to serialize payload: {e}")))?;

        let hash = Sha256::digest(&payload_bytes);

        // Sign the hash using AWS KMS
        let signature_bytes = self
            .aws_kms_service
            .sign_stellar(&hash)
            .await
            .map_err(|e| {
                SignerError::SigningError(format!("AWS KMS signing operation failed: {e}"))
            })?;

        // Create decorated signature with improved error handling
        self.create_decorated_signature(signature_bytes).await
    }

    /// Sign a transaction directly from Transaction struct
    async fn sign_transaction_directly(
        &self,
        transaction: &Transaction,
        network_id: &Hash,
    ) -> Result<DecoratedSignature, SignerError> {
        // Create signature payload for the transaction
        let payload = create_transaction_signature_payload(transaction, network_id);

        // Serialize and hash the payload
        let payload_bytes = payload
            .to_xdr(Limits::none())
            .map_err(|e| SignerError::SigningError(format!("Failed to serialize payload: {e}")))?;

        let hash = Sha256::digest(&payload_bytes);

        // Sign the hash using AWS KMS
        let signature_bytes = self
            .aws_kms_service
            .sign_stellar(&hash)
            .await
            .map_err(|e| {
                SignerError::SigningError(format!("AWS KMS signing operation failed: {e}"))
            })?;

        // Create decorated signature with improved error handling
        self.create_decorated_signature(signature_bytes).await
    }

    /// Helper function to create a DecoratedSignature from signature bytes
    async fn create_decorated_signature(
        &self,
        signature_bytes: Vec<u8>,
    ) -> Result<DecoratedSignature, SignerError> {
        // Validate signature length for Ed25519
        if signature_bytes.len() != 64 {
            return Err(SignerError::SigningError(format!(
                "AWS KMS returned invalid Ed25519 signature length: expected 64 bytes, got {}",
                signature_bytes.len()
            )));
        }

        let hint = self.get_signature_hint().await?;

        // Convert signature bytes to BytesM<64>
        let signature_bytes_m =
            soroban_rs::xdr::BytesM::try_from(signature_bytes).map_err(|_| {
                SignerError::SigningError(
                    "Failed to convert signature to BytesM format".to_string(),
                )
            })?;

        Ok(DecoratedSignature {
            hint,
            signature: Signature(signature_bytes_m),
        })
    }

    /// Get the signature hint for this signer (last 4 bytes of the public key)
    async fn get_signature_hint(&self) -> Result<SignatureHint, SignerError> {
        // Get the public key to derive the signature hint
        let stellar_address = self
            .aws_kms_service
            .get_stellar_address()
            .await
            .map_err(|e| {
                SignerError::SigningError(format!(
                    "Failed to retrieve Stellar address from AWS KMS: {e}"
                ))
            })?;

        // Extract hint from the public key (last 4 bytes of public key)
        match stellar_address {
            Address::Stellar(addr) => {
                // Parse the Stellar address to get the public key
                use stellar_strkey::ed25519::PublicKey;
                let pk = PublicKey::from_string(&addr).map_err(|e| {
                    SignerError::SigningError(format!(
                        "Failed to parse Stellar address '{addr}': {e}"
                    ))
                })?;
                let pk_bytes = pk.0;

                // Safety check: ensure we have enough bytes for the hint
                if pk_bytes.len() < 4 {
                    return Err(SignerError::SigningError(format!(
                        "Public key too short for signature hint: {} bytes",
                        pk_bytes.len()
                    )));
                }

                let hint_bytes: [u8; 4] =
                    pk_bytes[pk_bytes.len() - 4..].try_into().map_err(|_| {
                        SignerError::SigningError(
                            "Failed to create signature hint from public key".to_string(),
                        )
                    })?;
                Ok(SignatureHint(hint_bytes))
            }
            _ => Err(SignerError::SigningError(format!(
                "Expected Stellar address, got: {stellar_address:?}"
            ))),
        }
    }
}

#[async_trait]
impl<T: AwsKmsStellarService> StellarSignTrait for AwsKmsSigner<T> {
    async fn sign_xdr_transaction(
        &self,
        unsigned_xdr: &str,
        network_passphrase: &str,
    ) -> Result<SignXdrTransactionResponseStellar, SignerError> {
        debug!("Signing Stellar XDR transaction with AWS KMS");

        // Parse the unsigned XDR
        let mut envelope = parse_transaction_xdr(unsigned_xdr, false)
            .map_err(|e| SignerError::SigningError(format!("Invalid XDR: {e}")))?;

        // Create network ID from passphrase
        let hash_bytes: [u8; 32] = Sha256::digest(network_passphrase.as_bytes()).into();
        let network_id = Hash(hash_bytes);

        // Sign the envelope
        let signature = self.sign_envelope(&envelope, &network_id).await?;

        // Attach the signature to the envelope
        attach_signatures_to_envelope(&mut envelope, vec![signature.clone()])
            .map_err(|e| SignerError::SigningError(format!("Failed to attach signature: {e}")))?;

        // Serialize the signed envelope
        let signed_xdr = envelope.to_xdr_base64(Limits::none()).map_err(|e| {
            SignerError::SigningError(format!("Failed to serialize signed XDR: {e}"))
        })?;

        Ok(SignXdrTransactionResponseStellar {
            signed_xdr,
            signature,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::{StellarTransactionData, TransactionInput},
        services::{AwsKmsError, MockAwsKmsStellarService},
    };
    use soroban_rs::xdr::{SequenceNumber, TransactionV0, TransactionV0Envelope, Uint256};
    use stellar_strkey::ed25519::PublicKey;

    #[tokio::test]
    async fn test_address() {
        use stellar_strkey::ed25519::PublicKey as StrKeyPublicKey;
        let test_pk = StrKeyPublicKey([0u8; 32]);
        let test_address = test_pk.to_string();

        let mut mock_service = MockAwsKmsStellarService::new();
        let test_address_for_mock = test_address.clone();
        mock_service
            .expect_get_stellar_address()
            .times(1)
            .returning(move || {
                let addr = test_address_for_mock.clone();
                Box::pin(async { Ok(Address::Stellar(addr)) })
            });

        let signer = AwsKmsSigner::new_for_testing(mock_service);
        let result = signer.address().await.unwrap();

        match result {
            Address::Stellar(addr) => {
                assert_eq!(addr, test_address);
            }
            _ => panic!("Expected Stellar address"),
        }
    }

    #[tokio::test]
    async fn test_sign_xdr_transaction_success() {
        // Create test Stellar address - use all zeros public key
        use stellar_strkey::ed25519::PublicKey as StrKeyPublicKey;
        let test_pk = StrKeyPublicKey([0u8; 32]);
        let test_address = test_pk.to_string();
        let source_pk = PublicKey::from_string(&test_address).unwrap();

        // Create a simple unsigned transaction envelope
        let tx = TransactionV0 {
            source_account_ed25519: Uint256(source_pk.0),
            fee: 100,
            seq_num: SequenceNumber(1),
            time_bounds: None,
            memo: soroban_rs::xdr::Memo::None,
            operations: vec![].try_into().unwrap(),
            ext: soroban_rs::xdr::TransactionV0Ext::V0,
        };

        let envelope = TransactionEnvelope::TxV0(TransactionV0Envelope {
            tx,
            signatures: vec![].try_into().unwrap(), // No signatures - unsigned
        });

        let unsigned_xdr = envelope.to_xdr_base64(Limits::none()).unwrap();
        let network_passphrase = "Test SDF Network ; September 2015";

        let mut mock_service = MockAwsKmsStellarService::new();

        // Mock getting the address (called once for hint)
        let test_address_for_mock = test_address.clone();
        mock_service
            .expect_get_stellar_address()
            .times(1)
            .returning(move || {
                let addr = test_address_for_mock.clone();
                Box::pin(async move { Ok(Address::Stellar(addr)) })
            });

        // Mock signing
        mock_service.expect_sign_stellar().times(1).returning(|_| {
            let sig = vec![1u8; 64]; // Valid Ed25519 signature length
            Box::pin(async move { Ok(sig) })
        });

        let signer = AwsKmsSigner::new_for_testing(mock_service);

        let result = signer
            .sign_xdr_transaction(&unsigned_xdr, network_passphrase)
            .await
            .unwrap();

        // Verify the response
        assert!(!result.signed_xdr.is_empty());
        assert_eq!(result.signature.hint.0.len(), 4);
        assert_eq!(result.signature.signature.0.len(), 64);

        // Verify the signed XDR can be parsed back
        let signed_envelope =
            TransactionEnvelope::from_xdr_base64(&result.signed_xdr, Limits::none())
                .expect("Should be able to parse signed XDR");

        // Verify it now has a signature
        match signed_envelope {
            TransactionEnvelope::TxV0(v0_env) => {
                assert_eq!(
                    v0_env.signatures.len(),
                    1,
                    "Should have exactly one signature"
                );
            }
            _ => panic!("Expected V0 envelope"),
        }
    }

    #[tokio::test]
    async fn test_sign_xdr_transaction_invalid_xdr() {
        let mock_service = MockAwsKmsStellarService::new();

        let signer = AwsKmsSigner::new_for_testing(mock_service);
        let invalid_xdr = "INVALID_BASE64_XDR_DATA";
        let network_passphrase = "Test SDF Network ; September 2015";

        let result = signer
            .sign_xdr_transaction(invalid_xdr, network_passphrase)
            .await;

        assert!(result.is_err());
        match result.err().unwrap() {
            SignerError::SigningError(msg) => {
                assert!(msg.contains("Invalid XDR"));
            }
            _ => panic!("Expected SigningError"),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_with_operations_input_success() {
        use stellar_strkey::ed25519::PublicKey as StrKeyPublicKey;
        let test_pk = StrKeyPublicKey([0u8; 32]);
        let test_address = test_pk.to_string();

        let mut mock_service = MockAwsKmsStellarService::new();
        mock_service.expect_sign_stellar().times(1).returning(|_| {
            Box::pin(async {
                Ok(vec![0u8; 64]) // 64-byte Ed25519 signature
            })
        });
        let test_address_for_mock = test_address.clone();
        mock_service
            .expect_get_stellar_address()
            .times(1)
            .returning(move || {
                let addr = test_address_for_mock.clone();
                Box::pin(async { Ok(Address::Stellar(addr)) })
            });

        let signer = AwsKmsSigner::new_for_testing(mock_service);

        let tx_data = StellarTransactionData {
            source_account: test_address,
            fee: Some(100),
            sequence_number: Some(1),
            transaction_input: TransactionInput::Operations(vec![]),
            memo: None,
            valid_until: None,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            signatures: Vec::new(),
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
            transaction_result_xdr: None,
        };

        let result = signer
            .sign_transaction(NetworkTransactionData::Stellar(tx_data))
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_address_error_handling() {
        let mut mock_service = MockAwsKmsStellarService::new();
        mock_service
            .expect_get_stellar_address()
            .times(1)
            .returning(|| {
                Box::pin(async { Err(AwsKmsError::GetError("Invalid configuration".to_string())) })
            });

        let signer = AwsKmsSigner::new_for_testing(mock_service);
        let result = signer.address().await;

        assert!(result.is_err());
        match result.err().unwrap() {
            SignerError::KeyError(msg) => {
                assert!(msg.contains("Invalid configuration"));
            }
            _ => panic!("Expected KeyError"),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_with_invalid_signature_length() {
        use stellar_strkey::ed25519::PublicKey as StrKeyPublicKey;
        let test_pk = StrKeyPublicKey([0u8; 32]);
        let test_address = test_pk.to_string();

        let mut mock_service = MockAwsKmsStellarService::new();
        mock_service.expect_sign_stellar().times(1).returning(|_| {
            Box::pin(async {
                Ok(vec![0u8; 32]) // Invalid length: 32 bytes instead of 64
            })
        });

        let signer = AwsKmsSigner::new_for_testing(mock_service);
        let tx_data = StellarTransactionData {
            source_account: test_address,
            fee: Some(100),
            sequence_number: Some(1),
            transaction_input: TransactionInput::Operations(vec![]),
            memo: None,
            valid_until: None,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            signatures: Vec::new(),
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
            transaction_result_xdr: None,
        };

        let result = signer
            .sign_transaction(NetworkTransactionData::Stellar(tx_data))
            .await;

        assert!(result.is_err());
        match result.err().unwrap() {
            SignerError::SigningError(msg) => {
                assert!(msg.contains("invalid Ed25519 signature length"));
                assert!(msg.contains("expected 64 bytes, got 32"));
            }
            _ => panic!("Expected SigningError about signature length"),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_with_kms_service_error() {
        use stellar_strkey::ed25519::PublicKey as StrKeyPublicKey;
        let test_pk = StrKeyPublicKey([0u8; 32]);
        let test_address = test_pk.to_string();

        let mut mock_service = MockAwsKmsStellarService::new();
        mock_service.expect_sign_stellar().times(1).returning(|_| {
            Box::pin(async {
                Err(AwsKmsError::SignError(
                    "KMS service unavailable".to_string(),
                ))
            })
        });

        let signer = AwsKmsSigner::new_for_testing(mock_service);
        let tx_data = StellarTransactionData {
            source_account: test_address,
            fee: Some(100),
            sequence_number: Some(1),
            transaction_input: TransactionInput::Operations(vec![]),
            memo: None,
            valid_until: None,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            signatures: Vec::new(),
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
            transaction_result_xdr: None,
        };

        let result = signer
            .sign_transaction(NetworkTransactionData::Stellar(tx_data))
            .await;

        assert!(result.is_err());
        match result.err().unwrap() {
            SignerError::SigningError(msg) => {
                assert!(msg.contains("AWS KMS signing operation failed"));
                assert!(msg.contains("KMS service unavailable"));
            }
            _ => panic!("Expected SigningError about KMS service"),
        }
    }
}
