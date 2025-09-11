//! # CDP Service Module
//!
//! This module provides integration with CDP API for secure wallet management
//! and cryptographic operations.
//!
//! ## Features
//!
//! - API key-based authentication via WalletAuth
//! - Digital signature generation for EVM
//! - Message signing via CDP API
//! - Secure transaction signing for blockchain operations
//!
//! ## Architecture
//!
//! ```text
//! CdpService (implements CdpServiceTrait)
//!   ├── Authentication (WalletAuth)
//!   ├── Transaction Signing
//!   └── Raw Payload Signing
//! ```
use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use reqwest_middleware::ClientBuilder;
use std::{str, time::Duration};
use thiserror::Error;

use crate::models::{Address, CdpSignerConfig};

use cdp_sdk::{auth::WalletAuth, types, Client, CDP_BASE_URL};

#[derive(Error, Debug, serde::Serialize)]
pub enum CdpError {
    #[error("HTTP error: {0}")]
    HttpError(String),

    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Signing error: {0}")]
    SigningError(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Invalid signature: {0}")]
    SignatureError(String),

    #[error("Other error: {0}")]
    OtherError(String),
}

/// Result type for CDP operations
pub type CdpResult<T> = Result<T, CdpError>;

#[cfg(test)]
use mockall::automock;

#[async_trait]
#[cfg_attr(test, automock)]
pub trait CdpServiceTrait: Send + Sync {
    /// Returns the EVM or Solana address for the configured account
    async fn account_address(&self) -> Result<Address, CdpError>;

    /// Signs a message using the EVM signing scheme
    async fn sign_evm_message(&self, message: String) -> Result<Vec<u8>, CdpError>;

    /// Signs an EVM transaction using the CDP API
    async fn sign_evm_transaction(&self, message: &[u8]) -> Result<Vec<u8>, CdpError>;

    /// Signs a message using Solana signing scheme
    async fn sign_solana_message(&self, message: &[u8]) -> Result<Vec<u8>, CdpError>;

    /// Signs a transaction using Solana signing scheme
    async fn sign_solana_transaction(&self, message: String) -> Result<Vec<u8>, CdpError>;
}

#[derive(Clone)]
pub struct CdpService {
    pub config: CdpSignerConfig,
    pub client: Client,
}

impl CdpService {
    pub fn new(config: CdpSignerConfig) -> Result<Self, CdpError> {
        // Initialize the CDP client with WalletAuth middleware, which is required for signing operations
        let wallet_auth = WalletAuth::builder()
            .api_key_id(config.api_key_id.clone())
            .api_key_secret(config.api_key_secret.to_str().to_string())
            .wallet_secret(config.wallet_secret.to_str().to_string())
            .source("openzeppelin-relayer".to_string())
            .source_version(env!("CARGO_PKG_VERSION").to_string())
            .build()
            .map_err(|e| CdpError::ConfigError(format!("Invalid CDP configuration: {}", e)))?;

        let inner = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| CdpError::ConfigError(format!("Failed to build HTTP client: {}", e)))?;
        let wallet_client = ClientBuilder::new(inner).with(wallet_auth).build();
        let client = Client::new_with_client(CDP_BASE_URL, wallet_client);
        Ok(Self { config, client })
    }

    /// Get the configured account address
    fn get_account_address(&self) -> &str {
        &self.config.account_address
    }

    /// Check if the configured address is an EVM address (0x-prefixed hex)
    fn is_evm_address(&self) -> bool {
        self.config.account_address.starts_with("0x")
    }

    /// Check if the configured address is a Solana address (Base58)
    fn is_solana_address(&self) -> bool {
        !self.config.account_address.starts_with("0x")
    }

