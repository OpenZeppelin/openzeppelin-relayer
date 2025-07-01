//! # Midnight Local Signer Implementation
//!
//! This module provides a local signer implementation for Midnight transactions
//! TODO: Implement Midnight local signer
use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignTransactionResponse,
        SignTransactionResponseMidnight, SignTypedDataRequest,
    },
    models::{Address, NetworkTransactionData, SignerError, SignerRepoModel},
    services::Signer,
};
use async_trait::async_trait;
use ed25519_dalek::Signer as Ed25519Signer;
use ed25519_dalek::{ed25519::signature::SignerMut, SigningKey};
use eyre::Result;
use midnight_node_ledger_helpers::WalletSeed;
use sha2::{Digest, Sha256};
use std::convert::TryInto;

/// Local signer that stores the wallet seed in memory.
///
/// # Security Considerations
/// The wallet seed is stored as a plain value in memory. In production environments,
/// consider using hardware security modules or secure enclaves for key storage.
pub struct LocalSigner {
    wallet_seed: WalletSeed,
}

impl LocalSigner {
    pub fn new(signer_model: &SignerRepoModel) -> Result<Self, SignerError> {
        let config = signer_model
            .config
            .get_local()
            .ok_or_else(|| SignerError::Configuration("Local config not found".into()))?;

        let key_slice = config.raw_key.borrow();

        if key_slice.len() != 32 {
            return Err(SignerError::Configuration(
                "Private key must be 32 bytes".into(),
            ));
        }

        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&key_slice);

        let wallet_seed = WalletSeed::from(key_bytes);

        // Clear the temporary key_bytes
        key_bytes.iter_mut().for_each(|b| *b = 0);

        Ok(Self { wallet_seed })
    }

    /// Returns a reference to the wallet seed.
    ///
    /// # Security Note
    /// This returns a reference to sensitive cryptographic material.
    /// Avoid dereferencing (*) unless absolutely necessary, as it creates
    /// copies in memory that could be exposed. Consider using secure
    /// key storage solutions in production environments.
    pub fn wallet_seed(&self) -> &WalletSeed {
        &self.wallet_seed
    }
}

#[async_trait]
impl Signer for LocalSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        // let account_id = self.local_signer_client.account_id();
        Ok(Address::Midnight("".to_string())) // TODO: Implement address
    }

    async fn sign_transaction(
        &self,
        tx: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        let _midnight_data = tx
            .get_midnight_transaction_data()
            .map_err(|e| SignerError::SigningError(format!("failed to get tx data: {e}")))?;

        // let transaction = Transaction::try_from(midnight_data)
        //     .map_err(|e| SignerError::SigningError(format!("invalid XDR: {e}")))?;

        // let signature = self
        //     .local_signer_client
        //     .sign_transaction(&transaction, &network_id)
        //     .map_err(|e| SignerError::SigningError(format!("failed to sign transaction: {e}")))?;

        Ok(SignTransactionResponse::Midnight(
            SignTransactionResponseMidnight {
                signature: "".to_string(), // TODO: Implement signature
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::network::IndexerUrls,
        models::{
            EvmTransactionData, LocalSignerConfig, MidnightNetwork, MidnightTransactionData,
            SignerConfig,
        },
    };
    use secrets::SecretVec;

    fn _create_test_midnight_network() -> MidnightNetwork {
        MidnightNetwork {
            network: "testnet".to_string(),
            rpc_urls: vec!["https://rpc.testnet.midnight.org".to_string()],
            explorer_urls: None,
            average_blocktime_ms: 5000,
            is_testnet: true,
            tags: vec![],
            indexer_urls: IndexerUrls {
                http: "https://indexer.testnet.midnight.org".to_string(),
                ws: "wss://indexer.testnet.midnight.org".to_string(),
            },
            prover_url: "http://localhost:6300".to_string(),
        }
    }

    fn create_test_signer_model() -> SignerRepoModel {
        let seed = vec![1u8; 32];
        let raw_key = SecretVec::new(32, |v| v.copy_from_slice(&seed));
        SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig { raw_key }),
        }
    }

    // TODO: Implement test_new_local_signer_and_address
    #[tokio::test]
    async fn test_new_local_signer_and_address() {
        // let signer = LocalSigner::new(&create_test_signer_model()).unwrap();
        // let address = signer.address().await.unwrap();
        // match address {
        //     Address::Midnight(addr) => {
        //         assert!(addr.starts_with("mn_"));
        //         assert!(!addr.is_empty());
        //     }
        //     _ => panic!("Expected Midnight address"),
        // }
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

    // TODO: Implement test_sign_transaction_midnight
    #[tokio::test]
    async fn test_sign_transaction_midnight() {
        // let signer = LocalSigner::new(&create_test_signer_model()).unwrap();
        // let source_account = match signer.address().await.unwrap() {
        //     Address::Midnight(addr) => addr,
        //     _ => panic!("Expected Midnight address"),
        // };
        // let tx_data = MidnightTransactionData { hash: None };
        // let response = signer
        //     .sign_transaction(NetworkTransactionData::Midnight(tx_data))
        //     .await
        //     .unwrap();
        // match response {
        //     SignTransactionResponse::Midnight(res) => {
        //         let sig = res.signature;
        //         let hint = sig.hint.0;
        //         let signature = sig.signature.0;
        //         assert_eq!(hint.len(), 4);
        //         assert_eq!(signature.len(), 64);
        //         // signature bytes should not all be zero
        //         assert!(signature.iter().any(|&b| b != 0));
        //     }
        //     _ => panic!("Expected Midnight signature response"),
        // }
    }
}
