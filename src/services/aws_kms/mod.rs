//! # AWS KMS Service Module
//!
//! This module provides integration with AWS KMS for secure key management
//! and cryptographic operations such as public key retrieval and message signing.
//!
//! Supports EVM (secp256k1/ECDSA), Solana (Ed25519), and Stellar (Ed25519) networks.
//!
//! ## Features
//!
//! - Service account authentication using credential providers
//! - Public key retrieval from KMS
//! - Message signing via KMS for multiple key types
//!
//! ## Architecture
//!
//! ```text
//! AwsKmsService (implements AwsKmsEvmService, AwsKmsSolanaService, AwsKmsStellarService)
//!   ├── Authentication (via AwsKmsClient)
//!   ├── Public Key Retrieval (via AwsKmsClient)
//!   └── Message Signing (via AwsKmsClient)
//! ```
//! is based on
//! ```text
//! AwsKmsClient (implements AwsKmsK256, AwsKmsEd25519)
//!   ├── Authentication (via shared credentials)
//!   ├── Public Key Retrieval in DER Encoding
//!   └── Message Signing (ECDSA for secp256k1, Ed25519 for EdDSA)
//! ```
//! `AwsKmsK256` and `AwsKmsEd25519` are mocked with `mockall` for unit testing
//! and injected into `AwsKmsService`
//!

use alloy::primitives::keccak256;
use async_trait::async_trait;
use aws_config::{meta::region::RegionProviderChain, BehaviorVersion, Region};
use aws_sdk_kms::{
    primitives::Blob,
    types::{MessageType, SigningAlgorithmSpec},
    Client,
};
use once_cell::sync::Lazy;
use serde::Serialize;
use std::collections::HashMap;
use tokio::sync::RwLock;

use crate::{
    models::{Address, AwsKmsSignerConfig},
    services::signer::evm::utils::recover_evm_signature_from_der,
    utils::{
        self, derive_ethereum_address_from_der, derive_solana_address_from_der,
        derive_stellar_address_from_der,
    },
};
use tracing::debug;

#[cfg(test)]
use mockall::{automock, mock};

#[derive(Clone, Debug, thiserror::Error, Serialize)]
pub enum AwsKmsError {
    #[error("AWS KMS response parse error: {0}")]
    ParseError(String),
    #[error("AWS KMS config error: {0}")]
    ConfigError(String),
    #[error("AWS KMS get error: {0}")]
    GetError(String),
    #[error("AWS KMS signing error: {0}")]
    SignError(String),
    #[error("AWS KMS permissions error: {0}")]
    PermissionError(String),
    #[error("AWS KMS public key error: {0}")]
    RecoveryError(#[from] utils::Secp256k1Error),
    #[error("AWS KMS conversion error: {0}")]
    ConvertError(String),
    #[error("AWS KMS Other error: {0}")]
    Other(String),
}

pub type AwsKmsResult<T> = Result<T, AwsKmsError>;

#[async_trait]
#[cfg_attr(test, automock)]
pub trait AwsKmsEvmService: Send + Sync {
    /// Returns the EVM address derived from the configured public key.
    async fn get_evm_address(&self) -> AwsKmsResult<Address>;
    /// Signs a payload using the EVM signing scheme (hashes before signing).
    ///
    /// This method applies keccak256 hashing before signing.
    ///
    /// **Use for:**
    /// - Raw transaction data (TxLegacy, TxEip1559)
    /// - EIP-191 personal messages
    ///
    /// **Note:** For EIP-712 typed data, use `sign_hash_evm()` to avoid double-hashing.
    async fn sign_payload_evm(&self, payload: &[u8]) -> AwsKmsResult<Vec<u8>>;

