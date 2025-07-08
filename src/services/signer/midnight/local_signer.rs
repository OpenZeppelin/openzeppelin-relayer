//! # Midnight Local Signer Implementation
//!
//! This module provides a local signer implementation for Midnight transactions

use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignTransactionResponse,
        SignTransactionResponseMidnight, SignTypedDataRequest,
    },
    models::{Address, MidnightAddress, NetworkTransactionData, SignerError, SignerRepoModel},
    services::Signer,
};
use async_trait::async_trait;
use ed25519_dalek::Signer as Ed25519Signer;
use ed25519_dalek::{ed25519::signature::SignerMut, SigningKey};
use eyre::Result;
use midnight_node_ledger_helpers::{DefaultDB, NetworkId, Wallet, WalletKind, WalletSeed};
use sha2::{Digest, Sha256};
use std::convert::TryInto;

/// Local signer that stores the wallet seed and wallet in memory.
///
/// # Security Considerations
/// The wallet seed and its derived wallet are stored in memory. In production environments,
/// consider using hardware security modules or secure enclaves for key storage.
pub struct LocalSigner {
    network_id: NetworkId,
    wallet_seed: WalletSeed,
    wallet: Wallet<DefaultDB>,
}

impl LocalSigner {
    pub fn new(signer_model: &SignerRepoModel, network_id: NetworkId) -> Result<Self, SignerError> {
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

        // Create the wallet from the seed
        // Using index 0 and NoLegacy wallet kind as default
        let wallet = Wallet::new(wallet_seed, 0, WalletKind::NoLegacy);

        Ok(Self {
            network_id,
            wallet_seed,
            wallet,
        })
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
        // Create the Midnight address using the stored wallet
        let midnight_address = MidnightAddress::from_wallet(&self.wallet, self.network_id);

        // Encode the address to bech32m format
        let encoded_address = midnight_address.encode();

        Ok(Address::Midnight(encoded_address))
    }

    async fn sign_transaction(
        &self,
        tx: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        let midnight_data = tx
            .get_midnight_transaction_data()
            .map_err(|e| SignerError::SigningError(format!("failed to get tx data: {e}")))?;

        // Extract the raw transaction bytes
        let raw_tx = midnight_data
            .raw
            .as_ref()
            .ok_or_else(|| SignerError::SigningError("Transaction data not serialized".into()))?;

        let signing_key = SigningKey::from_bytes(&self.wallet_seed.0);

        // Hash the transaction data with SHA-256
        let mut hasher = Sha256::new();
        hasher.update(raw_tx);
        let tx_hash = hasher.finalize();

        // Sign the transaction hash
        let signature = signing_key.sign(&tx_hash);

        // Convert signature to hex string
        let signature_hex = hex::encode(signature.to_bytes());

        Ok(SignTransactionResponse::Midnight(
            SignTransactionResponseMidnight {
                signature: signature_hex,
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

    #[tokio::test]
    async fn test_new_local_signer_and_address() {
        let signer = LocalSigner::new(&create_test_signer_model(), NetworkId::TestNet).unwrap();
        let address = signer.address().await.unwrap();
        match address {
            Address::Midnight(addr) => {
                assert!(addr.starts_with("mn_"));
                assert!(!addr.is_empty());
                // Verify the address format includes network type
                assert!(addr.contains("shield-addr_test"));
            }
            _ => panic!("Expected Midnight address"),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_invalid_type() {
        let signer = LocalSigner::new(&create_test_signer_model(), NetworkId::TestNet).unwrap();
        let evm_tx = NetworkTransactionData::Evm(EvmTransactionData::default());
        let result = signer.sign_transaction(evm_tx).await;
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(format!("{}", err).contains("failed to get tx data"));
    }

    #[tokio::test]
    async fn test_sign_transaction_midnight() {
        let signer = LocalSigner::new(&create_test_signer_model(), NetworkId::TestNet).unwrap();
        let _source_account = match signer.address().await.unwrap() {
            Address::Midnight(addr) => addr,
            _ => panic!("Expected Midnight address"),
        };

        // Create test transaction data with raw bytes
        let test_tx_bytes = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let tx_data = MidnightTransactionData {
            hash: None,
            pallet_hash: None,
            block_hash: None,
            segment_results: None,
            raw: Some(test_tx_bytes),
            signature: None,
            guaranteed_offer: None,
            intents: vec![],
            fallible_offers: vec![],
        };

        let response = signer
            .sign_transaction(NetworkTransactionData::Midnight(tx_data))
            .await
            .unwrap();

        match response {
            SignTransactionResponse::Midnight(res) => {
                let sig = res.signature;
                // Ed25519 signatures are 64 bytes, hex encoded = 128 chars
                assert_eq!(sig.len(), 128);
                // Verify it's valid hex
                assert!(hex::decode(&sig).is_ok());
                // signature should not be all zeros
                let sig_bytes = hex::decode(&sig).unwrap();
                assert!(sig_bytes.iter().any(|&b| b != 0));
            }
            _ => panic!("Expected Midnight signature response"),
        }
    }
}