    /// Converts a CDP address to our Address type, auto-detecting format
    fn address_from_string(&self, address_str: &str) -> Result<Address, CdpError> {
        if address_str.starts_with("0x") {
            // EVM address (hex)
            let hex_str = address_str.strip_prefix("0x").unwrap();

            // Decode hex string to bytes
            let bytes = hex::decode(hex_str)
                .map_err(|e| CdpError::ConfigError(format!("Invalid hex address: {}", e)))?;

            if bytes.len() != 20 {
                return Err(CdpError::ConfigError(format!(
                    "EVM address should be 20 bytes, got {} bytes",
                    bytes.len()
                )));
            }

            let mut array = [0u8; 20];
            array.copy_from_slice(&bytes);

            Ok(Address::Evm(array))
        } else {
            // Solana address (Base58)
            Ok(Address::Solana(address_str.to_string()))
        }
    }
}

#[async_trait]
impl CdpServiceTrait for CdpService {
    async fn account_address(&self) -> Result<Address, CdpError> {
        let address_str = self.get_account_address();
        self.address_from_string(address_str)
    }

    async fn sign_evm_message(&self, message: String) -> Result<Vec<u8>, CdpError> {
        if !self.is_evm_address() {
            return Err(CdpError::ConfigError(
                "Account address is not an EVM address (must start with 0x)".to_string(),
            ));
        }
        let address = self.get_account_address();

        let message_body = types::SignEvmMessageBody::builder().message(message);

        let response = self
            .client
            .sign_evm_message()
            .address(address)
            .x_wallet_auth("") // Added by WalletAuth middleware.
            .body(message_body)
            .send()
            .await
            .map_err(|e| CdpError::SigningError(format!("Failed to sign message: {}", e)))?;

        let result = response.into_inner();

        // Parse the signature hex string to bytes
        let signature_bytes = hex::decode(
            result
                .signature
                .strip_prefix("0x")
                .unwrap_or(&result.signature),
        )
        .map_err(|e| CdpError::SigningError(format!("Invalid signature hex: {}", e)))?;

        Ok(signature_bytes)
    }

    async fn sign_evm_transaction(&self, message: &[u8]) -> Result<Vec<u8>, CdpError> {
        if !self.is_evm_address() {
            return Err(CdpError::ConfigError(
                "Account address is not an EVM address (must start with 0x)".to_string(),
            ));
        }
        let address = self.get_account_address();

        // Convert transaction bytes to hex string for CDP API
        let hex_encoded = hex::encode(message);

        let tx_body =
            types::SignEvmTransactionBody::builder().transaction(format!("0x{}", hex_encoded));

        let response = self
            .client
            .sign_evm_transaction()
            .address(address)
            .x_wallet_auth("")
            .body(tx_body)
            .send()
            .await
            .map_err(|e| CdpError::SigningError(format!("Failed to sign transaction: {}", e)))?;

        let result = response.into_inner();

        // Parse the signed transaction hex string to bytes
        let signed_tx_bytes = hex::decode(
            result
                .signed_transaction
                .strip_prefix("0x")
                .unwrap_or(&result.signed_transaction),
        )
        .map_err(|e| CdpError::SigningError(format!("Invalid signed transaction hex: {}", e)))?;

        Ok(signed_tx_bytes)
    }

    async fn sign_solana_message(&self, message: &[u8]) -> Result<Vec<u8>, CdpError> {
        if !self.is_solana_address() {
            return Err(CdpError::ConfigError(
                "Account address is not a Solana address (must not start with 0x)".to_string(),
            ));
        }
        let address = self.get_account_address();
        let encoded_message = str::from_utf8(message)
            .map_err(|e| CdpError::SerializationError(format!("Invalid UTF-8 message: {}", e)))?
            .to_string();

        let message_body = types::SignSolanaMessageBody::builder().message(encoded_message);

        let response = self
            .client
            .sign_solana_message()
            .address(address)
            .x_wallet_auth("") // Added by WalletAuth middleware.
            .body(message_body)
            .send()
            .await
            .map_err(|e| CdpError::SigningError(format!("Failed to sign Solana message: {}", e)))?;

        let result = response.into_inner();

        // Parse the signature base58 string to bytes
        let signature_bytes = bs58::decode(result.signature).into_vec().map_err(|e| {
            CdpError::SigningError(format!("Invalid Solana signature base58: {}", e))
        })?;

        Ok(signature_bytes)
    }

