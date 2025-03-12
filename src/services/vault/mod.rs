//! # Vault Service Module
//!
//! This module provides integration with HashiCorp Vault for secure secret management
//! and cryptographic operations.
//!
//! ## Features
//!
//! - Token-based authentication using AppRole method
//! - Automatic token caching and renewal
//! - Secret retrieval from KV2 secrets engine
//! - Message signing via Vault's Transit engine
//! - Namespace support for Vault Enterprise
//!
//! ## Architecture
//!
//! ```text
//! VaultService (implements VaultServiceTrait)
//!   ├── Authentication (AppRole)
//!   ├── Token Caching
//!   ├── KV2 Secret Operations
//!   └── Transit Signing Operations
//! ```
use async_trait::async_trait;
use core::fmt;
use log::{debug, warn};
use once_cell::sync::Lazy;
use serde::Serialize;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::RwLock;
use vaultrs::{
    auth::approle::login,
    client::{VaultClient, VaultClientSettingsBuilder},
    kv2, transit,
};

#[derive(Error, Debug, Serialize)]
pub enum VaultError {
    #[error("Vault client error: {0}")]
    ClientError(String),

    #[error("Secret not found: {0}")]
    SecretNotFound(String),

    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Signing error: {0}")]
    SigningError(String),
}

// Token cache key to uniquely identify a vault configuration
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct VaultCacheKey {
    address: String,
    role_id: String,
    secret_id: String,
    namespace: Option<String>,
}

impl fmt::Display for VaultCacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}|{}|{}|{}",
            self.address,
            self.role_id,
            self.secret_id,
            self.namespace.as_deref().unwrap_or("")
        )
    }
}

struct TokenCache {
    client: Arc<VaultClient>,
    expiry: Instant,
}

// Global token cache - HashMap keyed by VaultCacheKey
static TOKEN_CACHE: Lazy<RwLock<HashMap<VaultCacheKey, TokenCache>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

#[cfg(test)]
use mockall::automock;

use crate::utils::base64_encode;

#[derive(Clone)]
pub struct VaultConfig {
    pub address: String,
    pub namespace: Option<String>,
    pub role_id: String,
    pub secret_id: String,
    pub mount_path: String,
    // Optional token TTL in seconds, defaults to 45 minutes if not set
    pub token_ttl: Option<u64>,
}

impl VaultConfig {
    pub fn new(
        address: String,
        role_id: String,
        secret_id: String,
        namespace: Option<String>,
        mount_path: String,
        token_ttl: Option<u64>,
    ) -> Self {
        Self {
            address,
            role_id,
            secret_id,
            namespace,
            mount_path,
            token_ttl,
        }
    }

    fn cache_key(&self) -> VaultCacheKey {
        VaultCacheKey {
            address: self.address.clone(),
            role_id: self.role_id.clone(),
            secret_id: self.secret_id.clone(),
            namespace: self.namespace.clone(),
        }
    }
}

#[async_trait]
#[cfg_attr(test, automock)]
pub trait VaultServiceTrait: Send + Sync {
    async fn retrieve_secret(&self, key_name: &str) -> Result<String, VaultError>;
    async fn sign(&self, key_name: &str, message: &[u8]) -> Result<String, VaultError>;
}

#[derive(Clone)]
pub struct VaultService {
    pub config: VaultConfig,
}

impl VaultService {
    pub fn new(config: VaultConfig) -> Self {
        Self { config }
    }

    // Get a cached client or create a new one if cache is empty/expired
    async fn get_client(&self) -> Result<Arc<VaultClient>, VaultError> {
        let cache_key = self.config.cache_key();

        // Try to read from cache first
        {
            let cache = TOKEN_CACHE.read().await;
            if let Some(cached) = cache.get(&cache_key) {
                if Instant::now() < cached.expiry {
                    warn!("Cache hit");
                    return Ok(Arc::clone(&cached.client));
                }
            }
        }

        warn!("Cache miss");
        // Cache miss or expired token, need to acquire write lock and refresh
        let mut cache = TOKEN_CACHE.write().await;
        // Double-check after acquiring write lock
        if let Some(cached) = cache.get(&cache_key) {
            if Instant::now() < cached.expiry {
                return Ok(Arc::clone(&cached.client));
            }
        }

        // Create and authenticate a new client
        let client = self.create_authenticated_client().await?;

        // Determine TTL (defaults to 45 minutes if not specified)
        let ttl = Duration::from_secs(self.config.token_ttl.unwrap_or(45 * 60));

        // Update the cache
        cache.insert(
            cache_key,
            TokenCache {
                client: client.clone(),
                expiry: Instant::now() + ttl,
            },
        );

        Ok(client)
    }