    /// Signs a pre-computed hash using the EVM signing scheme (no hashing).
    ///
    /// This method signs the hash directly without applying keccak256.
    ///
    /// **Use for:**
    /// - EIP-712 typed data (already hashed)
    /// - Pre-computed message digests
    ///
    /// **Note:** For raw data, use `sign_payload_evm()` instead.
    async fn sign_hash_evm(&self, hash: &[u8; 32]) -> AwsKmsResult<Vec<u8>>;
}

#[async_trait]
#[cfg_attr(test, automock)]
pub trait AwsKmsK256: Send + Sync {
    /// Fetches the DER-encoded public key from AWS KMS.
    async fn get_der_public_key<'a, 'b>(&'a self, key_id: &'b str) -> AwsKmsResult<Vec<u8>>;
    /// Signs a digest using EcdsaSha256 spec. Returns DER-encoded signature
    async fn sign_digest<'a, 'b>(
        &'a self,
        key_id: &'b str,
        digest: [u8; 32],
    ) -> AwsKmsResult<Vec<u8>>;
}

/// Trait for Ed25519 (EdDSA) operations with AWS KMS.
/// Used for Solana and Stellar signing.
#[async_trait]
#[cfg_attr(test, automock)]
pub trait AwsKmsEd25519: Send + Sync {
    /// Fetches the DER-encoded Ed25519 public key from AWS KMS.
    async fn get_ed25519_public_key<'a, 'b>(&'a self, key_id: &'b str) -> AwsKmsResult<Vec<u8>>;
    /// Signs a message using Ed25519. Returns 64-byte signature.
    /// Uses ED25519_SHA_512 algorithm with RAW message type.
    async fn sign_ed25519<'a, 'b>(
        &'a self,
        key_id: &'b str,
        message: &'b [u8],
    ) -> AwsKmsResult<Vec<u8>>;
}

/// Trait for Solana-specific AWS KMS operations
#[async_trait]
#[cfg_attr(test, automock)]
pub trait AwsKmsSolanaService: Send + Sync {
    /// Returns the Solana address derived from the configured Ed25519 public key.
    async fn get_solana_address(&self) -> AwsKmsResult<Address>;
    /// Signs a message using Ed25519 for Solana.
    async fn sign_solana(&self, message: &[u8]) -> AwsKmsResult<Vec<u8>>;
}

/// Trait for Stellar-specific AWS KMS operations
#[async_trait]
#[cfg_attr(test, automock)]
pub trait AwsKmsStellarService: Send + Sync {
    /// Returns the Stellar address derived from the configured Ed25519 public key.
    async fn get_stellar_address(&self) -> AwsKmsResult<Address>;
    /// Signs a message using Ed25519 for Stellar.
    async fn sign_stellar(&self, message: &[u8]) -> AwsKmsResult<Vec<u8>>;
}

#[cfg(test)]
mock! {
    pub AwsKmsClient { }
    impl Clone for AwsKmsClient {
        fn clone(&self) -> Self;
    }

    #[async_trait]
    impl AwsKmsK256 for AwsKmsClient {
        async fn get_der_public_key<'a, 'b>(&'a self, key_id: &'b str) -> AwsKmsResult<Vec<u8>>;
        async fn sign_digest<'a, 'b>(
            &'a self,
            key_id: &'b str,
            digest: [u8; 32],
        ) -> AwsKmsResult<Vec<u8>>;
    }

    #[async_trait]
    impl AwsKmsEd25519 for AwsKmsClient {
        async fn get_ed25519_public_key<'a, 'b>(&'a self, key_id: &'b str) -> AwsKmsResult<Vec<u8>>;
        async fn sign_ed25519<'a, 'b>(
            &'a self,
            key_id: &'b str,
            message: &'b [u8],
        ) -> AwsKmsResult<Vec<u8>>;
    }
}

// Global cache for secp256k1 public keys - HashMap keyed by kms_key_id
static KMS_DER_PK_CACHE: Lazy<RwLock<HashMap<String, Vec<u8>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

// Global cache for Ed25519 public keys - HashMap keyed by kms_key_id
static KMS_ED25519_PK_CACHE: Lazy<RwLock<HashMap<String, Vec<u8>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

#[derive(Debug, Clone)]
pub struct AwsKmsClient {
    inner: Client,
}

#[async_trait]
impl AwsKmsK256 for AwsKmsClient {
    async fn get_der_public_key<'a, 'b>(&'a self, key_id: &'b str) -> AwsKmsResult<Vec<u8>> {
        // Try cache first with minimal lock time
        let cached = {
            let cache_read = KMS_DER_PK_CACHE.read().await;
            cache_read.get(key_id).cloned()
        };
        if let Some(cached) = cached {
            return Ok(cached);
        }

        // Fetch from AWS KMS
        let get_output = self
            .inner
            .get_public_key()
            .key_id(key_id)
            .send()
            .await
            .map_err(|e| {
                AwsKmsError::GetError(format!(
                    "Failed to get secp256k1 public key for key '{key_id}': {e:?}"
                ))
            })?;

        let der_pk_blob = get_output
            .public_key
            .ok_or(AwsKmsError::GetError(
                "No public key blob found".to_string(),
            ))?
            .into_inner();

        // Cache the result
        let mut cache_write = KMS_DER_PK_CACHE.write().await;
        cache_write.insert(key_id.to_string(), der_pk_blob.clone());
        drop(cache_write);

        Ok(der_pk_blob)
    }

