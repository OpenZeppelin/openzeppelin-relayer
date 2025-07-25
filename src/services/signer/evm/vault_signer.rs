//! # Vault Signer for EVM
//!
//! This module provides an EVM signer implementation that uses HashiCorp Vault's KV2 engine
//! for secure key management. The private key is fetched once during signer creation and cached
//! in memory for optimal performance.

use async_trait::async_trait;
use once_cell::sync::Lazy;
use secrets::SecretVec;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use zeroize::Zeroizing;

use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignDataResponseEvm, SignTransactionResponse,
        SignTypedDataRequest,
    },
    models::{
        Address, NetworkTransactionData, Signer as SignerDomainModel, SignerError, SignerRepoModel,
        VaultSignerConfig,
    },
    services::{
        signer::evm::{local_signer::LocalSigner, DataSignerTrait},
        vault::{VaultService, VaultServiceTrait},
        Signer,
    },
};

#[derive(Clone, Eq)]
struct VaultCacheKey {
    signer_id: String,
    address: String,
    namespace: Option<String>,
    key_name: String,
    mount_point: String,
}

impl PartialEq for VaultCacheKey {
    fn eq(&self, other: &Self) -> bool {
        self.signer_id == other.signer_id
            && self.key_name == other.key_name
            && self.mount_point == other.mount_point
            && self.address == other.address
            && self.namespace == other.namespace
    }
}

impl Hash for VaultCacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.signer_id.hash(state);
        self.key_name.hash(state);
        self.mount_point.hash(state);
        self.address.hash(state);
        self.namespace.hash(state);
    }
}

// Global signer cache - HashMap keyed by VaultCacheKey
static VAULT_SIGNER_CACHE: Lazy<RwLock<HashMap<VaultCacheKey, Arc<LocalSigner>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// EVM signer that fetches private keys from HashiCorp Vault KV2 engine
#[derive(Clone)]
pub struct VaultSigner<T>
where
    T: VaultServiceTrait + Clone,
{
    signer_id: String,
    key_name: String,
    address: String,
    namespace: Option<String>,
    mount_point: Option<String>,
    vault_service: T,
    /// Cached local signer
    local_signer: Arc<Mutex<Option<Arc<LocalSigner>>>>,
}

impl<T: VaultServiceTrait + Clone> VaultSigner<T> {
    pub fn new(signer_id: String, vault_config: VaultSignerConfig, vault_service: T) -> Self {
        Self {
            signer_id,
            key_name: vault_config.key_name,
            address: vault_config.address,
            namespace: vault_config.namespace,
            mount_point: vault_config.mount_point,
            vault_service,
            local_signer: Arc::new(Mutex::new(None)),
        }
    }

    /// Ensures the local signer is loaded, using caching for performance
    async fn get_local_signer(&self) -> Result<Arc<LocalSigner>, SignerError> {
        // Fast path: check if already loaded
        {
            let guard = self.local_signer.lock().await;
            if let Some(ref signer) = *guard {
                return Ok(Arc::clone(signer));
            }
        }

        // Check global cache
        let cache_key = self.create_cache_key()?;
        {
            let cache = VAULT_SIGNER_CACHE.read().await;
            if let Some(signer) = cache.get(&cache_key) {
                // Update local cache
                let mut guard = self.local_signer.lock().await;
                *guard = Some(Arc::clone(signer));
                return Ok(Arc::clone(signer));
            }
        }

        // Need to load from vault
        let signer = self.load_signer_from_vault().await?;
        let arc_signer = Arc::new(signer);

        // Update both caches
        {
            let mut cache = VAULT_SIGNER_CACHE.write().await;
            cache.insert(cache_key, Arc::clone(&arc_signer));
        }
        {
            let mut guard = self.local_signer.lock().await;
            *guard = Some(Arc::clone(&arc_signer));
        }

        Ok(arc_signer)
    }

