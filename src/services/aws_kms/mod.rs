//! # AWS KMS Service Module
//!
//! This module provides integration with AWS KMS for secure key management
//! and cryptographic operations such as public key retrieval and message signing.
//!
//! Currently only EVM is supported.
//!
//! ## Features
//!
//! - Service account authentication using credential providers
//! - Public key retrieval from KMS
//! - Message signing via KMS
//!
//! ## Architecture
//!
//! ```text
//! AwsKmsService (implements AwsKmsEvmService)
//!   ├── Authentication (via AwsKmsClient)
//!   ├── Public Key Retrieval (via AwsKmsClient)
//!   └── Message Signing (via AwsKmsClient)
//! ```
//! is based on
//! ```text
//! AwsKmsClient (implements AwsKmsK256)
//!   ├── Authentication (via shared credentials)
//!   ├── Public Key Retrieval in DER Encoding
//!   └── Message Digest Signing in DER Encoding
//! ```
//! `AwsKmsK256` is mocked with `mockall` for unit testing
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
    utils::{self, derive_ethereum_address_from_der, extract_public_key_from_der},
};

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
    /// Signs a payload using the EVM signing scheme.
    /// Pre-hashes the message with keccak-256.
    async fn sign_payload_evm(&self, payload: &[u8]) -> AwsKmsResult<Vec<u8>>;
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

}

// Global cache - HashMap keyed by kms_key_id
static KMS_DER_PK_CACHE: Lazy<RwLock<HashMap<String, Vec<u8>>>> =
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
            .map_err(|e| AwsKmsError::GetError(e.to_string()))?;

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

#[derive(Debug, Clone)]
pub struct AwsKmsService<T: AwsKmsK256 + Clone = AwsKmsClient> {
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
impl<T: AwsKmsK256 + Clone> AwsKmsService<T> {
    pub fn new_for_testing(client: T, config: AwsKmsSignerConfig) -> Self {
        Self {
            client,
            kms_key_id: config.key_id,
        }
    }
}

impl<T: AwsKmsK256 + Clone> AwsKmsService<T> {
    /// Signs a bytes with the private key stored in AWS KMS.
    ///
    /// Pre-hashes the message with keccak256.
    pub async fn sign_bytes_evm(&self, bytes: &[u8]) -> AwsKmsResult<Vec<u8>> {
        // Create a digest of a message payload
        let digest = keccak256(bytes).0;

        // Sign the digest with the AWS KMS
        // Process the result, extract DER signature
        let der_signature = self.client.sign_digest(&self.kms_key_id, digest).await?;

        // Parse DER into Secp256k1 format
        let mut rs = k256::ecdsa::Signature::from_der(&der_signature)
            .map_err(|e| AwsKmsError::ParseError(e.to_string()))?;

        // Normalize to low-s if necessary
        if let Some(normalized) = rs.normalize_s() {
            rs = normalized;
        }
        let der_pk = self.client.get_der_public_key(&self.kms_key_id).await?;

        // Extract public key from AWS KMS and convert it to an uncompressed 64 pk
        let pk = extract_public_key_from_der(&der_pk)
            .map_err(|e| AwsKmsError::ConvertError(e.to_string()))?;

        // Extract v value from the public key recovery
        let v = utils::recover_public_key(&pk, &rs, bytes)?;

        // Adjust v value for Ethereum legacy transaction.
        let eth_v = 27 + v;

        // Append `v` to a signature bytes
        let mut sig_bytes = rs.to_vec();
        sig_bytes.push(eth_v);

        Ok(sig_bytes)
    }
}

#[async_trait]
impl<T: AwsKmsK256 + Clone> AwsKmsEvmService for AwsKmsService<T> {
    async fn get_evm_address(&self) -> AwsKmsResult<Address> {
        let der = self.client.get_der_public_key(&self.kms_key_id).await?;
        let eth_address = derive_ethereum_address_from_der(&der)
            .map_err(|e| AwsKmsError::ParseError(e.to_string()))?;
        Ok(Address::Evm(eth_address))
    }

    async fn sign_payload_evm(&self, message: &[u8]) -> AwsKmsResult<Vec<u8>> {
        self.sign_bytes_evm(message).await
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
}