    async fn sign_digest<'a, 'b>(
        &'a self,
        key_id: &'b str,
        digest: [u8; 32],
    ) -> AwsKmsResult<Vec<u8>> {
        // Sign the digest with the AWS KMS
        let sign_result = self
            .inner
            .sign()
            .key_id(key_id)
            .signing_algorithm(SigningAlgorithmSpec::EcdsaSha256)
            .message_type(MessageType::Digest)
            .message(Blob::new(digest))
            .send()
            .await;

        // Process the result, extract DER signature
        let der_signature = sign_result
            .map_err(|e| AwsKmsError::PermissionError(e.to_string()))?
            .signature
            .ok_or(AwsKmsError::SignError(
                "Signature not found in response".to_string(),
            ))?
            .into_inner();

        Ok(der_signature)
    }
}

#[async_trait]
impl AwsKmsEd25519 for AwsKmsClient {
    async fn get_ed25519_public_key<'a, 'b>(&'a self, key_id: &'b str) -> AwsKmsResult<Vec<u8>> {
        // Try cache first with minimal lock time
        let cached = {
            let cache_read = KMS_ED25519_PK_CACHE.read().await;
            cache_read.get(key_id).cloned()
        };
        if let Some(cached) = cached {
            return Ok(cached);
        }

        // Fetch from AWS KMS
        let get_output = self
            .inner
            .get_public_key()
            .key_id(key_id)
            .send()
            .await
            .map_err(|e| {
                AwsKmsError::GetError(format!(
                    "Failed to get Ed25519 public key for key '{key_id}': {e:?}"
                ))
            })?;

        let der_pk_blob = get_output
            .public_key
            .ok_or(AwsKmsError::GetError(
                "No public key blob found".to_string(),
            ))?
            .into_inner();

        // Cache the result
        let mut cache_write = KMS_ED25519_PK_CACHE.write().await;
        cache_write.insert(key_id.to_string(), der_pk_blob.clone());
        drop(cache_write);

        Ok(der_pk_blob)
    }