    /// Loads a new signer from vault
    async fn load_signer_from_vault(&self) -> Result<LocalSigner, SignerError> {
        let raw_key = self.fetch_private_key().await?;
        let local_config = crate::models::LocalSignerConfig { raw_key };
        let local_model = SignerDomainModel {
            id: self.key_name.clone(),
            config: crate::models::SignerConfig::Local(local_config),
        };

        LocalSigner::new(&local_model)
    }

    /// Fetches private key from vault with proper error handling
    async fn fetch_private_key(&self) -> Result<SecretVec<u8>, SignerError> {
        let hex_secret = Zeroizing::new(
            self.vault_service
                .retrieve_secret(&self.key_name)
                .await
                .map_err(SignerError::VaultError)?,
        );

        // Validate hex format before decoding
        let trimmed = hex_secret.trim();
        if trimmed.is_empty() {
            return Err(SignerError::KeyError(
                "Empty key received from vault".to_string(),
            ));
        }

        // Remove '0x' prefix if present
        let hex_str = if trimmed.starts_with("0x") || trimmed.starts_with("0X") {
            &trimmed[2..]
        } else {
            trimmed
        };

        // Validate hex characters
        if !hex_str.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(SignerError::KeyError(
                "Invalid hex characters in vault secret".to_string(),
            ));
        }

        // Validate key length (32 bytes = 64 hex chars for secp256k1)
        if hex_str.len() != 64 {
            return Err(SignerError::KeyError(format!(
                "Invalid key length: expected 64 hex characters, got {}",
                hex_str.len()
            )));
        }

        let decoded_bytes = hex::decode(hex_str)
            .map_err(|e| SignerError::KeyError(format!("Failed to decode hex: {}", e)))?;

        Ok(SecretVec::new(decoded_bytes.len(), |buffer| {
            buffer.copy_from_slice(&decoded_bytes);
        }))
    }

    fn create_cache_key(&self) -> Result<VaultCacheKey, SignerError> {
        Ok(VaultCacheKey {
            signer_id: self.signer_id.clone(),
            address: self.address.clone(),
            namespace: self.namespace.clone(),
            key_name: self.key_name.clone(),
            mount_point: self
                .mount_point
                .clone()
                .unwrap_or_else(|| "secret".to_string()),
        })
    }
}

#[async_trait]
impl<T: VaultServiceTrait + Clone> Signer for VaultSigner<T> {
    async fn address(&self) -> Result<Address, SignerError> {
        let signer = self.get_local_signer().await?;
        signer.address().await
    }

    async fn sign_transaction(
        &self,
        transaction: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        let signer = self.get_local_signer().await?;
        signer.sign_transaction(transaction).await
    }
}

#[async_trait]
impl<T: VaultServiceTrait + Clone> DataSignerTrait for VaultSigner<T> {
    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, SignerError> {
        let signer = self.get_local_signer().await?;
        signer.sign_data(request).await
    }

