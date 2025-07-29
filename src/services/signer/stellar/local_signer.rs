//! # Stellar Local Signer Implementation
//!
//! This module provides a local signer implementation for Stellar transactions
//! using the `ed25519-dalek` and `soroban-rs` libraries with an in-memory private key.
//!
//! ## Features
//!
//! - Transaction signing for Stellar networks
//! - Integration with `soroban-rs` for Soroban compatibility
//!
//! ## Security Considerations
//!
//! This implementation stores private keys in memory and should primarily be used
//! for development and testing purposes, not production.
use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignTransactionResponse, SignTransactionResponseStellar,
        SignTypedDataRequest,
    },
    models::{
        Address, NetworkTransactionData, Signer as SignerDomainModel, SignerError, TransactionInput,
    },
    services::Signer,
};
use async_trait::async_trait;
use ed25519_dalek::Signer as Ed25519Signer;
use ed25519_dalek::{ed25519::signature::SignerMut, SigningKey};
use eyre::Result;
use sha2::{Digest, Sha256};
use soroban_rs::xdr::{
    DecoratedSignature, Hash, Limits, ReadXdr, Signature, SignatureHint, Transaction,
    TransactionEnvelope, TransactionSignaturePayload, TransactionSignaturePayloadTaggedTransaction,
    Uint256, VecM, WriteXdr,
};
use soroban_rs::Signer as SorobanSigner;
use std::{convert::TryInto, sync::Arc};

pub struct LocalSigner {
    local_signer_client: SorobanSigner,
    signing_key: SigningKey,
}

impl LocalSigner {
    pub fn new(signer_model: &SignerDomainModel) -> Result<Self, SignerError> {
        let config = signer_model
            .config
            .get_local()
            .ok_or_else(|| SignerError::Configuration("Local config not found".into()))?;

        let key_slice = config.raw_key.borrow();
        let key_bytes: [u8; 32] = <[u8; 32]>::try_from(&key_slice[..])
            .map_err(|_| SignerError::Configuration("Private key must be 32 bytes".into()))?;

        let signing_key = SigningKey::from_bytes(&key_bytes);
        let local_signer_client = SorobanSigner::new(signing_key.clone());

        Ok(Self {
            local_signer_client,
            signing_key,
        })
    }

    /// Create a signature payload for the given envelope type
    fn create_signature_payload(
        &self,
        envelope: &TransactionEnvelope,
        network_id: &Hash,
    ) -> Result<TransactionSignaturePayload, SignerError> {
        let tagged_transaction = match envelope {
            TransactionEnvelope::TxV0(e) => {
                // For V0, convert to V1 transaction format for signing
                let v1_tx = self.convert_v0_to_v1_transaction(&e.tx);
                TransactionSignaturePayloadTaggedTransaction::Tx(v1_tx)
            }
            TransactionEnvelope::Tx(e) => {
                TransactionSignaturePayloadTaggedTransaction::Tx(e.tx.clone())
            }
            TransactionEnvelope::TxFeeBump(e) => {
                TransactionSignaturePayloadTaggedTransaction::TxFeeBump(e.tx.clone())
            }
        };

        Ok(TransactionSignaturePayload {
            network_id: network_id.clone(),
            tagged_transaction,
        })
    }

    /// Convert a V0 transaction to V1 format
    fn convert_v0_to_v1_transaction(&self, v0_tx: &soroban_rs::xdr::TransactionV0) -> Transaction {
        Transaction {
            source_account: soroban_rs::xdr::MuxedAccount::Ed25519(
                v0_tx.source_account_ed25519.clone(),
            ),
            fee: v0_tx.fee,
            seq_num: v0_tx.seq_num.clone(),
            cond: match v0_tx.time_bounds.clone() {
                Some(tb) => soroban_rs::xdr::Preconditions::Time(tb),
                None => soroban_rs::xdr::Preconditions::None,
            },
            memo: v0_tx.memo.clone(),
            operations: v0_tx.operations.clone(),
            ext: soroban_rs::xdr::TransactionExt::V0,
        }
    }

