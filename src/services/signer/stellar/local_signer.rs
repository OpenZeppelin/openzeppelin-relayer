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
    models::{Address, NetworkTransactionData, SignerError, SignerRepoModel},
    services::Signer,
};
use async_trait::async_trait;
use ed25519_dalek::Signer as Ed25519Signer;
use ed25519_dalek::{ed25519::signature::SignerMut, SigningKey};
use eyre::Result;
use sha2::{Digest, Sha256};
use soroban_rs::xdr::{
    Hash, Limits, Signature, SignatureHint, Transaction, TransactionEnvelope,
    TransactionSignaturePayload, TransactionSignaturePayloadTaggedTransaction,
    TransactionV1Envelope, VecM, WriteXdr,
};
use soroban_rs::Signer as SorobanSigner;
use std::{convert::TryInto, sync::Arc};

pub struct LocalSigner {
    local_signer_client: SorobanSigner,
}

impl LocalSigner {
    pub fn new(signer_model: &SignerRepoModel) -> Result<Self, SignerError> {
        let config = signer_model
            .config
            .get_local()
            .ok_or_else(|| SignerError::Configuration("Local config not found".into()))?;

        let local_signer_client = {
            let key_slice = config.raw_key.borrow();

            let key_bytes: [u8; 32] = <[u8; 32]>::try_from(&key_slice[..])
                .map_err(|_| SignerError::Configuration("Private key must be 32 bytes".into()))?;

            SorobanSigner::new(SigningKey::from_bytes(&key_bytes))
        };

        Ok(Self {
            local_signer_client,
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

        let transaction = Transaction::try_from(stellar_data)
            .map_err(|e| SignerError::SigningError(format!("invalid XDR: {e}")))?;

        let signature = self
            .local_signer_client
            .sign_transaction(&transaction, &network_id)
            .map_err(|e| SignerError::SigningError(format!("failed to sign transaction: {e}")))?;

        Ok(SignTransactionResponse::Stellar(
            SignTransactionResponseStellar { signature },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        EvmTransactionData, LocalSignerConfig, SignerConfig, StellarTransactionData,
    };
    use secrets::SecretVec;

    fn create_test_signer_model() -> SignerRepoModel {
        let seed = vec![1u8; 32];
        let raw_key = SecretVec::new(32, |v| v.copy_from_slice(&seed));
        SignerRepoModel {
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
            operations: vec![],
            memo: None,
            valid_until: None,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            signatures: Vec::new(),
            hash: None,
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
}
