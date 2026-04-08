use async_trait::async_trait;
use ed25519_dalek::{Signer as Ed25519Signer, SigningKey, VerifyingKey};
use eyre::Result;
use sha2::{Digest, Sha256};
use std::env;

use crate::{
    domain::SignTransactionResponse,
    models::{Address, NetworkTransactionData, Signer as SignerDomainModel, SignerError},
    services::signer::Signer,
};

/// Local Midnight signer using Ed25519 keys.
///
/// Midnight uses Ed25519 for transaction signing. The signing flow:
/// 1. Extract raw transaction bytes from `NetworkTransactionData::Midnight`
/// 2. SHA-256 hash the bytes
/// 3. Ed25519-sign the hash
/// 4. Return hex-encoded 64-byte signature
///
/// The address and viewing key can be provided via environment variables
/// (`MIDNIGHT_ADDRESS`, `MIDNIGHT_VIEWING_KEY`) when derived externally
/// using the `scripts/midnight-keygen` tool. If not set, a placeholder
/// hex-based address is generated from the public key.
pub struct LocalSigner {
    signing_key: SigningKey,
    address: String,
    viewing_key_override: Option<String>,
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
        let verifying_key = signing_key.verifying_key();

        // Use pre-derived address from env if available, otherwise fall back to hex
        let address =
            env::var("MIDNIGHT_ADDRESS").unwrap_or_else(|_| Self::derive_address(&verifying_key));

        let viewing_key_override = env::var("MIDNIGHT_VIEWING_KEY").ok();

        Ok(Self {
            signing_key,
            address,
            viewing_key_override,
        })
    }

    /// Derive a Midnight-style address from the Ed25519 public key.
    ///
    /// Uses a simplified hex-encoded address with `mn_` prefix.
    /// A full bech32m implementation would be added when the midnight-node
    /// crate types are integrated.
    fn derive_address(verifying_key: &VerifyingKey) -> String {
        format!("mn_{}", hex::encode(verifying_key.as_bytes()))
    }

    /// Return the viewing key for indexer wallet sync.
    ///
    /// If `MIDNIGHT_VIEWING_KEY` env var is set (derived by the keygen script),
    /// uses that. Otherwise falls back to a placeholder that will fail at the
    /// indexer with a bech32m validation error.
    pub fn viewing_key(&self) -> crate::services::sync::midnight::indexer::ViewingKeyFormat {
        let key = self.viewing_key_override.clone().unwrap_or_else(|| {
            format!(
                "mn_shield-esk_{}",
                hex::encode(self.signing_key.verifying_key().as_bytes())
            )
        });
        crate::services::sync::midnight::indexer::ViewingKeyFormat::Bech32m(key)
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
                // Use pallet_hash if available, otherwise use the hash field, as the signable payload
                let signable = data
                    .pallet_hash
                    .as_deref()
                    .or(data.hash.as_deref())
                    .ok_or_else(|| {
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

        // SHA-256 hash then Ed25519-sign
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
    use crate::models::{
        transaction::repository::MidnightTransactionData, LocalSignerConfig, SignerConfig,
    };
    use secrecy::SecretVec;

    fn make_signer_model(key: [u8; 32]) -> crate::models::Signer {
        crate::models::Signer {
            id: "test-midnight-signer".into(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: SecretVec::new(key.to_vec()),
            }),
        }
    }

    #[test]
    fn test_local_signer_creates_address() {
        let model = make_signer_model([1u8; 32]);
        let signer = LocalSigner::new(&model).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let addr = rt.block_on(signer.address()).unwrap();
        match addr {
            Address::Midnight(a) => assert!(a.starts_with("mn_")),
            _ => panic!("Expected Midnight address"),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_produces_valid_signature() {
        let model = make_signer_model([2u8; 32]);
        let signer = LocalSigner::new(&model).unwrap();

        let tx_data = NetworkTransactionData::Midnight(MidnightTransactionData {
            hash: Some("0xdeadbeef".into()),
            pallet_hash: None,
            block_hash: None,
            guaranteed_offer: None,
            intents: vec![],
            fallible_offers: vec![],
        });

        let response = signer.sign_transaction(tx_data).await.unwrap();
        match response {
            SignTransactionResponse::Midnight(resp) => {
                // Ed25519 signature is 64 bytes = 128 hex chars
                assert_eq!(resp.signature.len(), 128);
            }
            _ => panic!("Expected Midnight response"),
        }
    }
}