    /// Sign a transaction envelope based on its type
    fn sign_envelope(
        &self,
        envelope: &TransactionEnvelope,
        network_id: &Hash,
    ) -> Result<DecoratedSignature, SignerError> {
        // Create the appropriate signature payload based on envelope type
        let payload = self.create_signature_payload(envelope, network_id)?;

        // Serialize and hash the payload
        let payload_bytes = payload
            .to_xdr(Limits::none())
            .map_err(|e| SignerError::SigningError(format!("failed to serialize payload: {e}")))?;

        let hash = sha2::Sha256::digest(&payload_bytes);

        // Sign the hash
        let signature = self.signing_key.sign(&hash);

        // Get the signature hint (last 4 bytes of public key)
        let public_key = self.signing_key.verifying_key();
        let public_key_bytes = public_key.to_bytes();
        let hint_bytes: [u8; 4] = public_key_bytes[public_key_bytes.len() - 4..]
            .try_into()
            .map_err(|_| SignerError::SigningError("failed to create signature hint".into()))?;

        Ok(DecoratedSignature {
            hint: SignatureHint(hint_bytes),
            signature: Signature(signature.to_bytes().try_into().unwrap()),
        })
    }
}

#[async_trait]
impl Signer for LocalSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        let account_id = self.local_signer_client.account_id();
        Ok(Address::Stellar(account_id.to_string()))
    }

    async fn sign_transaction(
        &self,
        tx: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        let stellar_data = tx
            .get_stellar_transaction_data()
            .map_err(|e| SignerError::SigningError(format!("failed to get tx data: {e}")))?;

        let passphrase = &stellar_data.network_passphrase;
        let hash_bytes: [u8; 32] = sha2::Sha256::digest(passphrase.as_bytes()).into();
        let network_id = Hash(hash_bytes);

        // Sign based on transaction input type
        let signature = match &stellar_data.transaction_input {
            TransactionInput::Operations(_) => {
                // Build transaction from operations and sign
                let transaction = Transaction::try_from(stellar_data).map_err(|e| {
                    SignerError::SigningError(format!("invalid transaction data: {e}"))
                })?;

                self.local_signer_client
                    .sign_transaction(&transaction, &network_id)
                    .map_err(|e| {
                        SignerError::SigningError(format!("failed to sign transaction: {e}"))
                    })?
            }
            TransactionInput::UnsignedXdr(xdr) | TransactionInput::SignedXdr { xdr, .. } => {
                // Parse the XDR envelope and sign
                let envelope = TransactionEnvelope::from_xdr_base64(xdr, Limits::none())
                    .map_err(|e| SignerError::SigningError(format!("invalid envelope XDR: {e}")))?;

                self.sign_envelope(&envelope, &network_id)?
            }
        };

        Ok(SignTransactionResponse::Stellar(
            SignTransactionResponseStellar { signature },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        EvmTransactionData, LocalSignerConfig, Signer as SignerDomainModel, SignerConfig,
        StellarTransactionData,
    };
    use secrets::SecretVec;

    fn create_test_signer_model() -> SignerDomainModel {
        let seed = vec![1u8; 32];
        let raw_key = SecretVec::new(32, |v| v.copy_from_slice(&seed));
        SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig { raw_key }),
        }
    }

    #[tokio::test]
    async fn test_new_local_signer_and_address() {
        let signer = LocalSigner::new(&create_test_signer_model()).unwrap();
        let address = signer.address().await.unwrap();
        match address {
            Address::Stellar(addr) => {
                assert!(addr.starts_with('G'));
                assert!(!addr.is_empty());
            }
            _ => panic!("Expected Stellar address"),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_invalid_type() {
        let signer = LocalSigner::new(&create_test_signer_model()).unwrap();
        let evm_tx = NetworkTransactionData::Evm(EvmTransactionData::default());
        let result = signer.sign_transaction(evm_tx).await;
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(format!("{}", err).contains("failed to get tx data"));
    }

    #[tokio::test]
    async fn test_sign_transaction_stellar() {
        let signer = LocalSigner::new(&create_test_signer_model()).unwrap();
        let source_account = match signer.address().await.unwrap() {
            Address::Stellar(addr) => addr,
            _ => panic!("Expected Stellar address"),
        };
        let tx_data = StellarTransactionData {
            source_account: source_account.clone(),
            fee: Some(100),
            sequence_number: Some(1),
            transaction_input: crate::models::TransactionInput::Operations(vec![]),
            memo: None,
            valid_until: None,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            signatures: Vec::new(),
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
        };
        let response = signer
            .sign_transaction(NetworkTransactionData::Stellar(tx_data))
            .await
            .unwrap();
        match response {
            SignTransactionResponse::Stellar(res) => {
                let sig = res.signature;
                let hint = sig.hint.0;
                let signature = sig.signature.0;
                assert_eq!(hint.len(), 4);
                assert_eq!(signature.len(), 64);
                // signature bytes should not all be zero
                assert!(signature.iter().any(|&b| b != 0));
            }
            _ => panic!("Expected Stellar signature response"),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_with_xdr() {
        use soroban_rs::xdr::{SequenceNumber, TransactionV0, TransactionV0Envelope};
        use stellar_strkey::ed25519::PublicKey;

        let signer = LocalSigner::new(&create_test_signer_model()).unwrap();
        let source_account = match signer.address().await.unwrap() {
            Address::Stellar(addr) => addr,
            _ => panic!("Expected Stellar address"),
        };

        // Create a simple unsigned transaction envelope
        let source_pk = PublicKey::from_string(&source_account).unwrap();
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
            signatures: vec![].try_into().unwrap(),
        });

        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();

        let tx_data = StellarTransactionData {
            source_account: source_account.clone(),
            fee: Some(100),
            sequence_number: Some(1),
            transaction_input: crate::models::TransactionInput::UnsignedXdr(xdr),
            memo: None,
            valid_until: None,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            signatures: Vec::new(),
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
        };

        let response = signer
            .sign_transaction(NetworkTransactionData::Stellar(tx_data))
            .await
            .unwrap();

        match response {
            SignTransactionResponse::Stellar(res) => {
                let sig = res.signature;
                assert_eq!(sig.hint.0.len(), 4);
                assert_eq!(sig.signature.0.len(), 64);
                assert!(sig.signature.0.iter().any(|&b| b != 0));
            }
            _ => panic!("Expected Stellar signature response"),
        }
    }

    #[tokio::test]
    async fn test_sign_fee_bump_transaction() {
        use soroban_rs::xdr::{
            FeeBumpTransaction, FeeBumpTransactionInnerTx, MuxedAccount, SequenceNumber,
            TransactionV1Envelope,
        };
        use stellar_strkey::ed25519::PublicKey;

        let signer = LocalSigner::new(&create_test_signer_model()).unwrap();
        let source_account = match signer.address().await.unwrap() {
            Address::Stellar(addr) => addr,
            _ => panic!("Expected Stellar address"),
        };

        let source_pk = PublicKey::from_string(&source_account).unwrap();

        // Create an inner transaction
        let inner_tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256([1u8; 32])), // Different source
            fee: 100,
            seq_num: SequenceNumber(1),
            cond: soroban_rs::xdr::Preconditions::None,
            memo: soroban_rs::xdr::Memo::None,
            operations: vec![].try_into().unwrap(),
            ext: soroban_rs::xdr::TransactionExt::V0,
        };

        let inner_envelope = TransactionV1Envelope {
            tx: inner_tx,
            signatures: vec![DecoratedSignature {
                hint: SignatureHint([0, 0, 0, 0]),
                signature: Signature([0u8; 64].try_into().unwrap()),
            }]
            .try_into()
            .unwrap(), // Mock signature for inner tx
        };

        // Create fee-bump envelope
        let fee_bump_tx = FeeBumpTransaction {
            fee_source: MuxedAccount::Ed25519(Uint256(source_pk.0)),
            fee: 200,
            inner_tx: FeeBumpTransactionInnerTx::Tx(inner_envelope),
            ext: soroban_rs::xdr::FeeBumpTransactionExt::V0,
        };

        let envelope =
            TransactionEnvelope::TxFeeBump(soroban_rs::xdr::FeeBumpTransactionEnvelope {
                tx: fee_bump_tx,
                signatures: vec![].try_into().unwrap(),
            });

        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();

        let tx_data = StellarTransactionData {
            source_account: source_account.clone(),
            fee: Some(200),
            sequence_number: Some(1),
            transaction_input: crate::models::TransactionInput::SignedXdr { xdr, max_fee: 200 },
            memo: None,
            valid_until: None,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            signatures: Vec::new(),
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
        };

        let response = signer
            .sign_transaction(NetworkTransactionData::Stellar(tx_data))
            .await
            .unwrap();

        match response {
            SignTransactionResponse::Stellar(res) => {
                let sig = res.signature;
                assert_eq!(sig.hint.0.len(), 4);
                assert_eq!(sig.signature.0.len(), 64);
                assert!(sig.signature.0.iter().any(|&b| b != 0));
            }
            _ => panic!("Expected Stellar signature response"),
        }
    }
}
