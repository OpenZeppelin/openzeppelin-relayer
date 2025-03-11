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

// Global token cache - now a HashMap keyed by VaultCacheKey
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
    async fn list_secrets(&self, path: &str) -> Result<Vec<String>, VaultError>;
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
        // Build client settings
        let mut auth_settings_builder = VaultClientSettingsBuilder::default();
        let address = &self.config.address;
        auth_settings_builder.address(address).verify(true);

        // Add namespace if configured
        if let Some(namespace) = &self.config.namespace {
            auth_settings_builder.namespace(Some(namespace.clone()));
        }

        // Build settings
        let auth_settings = auth_settings_builder.build().map_err(|e| {
            VaultError::ConfigError(format!("Failed to build Vault client settings: {}", e))
        })?;

        // Create client without authentication
        let client = VaultClient::new(auth_settings).map_err(|e| {
            VaultError::ConfigError(format!("Failed to create Vault client: {}", e))
        })?;

        // Authenticate and get the token
        let token = login(
            &client,
            "approle",
            &self.config.role_id,
            &self.config.secret_id,
        )
        .await
        .map_err(|e| VaultError::AuthenticationFailed(e.to_string()))?;

        // Create a new client with the token
        let mut transit_settings_builder = VaultClientSettingsBuilder::default();

        transit_settings_builder
            .address(&self.config.address.clone())
            .token(&token.client_token.clone())
            .verify(true);

        // Add namespace if configured
        if let Some(namespace) = &self.config.namespace {
            transit_settings_builder.namespace(Some(namespace.clone()));
        }

        let transit_settings = transit_settings_builder.build().map_err(|e| {
            VaultError::ConfigError(format!("Failed to build Vault client settings: {}", e))
        })?;

        // Create authenticated client
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
        // Get cached or new authenticated client
        let client = self.get_client().await?;

        // Retrieve the secret using KV2 engine
        let secret: serde_json::Value = kv2::read(&*client, &self.config.mount_path, key_name)
            .await
            .map_err(|e| VaultError::ClientError(e.to_string()))?;

        // Extract the value from the secret
        let value = secret["value"]
            .as_str()
            .ok_or_else(|| {
                VaultError::SecretNotFound(format!("Secret value invalid for key: {}", key_name))
            })?
            .to_string();

        Ok(value)
    }

    async fn list_secrets(&self, path: &str) -> Result<Vec<String>, VaultError> {
        // Get cached or new authenticated client
        let client = self.get_client().await?;

        // List secrets at the given path
        let keys = kv2::list(&*client, &self.config.mount_path, path)
            .await
            .map_err(|e| VaultError::ClientError(e.to_string()))?;

        Ok(keys)
    }

    async fn sign(&self, key_name: &str, message: &[u8]) -> Result<String, VaultError> {
        // Get cached or new authenticated client
        let client = self.get_client().await?;

        // Sign message using Transit engine
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

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use mockito::{mock, server_url};
//     use tokio::test;

//     #[test]
//     async fn test_retrieve_secret_success() {
//         // Create a mock server
//         let mock_server = mockito::Server::new();

//         // Mock AppRole login endpoint
//         let login_mock = mock_server.mock("POST", "/v1/auth/approle/login")
//             .with_status(200)
//             .with_header("content-type", "application/json")
//             .with_body(r#"{"auth":{"client_token":"mock-token"}}"#)
//             .create();

//         // Mock secret retrieval endpoint
//         let secret_mock = mock_server.mock("GET", "/v1/secret/data/test-secret")
//             .match_header("authorization", "Bearer mock-token")
//             .with_status(200)
//             .with_header("content-type", "application/json")
//             .with_body(r#"{"data":{"data":{"value":"test-value"}}}"#)
//             .create();

//         // Create test configuration
//         let config = VaultConfig::new(
//             mock_server.url(),
//             "mock-role-id".to_string(),
//             "mock-secret-id".to_string(),
//             None,
//             Some("secret".to_string()),
//             Some(300), // 5 minutes TTL for testing
//         );

//         // Create service with mocked client
//         let service = VaultService::new(config);

//         // Test retrieving a secret
//         let secret = service.retrieve_secret("test-secret").await;

//         // Verify the mock was called
//         login_mock.assert();
//         secret_mock.assert();

//         // Check the result
//         assert!(secret.is_ok());
//         assert_eq!(secret.unwrap(), "test-value");
//     }

//     #[test]
//     async fn test_token_caching() {
//         // Create a mock server
//         let mock_server = mockito::Server::new();

//         // Mock AppRole login endpoint - should be called only once
//         let login_mock = mock_server.mock("POST", "/v1/auth/approle/login")
//             .expect(1) // This is key - we expect only ONE call
//             .with_status(200)
//             .with_header("content-type", "application/json")
//             .with_body(r#"{"auth":{"client_token":"mock-token"}}"#)
//             .create();

//         // Mock secret retrieval endpoint - will be called twice
//         let secret_mock = mock_server.mock("GET", "/v1/secret/data/test-secret")
//             .expect(2) // We expect TWO calls with the same token
//             .match_header("authorization", "Bearer mock-token")
//             .with_status(200)
//             .with_header("content-type", "application/json")
//             .with_body(r#"{"data":{"data":{"value":"test-value"}}}"#)
//             .create();

//         // Create test configuration with short TTL
//         let config = VaultConfig::new(
//             mock_server.url(),
//             "mock-role-id".to_string(),
//             "mock-secret-id".to_string(),
//             None,
//             Some("secret".to_string()),
//             Some(300), // 5 minutes TTL for testing
//         );

//         // Create service
//         let service = VaultService::new(config);

//         // First call - should authenticate
//         let secret1 = service.retrieve_secret("test-secret").await;
//         assert!(secret1.is_ok());
//         assert_eq!(secret1.unwrap(), "test-value");

//         // Second call - should use cached token
//         let secret2 = service.retrieve_secret("test-secret").await;
//         assert!(secret2.is_ok());
//         assert_eq!(secret2.unwrap(), "test-value");

//         // Verify the login mock was called exactly once
//         login_mock.assert();
//         secret_mock.assert();
//     }

//     // Additional tests for error cases and token expiration...
// }
