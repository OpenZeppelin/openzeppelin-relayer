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
use log::debug;
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
use zeroize::{Zeroize, ZeroizeOnDrop};

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
#[derive(Clone, Debug, PartialEq, Eq, Hash, Zeroize, ZeroizeOnDrop)]
struct VaultCacheKey {
    address: String,
    role_id: String,
    namespace: Option<String>,
}

impl fmt::Display for VaultCacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}|{}|{}",
            self.address,
            self.role_id,
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

use crate::models::SecretString;
use crate::utils::base64_encode;

#[derive(Clone)]
pub struct VaultConfig {
    pub address: String,
    pub namespace: Option<String>,
    pub role_id: SecretString,
    pub secret_id: SecretString,
    pub mount_path: String,
    // Optional token TTL in seconds, defaults to 45 minutes if not set
    pub token_ttl: Option<u64>,
}

impl VaultConfig {
    pub fn new(
        address: String,
        role_id: SecretString,
        secret_id: SecretString,
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
            role_id: self.role_id.to_str().to_string(),
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
                    return Ok(Arc::clone(&cached.client));
                }
            }
        }

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
            &self.config.role_id.to_str(),
            &self.config.secret_id.to_str(),
        )
        .await
        .map_err(|e| VaultError::AuthenticationFailed(e.to_string()))?;

        let mut transit_settings_builder = VaultClientSettingsBuilder::default();

        transit_settings_builder
            .address(self.config.address.clone())
            .token(token.client_token.clone())
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
    use serde_json::json;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn test_vault_config_new() {
        let config = VaultConfig::new(
            "https://vault.example.com".to_string(),
            SecretString::new("test-role-id"),
            SecretString::new("test-secret-id"),
            Some("test-namespace".to_string()),
            "test-mount-path".to_string(),
            Some(60),
        );

        assert_eq!(config.address, "https://vault.example.com");
        assert_eq!(config.role_id.to_str().as_str(), "test-role-id");
        assert_eq!(config.secret_id.to_str().as_str(), "test-secret-id");
        assert_eq!(config.namespace, Some("test-namespace".to_string()));
        assert_eq!(config.mount_path, "test-mount-path");
        assert_eq!(config.token_ttl, Some(60));
    }

    #[test]
    fn test_vault_cache_key() {
        let config1 = VaultConfig {
            address: "https://vault1.example.com".to_string(),
            namespace: Some("namespace1".to_string()),
            role_id: SecretString::new("role1"),
            secret_id: SecretString::new("secret1"),
            mount_path: "transit".to_string(),
            token_ttl: None,
        };

        let config2 = VaultConfig {
            address: "https://vault1.example.com".to_string(),
            namespace: Some("namespace1".to_string()),
            role_id: SecretString::new("role1"),
            secret_id: SecretString::new("secret1"),
            mount_path: "different-mount".to_string(),
            token_ttl: None,
        };

        let config3 = VaultConfig {
            address: "https://vault2.example.com".to_string(),
            namespace: Some("namespace1".to_string()),
            role_id: SecretString::new("role1"),
            secret_id: SecretString::new("secret1"),
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
            namespace: Some("my-namespace".to_string()),
        };

        let key_without_namespace = VaultCacheKey {
            address: "https://vault.example.com".to_string(),
            role_id: "role-123".to_string(),
            namespace: None,
        };

        assert_eq!(
            key_with_namespace.to_string(),
            "https://vault.example.com|role-123|my-namespace"
        );

        assert_eq!(
            key_without_namespace.to_string(),
            "https://vault.example.com|role-123|"
        );
    }

    // utility function to setup a mock AppRole login response
    async fn setup_mock_approle_login(
        mock_server: &MockServer,
        role_id: &str,
        secret_id: &str,
        token: &str,
    ) {
        Mock::given(method("POST"))
            .and(path("/v1/auth/approle/login"))
            .and(body_json(json!({
                "role_id": role_id,
                "secret_id": secret_id
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "request_id": "test-request-id",
                "lease_id": "",
                "renewable": false,
                "lease_duration": 0,
                "data": null,
                "wrap_info": null,
                "warnings": null,
                "auth": {
                    "client_token": token,
                    "accessor": "test-accessor",
                    "policies": ["default"],
                    "token_policies": ["default"],
                    "metadata": {
                        "role_name": "test-role"
                    },
                    "lease_duration": 3600,
                    "renewable": true,
                    "entity_id": "test-entity-id",
                    "token_type": "service",
                    "orphan": true
                }
            })))
            .mount(mock_server)
            .await;
    }

    #[tokio::test]
    async fn test_vault_service_auth_failure() {
        let mock_server = MockServer::start().await;

        setup_mock_approle_login(&mock_server, "test-role-id", "test-secret-id", "test-token")
            .await;

        Mock::given(method("GET"))
            .and(path("/v1/test-mount/data/my-secret"))
            .and(header("X-Vault-Token", "test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "request_id": "test-request-id",
                "lease_id": "",
                "renewable": false,
                "lease_duration": 0,
                "data": {
                    "data": {
                        "value": "super-secret-value"
                    },
                    "metadata": {
                        "created_time": "2024-01-01T00:00:00Z",
                        "deletion_time": "",
                        "destroyed": false,
                        "version": 1
                    }
                },
                "wrap_info": null,
                "warnings": null,
                "auth": null
            })))
            .mount(&mock_server)
            .await;

        let config = VaultConfig::new(
            mock_server.uri(),
            SecretString::new("test-role-id-fake"),
            SecretString::new("test-secret-id-fake"),
            None,
            "test-mount".to_string(),
            Some(60),
        );

        let vault_service = VaultService::new(config);

        let secret = vault_service.retrieve_secret("my-secret").await;

        assert!(secret.is_err());

        if let Err(e) = secret {
            assert!(matches!(e, VaultError::AuthenticationFailed(_)));
            assert!(e.to_string().contains("An error occurred with the request"));
        }
    }

    #[tokio::test]
    async fn test_vault_service_retrieve_secret_success() {
        let mock_server = MockServer::start().await;

        setup_mock_approle_login(&mock_server, "test-role-id", "test-secret-id", "test-token")
            .await;

        Mock::given(method("GET"))
            .and(path("/v1/test-mount/data/my-secret"))
            .and(header("X-Vault-Token", "test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "request_id": "test-request-id",
                "lease_id": "",
                "renewable": false,
                "lease_duration": 0,
                "data": {
                    "data": {
                        "value": "super-secret-value"
                    },
                    "metadata": {
                        "created_time": "2024-01-01T00:00:00Z",
                        "deletion_time": "",
                        "destroyed": false,
                        "version": 1
                    }
                },
                "wrap_info": null,
                "warnings": null,
                "auth": null
            })))
            .mount(&mock_server)
            .await;

        let config = VaultConfig::new(
            mock_server.uri(),
            SecretString::new("test-role-id"),
            SecretString::new("test-secret-id"),
            None,
            "test-mount".to_string(),
            Some(60),
        );

        let vault_service = VaultService::new(config);

        let secret = vault_service.retrieve_secret("my-secret").await.unwrap();

        assert_eq!(secret, "super-secret-value");
    }

    #[tokio::test]
    async fn test_vault_service_sign_success() {
        let mock_server = MockServer::start().await;

        setup_mock_approle_login(&mock_server, "test-role-id", "test-secret-id", "test-token")
            .await;

        let message = b"hello world";
        let encoded_message = base64_encode(message);

        Mock::given(method("POST"))
            .and(path("/v1/test-mount/sign/my-signing-key"))
            .and(header("X-Vault-Token", "test-token"))
            .and(body_json(json!({
                "input": encoded_message
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "request_id": "test-request-id",
                "lease_id": "",
                "renewable": false,
                "lease_duration": 0,
                "data": {
                    "signature": "vault:v1:fake-signature",
                    "key_version": 1
                },
                "wrap_info": null,
                "warnings": null,
                "auth": null
            })))
            .mount(&mock_server)
            .await;

        let config = VaultConfig::new(
            mock_server.uri(),
            SecretString::new("test-role-id"),
            SecretString::new("test-secret-id"),
            None,
            "test-mount".to_string(),
            Some(60),
        );

        let vault_service = VaultService::new(config);
        let signature = vault_service.sign("my-signing-key", message).await.unwrap();

        assert_eq!(signature, "vault:v1:fake-signature");
    }

    #[tokio::test]
    async fn test_vault_service_retrieve_secret_failure() {
        let mock_server = MockServer::start().await;

        setup_mock_approle_login(&mock_server, "test-role-id", "test-secret-id", "test-token")
            .await;

        Mock::given(method("GET"))
            .and(path("/v1/test-mount/data/my-secret"))
            .and(header("X-Vault-Token", "test-token"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "errors": ["secret not found:"]
            })))
            .mount(&mock_server)
            .await;

        let config = VaultConfig::new(
            mock_server.uri(),
            SecretString::new("test-role-id"),
            SecretString::new("test-secret-id"),
            None,
            "test-mount".to_string(),
            Some(60),
        );

        let vault_service = VaultService::new(config);

        let result = vault_service.retrieve_secret("my-secret").await;
        assert!(result.is_err());

        if let Err(e) = result {
            assert!(matches!(e, VaultError::ClientError(_)));
            assert!(e
                .to_string()
                .contains("The Vault server returned an error (status code 404)"));
        }
    }

    #[tokio::test]
    async fn test_vault_service_sign_failure() {
        let mock_server = MockServer::start().await;

        setup_mock_approle_login(&mock_server, "test-role-id", "test-secret-id", "test-token")
            .await;

        let message = b"hello world";
        let encoded_message = base64_encode(message);

        Mock::given(method("POST"))
            .and(path("/v1/test-mount/sign/my-signing-key"))
            .and(header("X-Vault-Token", "test-token"))
            .and(body_json(json!({
                "input": encoded_message
            })))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "errors": ["1 error occurred:\n\t* signing key not found"]
            })))
            .mount(&mock_server)
            .await;

        let config = VaultConfig::new(
            mock_server.uri(),
            SecretString::new("test-role-id"),
            SecretString::new("test-secret-id"),
            None,
            "test-mount".to_string(),
            Some(60),
        );

        let vault_service = VaultService::new(config);
        let result = vault_service.sign("my-signing-key", message).await;
        assert!(result.is_err());

        if let Err(e) = result {
            assert!(matches!(e, VaultError::SigningError(_)));
            assert!(e.to_string().contains("Failed to sign with Vault"));
        }
    }
}