    async fn sign_typed_data(
        &self,
        request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, SignerError> {
        let signer = self.get_local_signer().await?;
        signer.sign_typed_data(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{SecretString, SignerConfig, VaultSignerConfig};
    use crate::services::vault::VaultError;
    use async_trait::async_trait;

    // Mock VaultService for testing
    #[derive(Clone)]
    struct MockVaultService {
        mock_secret: String,
    }

    impl MockVaultService {
        fn new(mock_secret: String) -> Self {
            Self { mock_secret }
        }
    }

    #[async_trait]
    impl VaultServiceTrait for MockVaultService {
        async fn retrieve_secret(&self, _key_name: &str) -> Result<String, VaultError> {
            Ok(self.mock_secret.clone())
        }

        async fn sign(&self, _key_name: &str, _message: &[u8]) -> Result<String, VaultError> {
            Ok("mock_signature".to_string())
        }
    }

    fn create_test_config(key_name: Option<&str>) -> VaultSignerConfig {
        VaultSignerConfig {
            address: "https://vault.test.com".to_string(),
            namespace: Some("test-namespace".to_string()),
            role_id: SecretString::new("test-role-id"),
            secret_id: SecretString::new("test-secret-id"),
            key_name: key_name.unwrap_or("test-key").to_string(),
            mount_point: Some("secret".to_string()),
        }
    }

    #[tokio::test]
    async fn test_valid_private_key() {
        let config = create_test_config(Some(uuid::Uuid::new_v4().to_string().as_str()));
        let mock_private_key = "4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318"; // noboost
        let mock_service = MockVaultService::new(mock_private_key.to_string());
        let signer_id = uuid::Uuid::new_v4().to_string();
        let signer = VaultSigner::new(signer_id, config, mock_service);
        let address_result = signer.address().await;

        assert!(
            address_result.is_ok(),
            "Signer should provide a valid address"
        );
    }

    #[tokio::test]
    async fn test_valid_private_key_with_0x_prefix() {
        let config = create_test_config(Some(uuid::Uuid::new_v4().to_string().as_str()));
        let mock_private_key = "0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318"; // noboost
        let mock_service = MockVaultService::new(mock_private_key.to_string());
        let signer_id = uuid::Uuid::new_v4().to_string();
        let signer = VaultSigner::new(signer_id, config, mock_service);
        let address_result = signer.address().await;

        assert!(address_result.is_ok(), "Signer should handle 0x prefix");
    }

    #[tokio::test]
    async fn test_invalid_hex_characters() {
        let config = create_test_config(Some(uuid::Uuid::new_v4().to_string().as_str()));
        let invalid_hex = "invalid_hex_string_with_non_hex_chars";
        let mock_service = MockVaultService::new(invalid_hex.to_string());
        let signer_id = uuid::Uuid::new_v4().to_string();
        let signer = VaultSigner::new(signer_id, config, mock_service);
        let result = signer.address().await;

        assert!(result.is_err(), "Should fail with invalid hex characters");
        if let Err(SignerError::KeyError(msg)) = result {
            assert!(
                msg.contains("Invalid hex characters"),
                "Error should mention invalid hex characters"
            );
        } else {
            panic!("Expected KeyError for invalid hex characters");
        }
    }

    #[tokio::test]
    async fn test_invalid_key_length() {
        let config = create_test_config(Some(uuid::Uuid::new_v4().to_string().as_str()));
        let short_key = "4c0883a69102937d"; // Too short
        let mock_service = MockVaultService::new(short_key.to_string());
        let signer_id = uuid::Uuid::new_v4().to_string();
        let signer = VaultSigner::new(signer_id, config, mock_service);
        let result = signer.address().await;

        assert!(result.is_err(), "Should fail with invalid key length");
        if let Err(SignerError::KeyError(msg)) = result {
            assert!(
                msg.contains("Invalid key length"),
                "Error should mention invalid key length"
            );
        } else {
            panic!("Expected KeyError for invalid key length");
        }
    }

    #[tokio::test]
    async fn test_empty_key() {
        let config = create_test_config(Some(uuid::Uuid::new_v4().to_string().as_str()));
        let empty_key = "";
        let mock_service = MockVaultService::new(empty_key.to_string());
        let signer_id = uuid::Uuid::new_v4().to_string();
        let signer = VaultSigner::new(signer_id, config, mock_service);
        let result = signer.address().await;

        assert!(result.is_err(), "Should fail with empty key");
        if let Err(SignerError::KeyError(msg)) = result {
            assert!(msg.contains("Empty key"), "Error should mention empty key");
        } else {
            panic!("Expected KeyError for empty key");
        }
    }

    #[tokio::test]
    async fn test_caching_behavior() {
        let config = create_test_config(Some(uuid::Uuid::new_v4().to_string().as_str()));
        let mock_private_key = "4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318"; // noboost
        let mock_service = MockVaultService::new(mock_private_key.to_string());
        let signer_id = uuid::Uuid::new_v4().to_string();
        let signer = VaultSigner::new(signer_id, config, mock_service);

        // First call should load from vault
        let address1 = signer.address().await;
        assert!(address1.is_ok());

        // Second call should use cached version
        let address2 = signer.address().await;
        assert!(address2.is_ok());
        assert_eq!(address1.unwrap(), address2.unwrap());
    }
}