    async fn sign_ed25519<'a, 'b>(
        &'a self,
        key_id: &'b str,
        message: &'b [u8],
    ) -> AwsKmsResult<Vec<u8>> {
        debug!("Signing Ed25519 message with AWS KMS, key_id: {}", key_id);

        // Sign the message with Ed25519 using ED25519_SHA_512 algorithm
        // Note: ED25519_SHA_512 requires MessageType::Raw - we pass the raw message
        let sign_result = self
            .inner
            .sign()
            .key_id(key_id)
            .signing_algorithm(SigningAlgorithmSpec::Ed25519Sha512)
            .message_type(MessageType::Raw)
            .message(Blob::new(message))
            .send()
            .await;

        // Process the result, extract signature
        let signature = sign_result
            .map_err(|e| AwsKmsError::SignError(e.to_string()))?
            .signature
            .ok_or(AwsKmsError::SignError(
                "Signature not found in response".to_string(),
            ))?
            .into_inner();

        // Ed25519 signatures should be 64 bytes
        if signature.len() != 64 {
            return Err(AwsKmsError::SignError(format!(
                "Invalid Ed25519 signature length: expected 64 bytes, got {}",
                signature.len()
            )));
        }

        Ok(signature)
    }
}

#[derive(Debug, Clone)]
pub struct AwsKmsService<T: AwsKmsK256 + AwsKmsEd25519 + Clone = AwsKmsClient> {
    pub kms_key_id: String,
    client: T,
}

impl AwsKmsService<AwsKmsClient> {
    pub async fn new(config: AwsKmsSignerConfig) -> AwsKmsResult<Self> {
        let region_provider =
            RegionProviderChain::first_try(config.region.map(Region::new)).or_default_provider();

        let auth_config = aws_config::defaults(BehaviorVersion::latest())
            .region(region_provider)
            .load()
            .await;
        let client = AwsKmsClient {
            inner: Client::new(&auth_config),
        };

        Ok(Self {
            kms_key_id: config.key_id,
            client,
        })
    }
}

#[cfg(test)]
impl<T: AwsKmsK256 + AwsKmsEd25519 + Clone> AwsKmsService<T> {
    pub fn new_for_testing(client: T, config: AwsKmsSignerConfig) -> Self {
        Self {
            client,
            kms_key_id: config.key_id,
        }
    }
}

impl<T: AwsKmsK256 + AwsKmsEd25519 + Clone> AwsKmsService<T> {
    /// Common signing logic for EVM signatures.
    ///
    /// This internal helper eliminates duplication between `sign_payload_evm` and `sign_hash_evm`.
    ///
    /// # Parameters
    /// * `digest` - The 32-byte hash to sign
    /// * `original_bytes` - The original message bytes for recovery verification (if applicable)
    /// * `use_prehash_recovery` - If true, recovers using hash directly; if false, uses original bytes
    async fn sign_and_recover_evm(
        &self,
        digest: [u8; 32],
        original_bytes: &[u8],
        use_prehash_recovery: bool,
    ) -> AwsKmsResult<Vec<u8>> {
        // Sign the digest with AWS KMS
        let der_signature = self.client.sign_digest(&self.kms_key_id, digest).await?;

        // Get public key
        let der_pk = self.client.get_der_public_key(&self.kms_key_id).await?;

        // Use shared signature recovery logic
        recover_evm_signature_from_der(
            &der_signature,
            &der_pk,
            digest,
            original_bytes,
            use_prehash_recovery,
        )
        .map_err(|e| AwsKmsError::ParseError(e.to_string()))
    }

    /// Signs a payload using the EVM signing scheme (hashes before signing).
    ///
    /// This method applies keccak256 hashing before signing.
    ///
    /// **Use for:**
    /// - Raw transaction data (TxLegacy, TxEip1559)
    /// - EIP-191 personal messages
    ///
    /// **Note:** For EIP-712 typed data, use `sign_hash_evm()` to avoid double-hashing.
    pub async fn sign_payload_evm(&self, bytes: &[u8]) -> AwsKmsResult<Vec<u8>> {
        let digest = keccak256(bytes).0;
        self.sign_and_recover_evm(digest, bytes, false).await
    }

    /// Signs a pre-computed hash using the EVM signing scheme (no hashing).
    ///
    /// This method signs the hash directly without applying keccak256.
    ///
    /// **Use for:**
    /// - EIP-712 typed data (already hashed)
    /// - Pre-computed message digests
    ///
    /// **Note:** For raw data, use `sign_payload_evm()` instead.
    pub async fn sign_hash_evm(&self, hash: &[u8; 32]) -> AwsKmsResult<Vec<u8>> {
        self.sign_and_recover_evm(*hash, hash, true).await
    }
}

#[async_trait]
impl<T: AwsKmsK256 + AwsKmsEd25519 + Clone> AwsKmsEvmService for AwsKmsService<T> {
    async fn get_evm_address(&self) -> AwsKmsResult<Address> {
        let der = self.client.get_der_public_key(&self.kms_key_id).await?;
        let eth_address = derive_ethereum_address_from_der(&der)
            .map_err(|e| AwsKmsError::ParseError(e.to_string()))?;
        Ok(Address::Evm(eth_address))
    }

    async fn sign_payload_evm(&self, message: &[u8]) -> AwsKmsResult<Vec<u8>> {
        let digest = keccak256(message).0;
        self.sign_and_recover_evm(digest, message, false).await
    }