    async fn sign_solana_transaction(&self, transaction: String) -> Result<Vec<u8>, CdpError> {
        if !self.is_solana_address() {
            return Err(CdpError::ConfigError(
                "Account address is not a Solana address (must not start with 0x)".to_string(),
            ));
        }
        let address = self.get_account_address();

        let message_body = types::SignSolanaTransactionBody::builder().transaction(transaction);

        let response = self
            .client
            .sign_solana_transaction()
            .address(address)
            .x_wallet_auth("") // Added by WalletAuth middleware.
            .body(message_body)
            .send()
            .await
            .map_err(|e| CdpError::SigningError(format!("Failed to sign Solana transaction: {}", e)))?;

        let result = response.into_inner();

        // Parse the signed transaction base64 string to bytes
        let signature_bytes = general_purpose::STANDARD
            .decode(result.signed_transaction)
            .map_err(|e| {
                CdpError::SigningError(format!("Invalid Solana signed transaction base64: {}", e))
            })?;

        Ok(signature_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SecretString;

    fn create_test_config_evm() -> CdpSignerConfig {
        CdpSignerConfig {
            api_key_id: "test-api-key-id".to_string(),
            api_key_secret: SecretString::new("test-api-key-secret"),
            wallet_secret: SecretString::new("test-wallet-secret"),
            account_address: "0x742d35Cc6634C0532925a3b844Bc454e4438f44f".to_string(),
        }
    }

    fn create_test_config_solana() -> CdpSignerConfig {
        CdpSignerConfig {
            api_key_id: "test-api-key-id".to_string(),
            api_key_secret: SecretString::new("test-api-key-secret"),
            wallet_secret: SecretString::new("test-wallet-secret"),
            account_address: "6s7RsvzcdXFJi1tXeDoGfSKZFzN3juVt9fTar6WEhEm2".to_string(),
        }
    }

    #[test]
    fn test_new_cdp_service_valid_config() {
        let config = create_test_config_evm();
        let result = CdpService::new(config);

        // Service creation should succeed with valid config
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_account_address() {
        let config = create_test_config_evm();
        let service = CdpService::new(config).unwrap();

        let address = service.get_account_address();
        assert_eq!(address, "0x742d35Cc6634C0532925a3b844Bc454e4438f44f");
    }

    #[test]
    fn test_is_evm_address() {
        let config = create_test_config_evm();
        let service = CdpService::new(config).unwrap();
        assert!(service.is_evm_address());
        assert!(!service.is_solana_address());
    }

    #[test]
    fn test_is_solana_address() {
        let config = create_test_config_solana();
        let service = CdpService::new(config).unwrap();
        assert!(service.is_solana_address());
        assert!(!service.is_evm_address());
    }

    #[tokio::test]
    async fn test_address_evm_success() {
        let config = create_test_config_evm();
        let service = CdpService::new(config).unwrap();
        let result = service.account_address().await;

        assert!(result.is_ok());
        match result.unwrap() {
            Address::Evm(addr) => {
                // Verify the address bytes match expected values
                let expected = [
                    0x74, 0x2d, 0x35, 0xcc, 0x66, 0x34, 0xC0, 0x53, 0x29, 0x25, 0xa3, 0xb8, 0x44,
                    0xbc, 0x45, 0x4e, 0x44, 0x38, 0xf4, 0x4f,
                ];
                assert_eq!(addr, expected);
            }
            _ => panic!("Expected EVM address"),
        }
    }

    #[tokio::test]
    async fn test_address_solana_success() {
        let config = create_test_config_solana();
        let service = CdpService::new(config).unwrap();
        let result = service.account_address().await;

        assert!(result.is_ok());
        match result.unwrap() {
            Address::Solana(addr) => {
                assert_eq!(addr, "6s7RsvzcdXFJi1tXeDoGfSKZFzN3juVt9fTar6WEhEm2");
            }
            _ => panic!("Expected Solana address"),
        }
    }

    #[test]
    fn test_address_from_string_valid_evm_address() {
        let config = create_test_config_evm();
        let service = CdpService::new(config).unwrap();

        let test_address = "0x742d35Cc6634C0532925a3b844Bc454e4438f44f";
        let result = service.address_from_string(test_address);

        assert!(result.is_ok());
        match result.unwrap() {
            Address::Evm(addr) => {
                let expected = [
                    0x74, 0x2d, 0x35, 0xcc, 0x66, 0x34, 0xC0, 0x53, 0x29, 0x25, 0xa3, 0xb8, 0x44,
                    0xbc, 0x45, 0x4e, 0x44, 0x38, 0xf4, 0x4f,
                ];
                assert_eq!(addr, expected);
            }
            _ => panic!("Expected EVM address"),
        }
    }

    #[test]
    fn test_address_from_string_valid_solana_address() {
        let config = create_test_config_solana();
        let service = CdpService::new(config).unwrap();

        let test_address = "6s7RsvzcdXFJi1tXeDoGfSKZFzN3juVt9fTar6WEhEm2";
        let result = service.address_from_string(test_address);

        assert!(result.is_ok());
        match result.unwrap() {
            Address::Solana(addr) => {
                assert_eq!(addr, "6s7RsvzcdXFJi1tXeDoGfSKZFzN3juVt9fTar6WEhEm2");
            }
            _ => panic!("Expected Solana address"),
        }
    }

    #[test]
    fn test_address_from_string_without_0x_prefix() {
        let config = create_test_config_evm();
        let service = CdpService::new(config).unwrap();

        let test_address = "742d35Cc6634C0532925a3b844Bc454e4438f44f";
        let result = service.address_from_string(test_address);

        // Without 0x prefix, it should be treated as Solana address
        assert!(result.is_ok());
        match result.unwrap() {
            Address::Solana(addr) => {
                assert_eq!(addr, "742d35Cc6634C0532925a3b844Bc454e4438f44f");
            }
            _ => panic!("Expected Solana address"),
        }
    }

    #[test]
    fn test_address_from_string_invalid_hex() {
        let config = create_test_config_evm();
        let service = CdpService::new(config).unwrap();

        let test_address = "0xnot_valid_hex";
        let result = service.address_from_string(test_address);

        assert!(result.is_err());
        match result {
            Err(CdpError::ConfigError(msg)) => {
                assert!(msg.contains("Invalid hex address"));
            }
            _ => panic!("Expected ConfigError for invalid hex"),
        }
    }

    #[test]
    fn test_address_from_string_wrong_length() {
        let config = create_test_config_evm();
        let service = CdpService::new(config).unwrap();

        let test_address = "0x742d35Cc"; // Too short
        let result = service.address_from_string(test_address);

        assert!(result.is_err());
        match result {
            Err(CdpError::ConfigError(msg)) => {
                assert!(msg.contains("EVM address should be 20 bytes"));
            }
            _ => panic!("Expected ConfigError for wrong length"),
        }
    }

    #[test]
    fn test_cdp_error_display() {
        let errors = [
            CdpError::HttpError("HTTP error".to_string()),
            CdpError::AuthenticationFailed("Auth failed".to_string()),
            CdpError::ConfigError("Config error".to_string()),
            CdpError::SigningError("Signing error".to_string()),
            CdpError::SerializationError("Serialization error".to_string()),
            CdpError::SignatureError("Signature error".to_string()),
            CdpError::OtherError("Other error".to_string()),
        ];

        for error in errors {
            let error_str = error.to_string();
            assert!(!error_str.is_empty());
        }
    }
}