    // Create and authenticate a new vault client
    async fn create_authenticated_client(&self) -> Result<Arc<VaultClient>, VaultError> {
        let mut auth_settings_builder = VaultClientSettingsBuilder::default();
        let address = &self.config.address;
        auth_settings_builder.address(address).verify(true);

        if let Some(namespace) = &self.config.namespace {
            auth_settings_builder.namespace(Some(namespace.clone()));
        }

        let auth_settings = auth_settings_builder.build().map_err(|e| {
            VaultError::ConfigError(format!("Failed to build Vault client settings: {}", e))
        })?;

        let client = VaultClient::new(auth_settings).map_err(|e| {
            VaultError::ConfigError(format!("Failed to create Vault client: {}", e))
        })?;

        let token = login(
            &client,
            "approle",
            &self.config.role_id,
            &self.config.secret_id,
        )
        .await
        .map_err(|e| VaultError::AuthenticationFailed(e.to_string()))?;

        let mut transit_settings_builder = VaultClientSettingsBuilder::default();

        transit_settings_builder
            .address(&self.config.address.clone())
            .token(&token.client_token.clone())
            .verify(true);

        if let Some(namespace) = &self.config.namespace {
            transit_settings_builder.namespace(Some(namespace.clone()));
        }

        let transit_settings = transit_settings_builder.build().map_err(|e| {
            VaultError::ConfigError(format!("Failed to build Vault client settings: {}", e))
        })?;

        let client = Arc::new(VaultClient::new(transit_settings).map_err(|e| {
            VaultError::ConfigError(format!(
                "Failed to create authenticated Vault client: {}",
                e
            ))
        })?);

        Ok(client)
    }
}

#[async_trait]
impl VaultServiceTrait for VaultService {
    async fn retrieve_secret(&self, key_name: &str) -> Result<String, VaultError> {
        let client = self.get_client().await?;

        let secret: serde_json::Value = kv2::read(&*client, &self.config.mount_path, key_name)
            .await
            .map_err(|e| VaultError::ClientError(e.to_string()))?;

        let value = secret["value"]
            .as_str()
            .ok_or_else(|| {
                VaultError::SecretNotFound(format!("Secret value invalid for key: {}", key_name))
            })?
            .to_string();

        Ok(value)
    }

    async fn sign(&self, key_name: &str, message: &[u8]) -> Result<String, VaultError> {
        let client = self.get_client().await?;

        let vault_signature = transit::data::sign(
            &*client,
            &self.config.mount_path,
            key_name,
            &base64_encode(message),
            None,
        )
        .await
        .map_err(|e| VaultError::SigningError(format!("Failed to sign with Vault: {}", e)))?;

        let vault_signature_str = &vault_signature.signature;

        debug!("vault_signature_str: {}", vault_signature_str);

        Ok(vault_signature_str.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vault_config_new() {
        let config = VaultConfig::new(
            "https://vault.example.com".to_string(),
            "test-role-id".to_string(),
            "test-secret-id".to_string(),
            Some("test-namespace".to_string()),
            "test-mount-path".to_string(),
            Some(60),
        );

        assert_eq!(config.address, "https://vault.example.com");
        assert_eq!(config.role_id, "test-role-id");
        assert_eq!(config.secret_id, "test-secret-id");
        assert_eq!(config.namespace, Some("test-namespace".to_string()));
        assert_eq!(config.mount_path, "test-mount-path");
        assert_eq!(config.token_ttl, Some(60));
    }

    #[test]
    fn test_vault_cache_key() {
        let config1 = VaultConfig {
            address: "https://vault1.example.com".to_string(),
            namespace: Some("namespace1".to_string()),
            role_id: "role1".to_string(),
            secret_id: "secret1".to_string(),
            mount_path: "transit".to_string(),
            token_ttl: None,
        };

        let config2 = VaultConfig {
            address: "https://vault1.example.com".to_string(),
            namespace: Some("namespace1".to_string()),
            role_id: "role1".to_string(),
            secret_id: "secret1".to_string(),
            mount_path: "different-mount".to_string(),
            token_ttl: None,
        };

        let config3 = VaultConfig {
            address: "https://vault2.example.com".to_string(),
            namespace: Some("namespace1".to_string()),
            role_id: "role1".to_string(),
            secret_id: "secret1".to_string(),
            mount_path: "transit".to_string(),
            token_ttl: None,
        };

        assert_eq!(config1.cache_key(), config1.cache_key());
        assert_eq!(config1.cache_key(), config2.cache_key());
        assert_ne!(config1.cache_key(), config3.cache_key());
    }

    #[test]
    fn test_vault_cache_key_display() {
        let key_with_namespace = VaultCacheKey {
            address: "https://vault.example.com".to_string(),
            role_id: "role-123".to_string(),
            secret_id: "secret-456".to_string(),
            namespace: Some("my-namespace".to_string()),
        };

        let key_without_namespace = VaultCacheKey {
            address: "https://vault.example.com".to_string(),
            role_id: "role-123".to_string(),
            secret_id: "secret-456".to_string(),
            namespace: None,
        };

        assert_eq!(
            key_with_namespace.to_string(),
            "https://vault.example.com|role-123|secret-456|my-namespace"
        );

        assert_eq!(
            key_without_namespace.to_string(),
            "https://vault.example.com|role-123|secret-456|"
        );
    }
}