    async fn sign_hash_evm(&self, hash: &[u8; 32]) -> AwsKmsResult<Vec<u8>> {
        // Delegates to the implementation method on AwsKmsService
        self.sign_and_recover_evm(*hash, hash, true).await
    }
}

#[async_trait]
impl<T: AwsKmsK256 + AwsKmsEd25519 + Clone> AwsKmsSolanaService for AwsKmsService<T> {
    async fn get_solana_address(&self) -> AwsKmsResult<Address> {
        let der = self.client.get_ed25519_public_key(&self.kms_key_id).await?;
        let solana_address = derive_solana_address_from_der(&der)
            .map_err(|e| AwsKmsError::ParseError(e.to_string()))?;
        Ok(Address::Solana(solana_address))
    }

    async fn sign_solana(&self, message: &[u8]) -> AwsKmsResult<Vec<u8>> {
        self.client.sign_ed25519(&self.kms_key_id, message).await
    }
}

#[async_trait]
impl<T: AwsKmsK256 + AwsKmsEd25519 + Clone> AwsKmsStellarService for AwsKmsService<T> {
    async fn get_stellar_address(&self) -> AwsKmsResult<Address> {
        let der = self.client.get_ed25519_public_key(&self.kms_key_id).await?;
        let stellar_address = derive_stellar_address_from_der(&der)
            .map_err(|e| AwsKmsError::ParseError(e.to_string()))?;
        Ok(Address::Stellar(stellar_address))
    }

