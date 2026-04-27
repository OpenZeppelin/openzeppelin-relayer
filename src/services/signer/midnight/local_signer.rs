use async_trait::async_trait;
use ed25519_dalek::{Signer as Ed25519Signer, SigningKey};
use eyre::Result;
use sha2::{Digest, Sha256};

use crate::{
    domain::SignTransactionResponse,
    models::{Address, NetworkTransactionData, Signer as SignerDomainModel, SignerError},
    services::signer::Signer,
};

/// All derived addresses and keys for a Midnight wallet.
struct MidnightWalletKeys {
    unshielded_address: String,
    shielded_address: String,
    dust_address: String,
    viewing_key: String,
}

/// Local Midnight signer using Ed25519 keys.
///
/// Derives the bech32m unshielded address and viewing key directly from the
/// wallet seed using `midnight-node-ledger-helpers` and the `bech32` crate.
/// No external keygen script or env vars needed.
pub struct LocalSigner {
    signing_key: SigningKey,
    raw_key: [u8; 32],
    address: String,
    shielded_address: String,
    dust_address: String,
    viewing_key: String,
}

impl LocalSigner {
    pub fn new(signer_model: &SignerDomainModel, network_name: &str) -> Result<Self, SignerError> {
        let config = signer_model
            .config
            .get_local()
            .ok_or_else(|| SignerError::Configuration("Local config not found".into()))?;

        let key_slice = config.raw_key.borrow();
        let raw_key: [u8; 32] = <[u8; 32]>::try_from(&key_slice[..])
            .map_err(|_| SignerError::Configuration("Private key must be 32 bytes".into()))?;

        let signing_key = SigningKey::from_bytes(&raw_key);

        // Derive address and viewing key from the midnight wallet
        let keys = Self::derive_midnight_keys(&raw_key, network_name)?;

        Ok(Self {
            signing_key,
            raw_key,
            address: keys.unshielded_address,
            shielded_address: keys.shielded_address,
            dust_address: keys.dust_address,
            viewing_key: keys.viewing_key,
        })
    }

    /// Derive bech32m unshielded address and viewing key from the wallet seed.
    ///
    /// Uses midnight-node-ledger-helpers to derive the wallet, then bech32m-encodes
    /// the verifying key (for address) and encryption secret key (for viewing key).
    fn derive_midnight_keys(
        seed: &[u8; 32],
        network: &str,
    ) -> Result<MidnightWalletKeys, SignerError> {
        use midnight_node_ledger_helpers::{
            DefaultDB, DustWallet, IntoWalletAddress, LedgerContext, ShieldedWallet,
            UnshieldedWallet, WalletSeed,
        };

        let wallet_seed = WalletSeed::Medium(*seed);
        let context =
            LedgerContext::<DefaultDB>::new_from_wallet_seeds("".to_string(), &[wallet_seed]);

        let (unshielded_address, shielded_address, dust_address, viewing_key) = context
            .with_wallet_from_seed(wallet_seed, |wallet| {
                let unshielded = wallet.unshielded.address(network).to_bech32();
                let shielded = wallet.shielded.address(network).to_bech32();
                let dust = wallet.dust.address(network).to_bech32();
                let vk = wallet.shielded.viewing_key(network);
                (unshielded, shielded, dust, vk)
            });

        Ok(MidnightWalletKeys {
            unshielded_address,
            shielded_address,
            dust_address,
            viewing_key,
        })
    }

    fn bech32m_encode(data: &[u8], hrp: &str) -> Result<String, SignerError> {
        use bech32::{Bech32m, Hrp};

        let hrp = Hrp::parse(hrp)
            .map_err(|e| SignerError::Configuration(format!("Invalid HRP '{hrp}': {e}")))?;

        bech32::encode::<Bech32m>(hrp, data)
            .map_err(|e| SignerError::Configuration(format!("Bech32m encode failed: {e}")))
    }

    /// Get the raw 32-byte seed.
    pub fn raw_key(&self) -> &[u8; 32] {
        &self.raw_key
    }

    /// Shielded address (for private ZK transfers).
    pub fn shielded_address(&self) -> &str {
        &self.shielded_address
    }

    /// DUST address (for fee token generation).
    pub fn dust_address(&self) -> &str {
        &self.dust_address
    }

    /// Return the viewing key for indexer wallet sync.
    pub fn viewing_key(&self) -> crate::services::sync::midnight::indexer::ViewingKeyFormat {
        crate::services::sync::midnight::indexer::ViewingKeyFormat::Bech32m(
            self.viewing_key.clone(),
        )
    }
}

#[async_trait]
impl Signer for LocalSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        Ok(Address::Midnight(self.address.clone()))
    }

    async fn sign_transaction(
        &self,
        transaction: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        let raw_bytes = match &transaction {
            NetworkTransactionData::Midnight(data) => {
                let signable = data.hash.as_deref().ok_or_else(|| {
                    SignerError::SigningError("Midnight transaction has no hash to sign".into())
                })?;
                hex::decode(signable.trim_start_matches("0x")).map_err(|e| {
                    SignerError::SigningError(format!("Invalid hex in transaction hash: {e}"))
                })?
            }
            _ => {
                return Err(SignerError::SigningError(
                    "Expected Midnight transaction data".into(),
                ))
            }
        };

        let hash = Sha256::digest(&raw_bytes);
        let signature = self.signing_key.sign(&hash);

        Ok(SignTransactionResponse::Midnight(
            crate::domain::SignTransactionResponseMidnight {
                signature: hex::encode(signature.to_bytes()),
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{LocalSignerConfig, MidnightTransactionData, SignerConfig};
    use secrets::SecretVec;

    fn make_signer_model(key: [u8; 32]) -> crate::models::Signer {
        crate::models::Signer {
            id: "test-midnight-signer".into(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: SecretVec::new(32, |v| v.copy_from_slice(&key)),
            }),
        }
    }

    #[test]
    fn test_local_signer_derives_bech32m_address() {
        let model = make_signer_model([1u8; 32]);
        let signer = LocalSigner::new(&model, "preview").unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let addr = rt.block_on(signer.address()).unwrap();
        match addr {
            Address::Midnight(a) => {
                assert!(a.starts_with("mn_addr_"), "got: {a}");
            }
            _ => panic!("Expected Midnight address"),
        }
    }

    #[test]
    fn test_local_signer_derives_viewing_key() {
        let model = make_signer_model([1u8; 32]);
        let signer = LocalSigner::new(&model, "preview").unwrap();
        let vk = signer.viewing_key();
        match vk {
            crate::services::sync::midnight::indexer::ViewingKeyFormat::Bech32m(key) => {
                assert!(key.starts_with("mn_shield-esk_"), "got: {key}");
            }
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_produces_valid_signature() {
        let model = make_signer_model([2u8; 32]);
        let signer = LocalSigner::new(&model, "preview").unwrap();

        let tx_data = NetworkTransactionData::Midnight(MidnightTransactionData {
            hash: Some("0xdeadbeef".into()),
            extrinsic_hash: None,
            block_hash: None,
            serialized_tx: None,
            guaranteed_offer: None,
            intents: vec![],
            fallible_offers: vec![],
            fallible_shielded_offers: vec![],
        });

        let response = signer.sign_transaction(tx_data).await.unwrap();
        match response {
            SignTransactionResponse::Midnight(resp) => {
                assert_eq!(resp.signature.len(), 128);
            }
            _ => panic!("Expected Midnight response"),
        }
    }
}