    async fn sign_stellar(&self, message: &[u8]) -> AwsKmsResult<Vec<u8>> {
        self.client.sign_ed25519(&self.kms_key_id, message).await
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    use alloy::primitives::utils::eip191_message;
    use k256::{
        ecdsa::SigningKey,
        elliptic_curve::rand_core::OsRng,
        pkcs8::{der::Encode, EncodePublicKey},
    };
    use mockall::predicate::{eq, ne};

    /// Test Ed25519 key pair for mocking AWS KMS Ed25519 operations
    pub struct TestEd25519Keys {
        pub public_key_der: Vec<u8>,
        pub public_key_raw: [u8; 32],
    }

    impl TestEd25519Keys {
        pub fn new() -> Self {
            // Well-known test Ed25519 public key (32 bytes)
            let public_key_raw: [u8; 32] = [
                0x9d, 0x45, 0x7e, 0x45, 0xe4, 0x16, 0xc4, 0xc6, 0x77, 0x67, 0x6a, 0x42, 0xff, 0x96,
                0x8e, 0x3c, 0xf8, 0xdc, 0x73, 0xc8, 0xf3, 0x3a, 0x8d, 0x19, 0x81, 0x29, 0x7b, 0xfa,
                0x3e, 0x00, 0x30, 0xba,
            ];

            // Ed25519 SPKI format: 12-byte header + 32-byte key
            let mut public_key_der = vec![
                0x30, 0x2a, // SEQUENCE, 42 bytes
                0x30, 0x05, // SEQUENCE, 5 bytes
                0x06, 0x03, 0x2b, 0x65, 0x70, // OID 1.3.101.112 (Ed25519)
                0x03, 0x21, // BIT STRING, 33 bytes
                0x00, // zero unused bits
            ];
            public_key_der.extend_from_slice(&public_key_raw);

            Self {
                public_key_der,
                public_key_raw,
            }
        }
    }

    pub fn setup_mock_kms_client() -> (MockAwsKmsClient, SigningKey) {
        let mut client = MockAwsKmsClient::new();
        let signing_key = SigningKey::random(&mut OsRng);
        let s = signing_key
            .verifying_key()
            .to_public_key_der()
            .unwrap()
            .to_der()
            .unwrap();

        client
            .expect_get_der_public_key()
            .with(eq("test-key-id"))
            .return_const(Ok(s));
        client
            .expect_get_der_public_key()
            .with(ne("test-key-id"))
            .return_const(Err(AwsKmsError::GetError("Key does not exist".to_string())));

        client
            .expect_sign_digest()
            .withf(|key_id, _| key_id.ne("test-key-id"))
            .return_const(Err(AwsKmsError::SignError(
                "Key does not exist".to_string(),
            )));

        let key = signing_key.clone();
        client
            .expect_sign_digest()
            .withf(|key_id, _| key_id.eq("test-key-id"))
            .returning(move |_, digest| {
                let (signature, _) = signing_key
                    .sign_prehash_recoverable(&digest)
                    .map_err(|e| AwsKmsError::SignError(e.to_string()))?;
                let der_signature = signature.to_der().as_bytes().to_vec();
                Ok(der_signature)
            });

        // Setup Ed25519 mock expectations
        let test_ed25519_keys = TestEd25519Keys::new();
        client
            .expect_get_ed25519_public_key()
            .with(eq("test-key-id"))
            .return_const(Ok(test_ed25519_keys.public_key_der.clone()));
        client
            .expect_get_ed25519_public_key()
            .with(ne("test-key-id"))
            .return_const(Err(AwsKmsError::GetError("Key does not exist".to_string())));

        // Mock Ed25519 signing - return a fixed 64-byte signature
        client
            .expect_sign_ed25519()
            .withf(|key_id, _| key_id.eq("test-key-id"))
            .returning(|_, _| Ok(vec![0u8; 64]));
        client
            .expect_sign_ed25519()
            .withf(|key_id, _| key_id.ne("test-key-id"))
            .return_const(Err(AwsKmsError::SignError(
                "Key does not exist".to_string(),
            )));

        client.expect_clone().return_once(MockAwsKmsClient::new);

        (client, key)
    }

    #[tokio::test]
    async fn test_get_public_key() {
        let (mock_client, key) = setup_mock_kms_client();
        let kms = AwsKmsService::new_for_testing(
            mock_client,
            AwsKmsSignerConfig {
                region: Some("us-east-1".to_string()),
                key_id: "test-key-id".to_string(),
            },
        );

        let result = kms.get_evm_address().await;
        assert!(result.is_ok());
        if let Ok(Address::Evm(evm_address)) = result {
            let expected_address = derive_ethereum_address_from_der(
                key.verifying_key().to_public_key_der().unwrap().as_bytes(),
            )
            .unwrap();
            assert_eq!(expected_address, evm_address);
        }
    }

    #[tokio::test]
    async fn test_get_public_key_fail() {
        let (mock_client, _) = setup_mock_kms_client();
        let kms = AwsKmsService::new_for_testing(
            mock_client,
            AwsKmsSignerConfig {
                region: Some("us-east-1".to_string()),
                key_id: "invalid-key-id".to_string(),
            },
        );

        let result = kms.get_evm_address().await;
        assert!(result.is_err());
        if let Err(err) = result {
            assert!(matches!(err, AwsKmsError::GetError(_)))
        }
    }

    #[tokio::test]
    async fn test_sign_digest() {
        let (mock_client, _) = setup_mock_kms_client();
        let kms = AwsKmsService::new_for_testing(
            mock_client,
            AwsKmsSignerConfig {
                region: Some("us-east-1".to_string()),
                key_id: "test-key-id".to_string(),
            },
        );

        let message_eip = eip191_message(b"Hello World!");
        let result = kms.sign_payload_evm(&message_eip).await;

        // We just assert for Ok, since the pubkey recovery indicates the validity of signature
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sign_digest_fail() {
        let (mock_client, _) = setup_mock_kms_client();
        let kms = AwsKmsService::new_for_testing(
            mock_client,
            AwsKmsSignerConfig {
                region: Some("us-east-1".to_string()),
                key_id: "invalid-key-id".to_string(),
            },
        );

        let message_eip = eip191_message(b"Hello World!");
        let result = kms.sign_payload_evm(&message_eip).await;
        assert!(result.is_err());
        if let Err(err) = result {
            assert!(matches!(err, AwsKmsError::SignError(_)))
        }
    }

    #[tokio::test]
    async fn test_get_solana_address() {
        let (mock_client, _) = setup_mock_kms_client();
        let kms = AwsKmsService::new_for_testing(
            mock_client,
            AwsKmsSignerConfig {
                region: Some("us-east-1".to_string()),
                key_id: "test-key-id".to_string(),
            },
        );

        let result = kms.get_solana_address().await;
        assert!(result.is_ok());
        if let Ok(Address::Solana(solana_address)) = result {
            // Verify it's a valid base58-encoded address
            assert!(!solana_address.is_empty());
            assert!(solana_address.len() >= 32 && solana_address.len() <= 44);
            // Verify it matches the expected address from our test key
            let test_keys = TestEd25519Keys::new();
            let expected_address = bs58::encode(test_keys.public_key_raw).into_string();
            assert_eq!(solana_address, expected_address);
        } else {
            panic!("Expected Solana address");
        }
    }

    #[tokio::test]
    async fn test_get_solana_address_fail() {
        let (mock_client, _) = setup_mock_kms_client();
        let kms = AwsKmsService::new_for_testing(
            mock_client,
            AwsKmsSignerConfig {
                region: Some("us-east-1".to_string()),
                key_id: "invalid-key-id".to_string(),
            },
        );

        let result = kms.get_solana_address().await;
        assert!(result.is_err());
        if let Err(err) = result {
            assert!(matches!(err, AwsKmsError::GetError(_)))
        }
    }

    #[tokio::test]
    async fn test_sign_solana() {
        let (mock_client, _) = setup_mock_kms_client();
        let kms = AwsKmsService::new_for_testing(
            mock_client,
            AwsKmsSignerConfig {
                region: Some("us-east-1".to_string()),
                key_id: "test-key-id".to_string(),
            },
        );

        let message = b"Test Solana message";
        let result = kms.sign_solana(message).await;
        assert!(result.is_ok());
        let signature = result.unwrap();
        assert_eq!(signature.len(), 64); // Ed25519 signatures are 64 bytes
    }

    #[tokio::test]
    async fn test_sign_solana_fail() {
        let (mock_client, _) = setup_mock_kms_client();
        let kms = AwsKmsService::new_for_testing(
            mock_client,
            AwsKmsSignerConfig {
                region: Some("us-east-1".to_string()),
                key_id: "invalid-key-id".to_string(),
            },
        );

        let message = b"Test Solana message";
        let result = kms.sign_solana(message).await;
        assert!(result.is_err());
        if let Err(err) = result {
            assert!(matches!(err, AwsKmsError::SignError(_)))
        }
    }

    #[tokio::test]
    async fn test_get_stellar_address() {
        let (mock_client, _) = setup_mock_kms_client();
        let kms = AwsKmsService::new_for_testing(
            mock_client,
            AwsKmsSignerConfig {
                region: Some("us-east-1".to_string()),
                key_id: "test-key-id".to_string(),
            },
        );

        let result = kms.get_stellar_address().await;
        assert!(result.is_ok());
        if let Ok(Address::Stellar(stellar_address)) = result {
            // Stellar addresses start with 'G' for public accounts
            assert!(stellar_address.starts_with('G'));
            // Stellar addresses are 56 characters long
            assert_eq!(stellar_address.len(), 56);
        } else {
            panic!("Expected Stellar address");
        }
    }

    #[tokio::test]
    async fn test_get_stellar_address_fail() {
        let (mock_client, _) = setup_mock_kms_client();
        let kms = AwsKmsService::new_for_testing(
            mock_client,
            AwsKmsSignerConfig {
                region: Some("us-east-1".to_string()),
                key_id: "invalid-key-id".to_string(),
            },
        );

        let result = kms.get_stellar_address().await;
        assert!(result.is_err());
        if let Err(err) = result {
            assert!(matches!(err, AwsKmsError::GetError(_)))
        }
    }

    #[tokio::test]
    async fn test_sign_stellar() {
        let (mock_client, _) = setup_mock_kms_client();
        let kms = AwsKmsService::new_for_testing(
            mock_client,
            AwsKmsSignerConfig {
                region: Some("us-east-1".to_string()),
                key_id: "test-key-id".to_string(),
            },
        );

        let message = b"Test Stellar message";
        let result = kms.sign_stellar(message).await;
        assert!(result.is_ok());
        let signature = result.unwrap();
        assert_eq!(signature.len(), 64); // Ed25519 signatures are 64 bytes
    }

    #[tokio::test]
    async fn test_sign_stellar_fail() {
        let (mock_client, _) = setup_mock_kms_client();
        let kms = AwsKmsService::new_for_testing(
            mock_client,
            AwsKmsSignerConfig {
                region: Some("us-east-1".to_string()),
                key_id: "invalid-key-id".to_string(),
            },
        );

        let message = b"Test Stellar message";
        let result = kms.sign_stellar(message).await;
        assert!(result.is_err());
        if let Err(err) = result {
            assert!(matches!(err, AwsKmsError::SignError(_)))
        }
    }

    // Note: Ed25519 DER parsing tests are in utils/ed25519.rs
}
