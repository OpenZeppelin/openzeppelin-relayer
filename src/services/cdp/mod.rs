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
    use mockito;
    use serde_json::json;

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

    // Helper function to create a test client with middleware
    fn create_test_client() -> reqwest_middleware::ClientWithMiddleware {
        let inner = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        reqwest_middleware::ClientBuilder::new(inner).build()
    }

    // Setup mock for EVM message signing
    async fn setup_mock_sign_evm_message(mock_server: &mut mockito::ServerGuard) -> mockito::Mock {
        mock_server
            .mock("POST", mockito::Matcher::Regex(r".*/v2/evm/accounts/.*/sign/message".to_string()))
            .match_header("Content-Type", "application/json")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_string(&json!({
                "signature": "0x3045022100abcdef1234567890abcdef1234567890abcdef1234567890abcdef123456789002201234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
            })).unwrap())
            .expect(1)
            .create_async()
            .await
    }

    // Setup mock for EVM transaction signing
    async fn setup_mock_sign_evm_transaction(
        mock_server: &mut mockito::ServerGuard,
    ) -> mockito::Mock {
        mock_server
            .mock("POST", mockito::Matcher::Regex(r".*/v2/evm/accounts/.*/sign/transaction".to_string()))
            .match_header("Content-Type", "application/json")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_string(&json!({
                "signedTransaction": "0x02f87001020304050607080910111213141516171819202122232425262728293031"
            })).unwrap())
            .expect(1)
            .create_async()
            .await
    }

    // Setup mock for Solana message signing
    async fn setup_mock_sign_solana_message(
        mock_server: &mut mockito::ServerGuard,
    ) -> mockito::Mock {
        mock_server
            .mock("POST", mockito::Matcher::Regex(r".*/v2/solana/accounts/.*/sign/message".to_string()))
            .match_header("Content-Type", "application/json")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_string(&json!({
                "signature": "5VERuXP42jC4Uxo1Rc3eLQgFaQGYdM9ZJvqK3JmZ6vxGz4s8FJ7KHkQpE3cN8RuQ2mW6tX9Y5K2P1VcZqL8TfABC3X"
            })).unwrap())
            .expect(1)
            .create_async()
            .await
    }

    // Setup mock for Solana transaction signing
    async fn setup_mock_sign_solana_transaction(
        mock_server: &mut mockito::ServerGuard,
    ) -> mockito::Mock {
        mock_server
            .mock(
                "POST",
                mockito::Matcher::Regex(r".*/v2/solana/accounts/.*/sign/transaction".to_string()),
            )
            .match_header("Content-Type", "application/json")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::to_string(&json!({
                    "signedTransaction": "SGVsbG8gV29ybGQh"  // Base64 encoded test data
                }))
                .unwrap(),
            )
            .expect(1)
            .create_async()
            .await
    }

    // Setup mock for error responses - 400 Bad Request
    async fn setup_mock_error_400_malformed_transaction(
        mock_server: &mut mockito::ServerGuard,
        path_pattern: &str,
    ) -> mockito::Mock {
        mock_server
            .mock("POST", mockito::Matcher::Regex(path_pattern.to_string()))
            .match_header("Content-Type", "application/json")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::to_string(&json!({
                    "errorType": "malformed_transaction",
                    "errorMessage": "Malformed unsigned transaction."
                }))
                .unwrap(),
            )
            .expect(1)
            .create_async()
            .await
    }

    // Setup mock for error responses - 401 Unauthorized
    async fn setup_mock_error_401_unauthorized(
        mock_server: &mut mockito::ServerGuard,
        path_pattern: &str,
    ) -> mockito::Mock {
        mock_server
            .mock("POST", mockito::Matcher::Regex(path_pattern.to_string()))
            .match_header("Content-Type", "application/json")
            .with_status(401)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::to_string(&json!({
                    "errorType": "unauthorized",
                    "errorMessage": "Invalid API credentials."
                }))
                .unwrap(),
            )
            .expect(1)
            .create_async()
            .await
    }

    // Setup mock for error responses - 500 Internal Server Error
    async fn setup_mock_error_500_internal_error(
        mock_server: &mut mockito::ServerGuard,
        path_pattern: &str,
    ) -> mockito::Mock {
        mock_server
            .mock("POST", mockito::Matcher::Regex(path_pattern.to_string()))
            .match_header("Content-Type", "application/json")
            .with_status(500)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::to_string(&json!({
                    "errorType": "internal_error",
                    "errorMessage": "Internal server error occurred."
                }))
                .unwrap(),
            )
            .expect(1)
            .create_async()
            .await
    }

    // Setup mock for error responses - 422 Unprocessable Entity
    async fn setup_mock_error_422_invalid_signature(
        mock_server: &mut mockito::ServerGuard,
        path_pattern: &str,
    ) -> mockito::Mock {
        mock_server
            .mock("POST", mockito::Matcher::Regex(path_pattern.to_string()))
            .match_header("Content-Type", "application/json")
            .with_status(422)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::to_string(&json!({
                    "errorType": "invalid_signature_request",
                    "errorMessage": "Unable to process signature request."
                }))
                .unwrap(),
            )
            .expect(1)
            .create_async()
            .await
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

    #[tokio::test]
    async fn test_sign_evm_message_success() {
        let mut mock_server = mockito::Server::new_async().await;
        let _mock = setup_mock_sign_evm_message(&mut mock_server).await;

        let config = create_test_config_evm();
        let client = Client::new_with_client(&mock_server.url(), create_test_client());

        let service = CdpService { config, client };

        let message = "Hello World!".to_string();
        let result = service.sign_evm_message(message).await;

        match result {
            Ok(signature) => {
                assert!(!signature.is_empty());
            }
            Err(e) => {
                panic!("Expected success but got error: {:?}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_sign_evm_message_wrong_address_type() {
        let config = create_test_config_solana(); // Solana address for EVM signing
        let client = Client::new_with_client("http://test", create_test_client());
        let service = CdpService { config, client };

        let message = "Hello World!".to_string();
        let result = service.sign_evm_message(message).await;

        assert!(result.is_err());
        match result {
            Err(CdpError::ConfigError(msg)) => {
                assert!(msg.contains("Account address is not an EVM address"));
            }
            _ => panic!("Expected ConfigError for wrong address type"),
        }
    }

    #[tokio::test]
    async fn test_sign_evm_transaction_success() {
        let mut mock_server = mockito::Server::new_async().await;
        let _mock = setup_mock_sign_evm_transaction(&mut mock_server).await;

        let config = create_test_config_evm();
        let client = Client::new_with_client(&mock_server.url(), create_test_client());

        let service = CdpService { config, client };

        let transaction_bytes = b"test transaction data";
        let result = service.sign_evm_transaction(transaction_bytes).await;

        match result {
            Ok(signed_tx) => {
                assert!(!signed_tx.is_empty());
            }
            Err(e) => {
                panic!("Expected success but got error: {:?}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_sign_evm_transaction_wrong_address_type() {
        let config = create_test_config_solana(); // Solana address for EVM signing
        let client = Client::new_with_client("http://test", create_test_client());
        let service = CdpService { config, client };

        let transaction_bytes = b"test transaction data";
        let result = service.sign_evm_transaction(transaction_bytes).await;

        assert!(result.is_err());
        match result {
            Err(CdpError::ConfigError(msg)) => {
                assert!(msg.contains("Account address is not an EVM address"));
            }
            _ => panic!("Expected ConfigError for wrong address type"),
        }
    }

    #[tokio::test]
    async fn test_sign_solana_message_success() {
        let mut mock_server = mockito::Server::new_async().await;
        let _mock = setup_mock_sign_solana_message(&mut mock_server).await;

        let config = create_test_config_solana();
        let client = Client::new_with_client(&mock_server.url(), create_test_client());

        let service = CdpService { config, client };

        let message_bytes = b"Hello Solana!";
        let result = service.sign_solana_message(message_bytes).await;

        assert!(result.is_ok());
        let signature = result.unwrap();
        assert!(!signature.is_empty());
    }

    #[tokio::test]
    async fn test_sign_solana_message_wrong_address_type() {
        let config = create_test_config_evm(); // EVM address for Solana signing
        let client = Client::new_with_client("http://test", create_test_client());
        let service = CdpService { config, client };

        let message_bytes = b"Hello Solana!";
        let result = service.sign_solana_message(message_bytes).await;

        assert!(result.is_err());
        match result {
            Err(CdpError::ConfigError(msg)) => {
                assert!(msg.contains("Account address is not a Solana address"));
            }
            _ => panic!("Expected ConfigError for wrong address type"),
        }
    }

    #[tokio::test]
    async fn test_sign_solana_transaction_success() {
        let mut mock_server = mockito::Server::new_async().await;
        let _mock = setup_mock_sign_solana_transaction(&mut mock_server).await;

        let config = create_test_config_solana();
        let client = Client::new_with_client(&mock_server.url(), create_test_client());

        let service = CdpService { config, client };

        let transaction = "test-transaction-string".to_string();
        let result = service.sign_solana_transaction(transaction).await;

        match result {
            Ok(signed_tx) => {
                assert!(!signed_tx.is_empty());
            }
            Err(e) => {
                panic!("Expected success but got error: {:?}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_sign_solana_transaction_wrong_address_type() {
        let config = create_test_config_evm(); // EVM address for Solana signing
        let client = Client::new_with_client("http://test", create_test_client());
        let service = CdpService { config, client };

        let transaction = "test-transaction-string".to_string();
        let result = service.sign_solana_transaction(transaction).await;

        assert!(result.is_err());
        match result {
            Err(CdpError::ConfigError(msg)) => {
                assert!(msg.contains("Account address is not a Solana address"));
            }
            _ => panic!("Expected ConfigError for wrong address type"),
        }
    }

    // Error handling tests
    #[tokio::test]
    async fn test_sign_evm_message_error_400_malformed_transaction() {
        let mut mock_server = mockito::Server::new_async().await;
        let _mock = setup_mock_error_400_malformed_transaction(
            &mut mock_server,
            r".*/v2/evm/accounts/.*/sign/message",
        )
        .await;

        let config = create_test_config_evm();
        let client = Client::new_with_client(&mock_server.url(), create_test_client());
        let service = CdpService { config, client };

        let message = "Hello World!".to_string();
        let result = service.sign_evm_message(message).await;

        assert!(result.is_err());
        match result {
            Err(CdpError::SigningError(msg)) => {
                assert!(msg.contains("Failed to sign message"));
            }
            _ => panic!("Expected SigningError for malformed transaction"),
        }
    }

    #[tokio::test]
    async fn test_sign_evm_message_error_401_unauthorized() {
        let mut mock_server = mockito::Server::new_async().await;
        let _mock = setup_mock_error_401_unauthorized(
            &mut mock_server,
            r".*/v2/evm/accounts/.*/sign/message",
        )
        .await;

        let config = create_test_config_evm();
        let client = Client::new_with_client(&mock_server.url(), create_test_client());
        let service = CdpService { config, client };

        let message = "Hello World!".to_string();
        let result = service.sign_evm_message(message).await;

        assert!(result.is_err());
        match result {
            Err(CdpError::SigningError(msg)) => {
                assert!(msg.contains("Failed to sign message"));
            }
            _ => panic!("Expected SigningError for unauthorized"),
        }
    }

    #[tokio::test]
    async fn test_sign_evm_message_error_500_internal_error() {
        let mut mock_server = mockito::Server::new_async().await;
        let _mock = setup_mock_error_500_internal_error(
            &mut mock_server,
            r".*/v2/evm/accounts/.*/sign/message",
        )
        .await;

        let config = create_test_config_evm();
        let client = Client::new_with_client(&mock_server.url(), create_test_client());
        let service = CdpService { config, client };

        let message = "Hello World!".to_string();
        let result = service.sign_evm_message(message).await;

        assert!(result.is_err());
        match result {
            Err(CdpError::SigningError(msg)) => {
                assert!(msg.contains("Failed to sign message"));
            }
            _ => panic!("Expected SigningError for internal error"),
        }
    }

    #[tokio::test]
    async fn test_sign_evm_transaction_error_400_malformed_transaction() {
        let mut mock_server = mockito::Server::new_async().await;
        let _mock = setup_mock_error_400_malformed_transaction(
            &mut mock_server,
            r".*/v2/evm/accounts/.*/sign/transaction",
        )
        .await;

        let config = create_test_config_evm();
        let client = Client::new_with_client(&mock_server.url(), create_test_client());
        let service = CdpService { config, client };

        let transaction_bytes = b"invalid transaction data";
        let result = service.sign_evm_transaction(transaction_bytes).await;

        assert!(result.is_err());
        match result {
            Err(CdpError::SigningError(msg)) => {
                assert!(msg.contains("Failed to sign transaction"));
            }
            _ => panic!("Expected SigningError for malformed transaction"),
        }
    }

    #[tokio::test]
    async fn test_sign_evm_transaction_error_422_invalid_signature() {
        let mut mock_server = mockito::Server::new_async().await;
        let _mock = setup_mock_error_422_invalid_signature(
            &mut mock_server,
            r".*/v2/evm/accounts/.*/sign/transaction",
        )
        .await;

        let config = create_test_config_evm();
        let client = Client::new_with_client(&mock_server.url(), create_test_client());
        let service = CdpService { config, client };

        let transaction_bytes = b"test transaction data";
        let result = service.sign_evm_transaction(transaction_bytes).await;

        assert!(result.is_err());
        match result {
            Err(CdpError::SigningError(msg)) => {
                assert!(msg.contains("Failed to sign transaction"));
            }
            _ => panic!("Expected SigningError for invalid signature request"),
        }
    }

    #[tokio::test]
    async fn test_sign_solana_message_error_400_malformed_transaction() {
        let mut mock_server = mockito::Server::new_async().await;
        let _mock = setup_mock_error_400_malformed_transaction(
            &mut mock_server,
            r".*/v2/solana/accounts/.*/sign/message",
        )
        .await;

        let config = create_test_config_solana();
        let client = Client::new_with_client(&mock_server.url(), create_test_client());
        let service = CdpService { config, client };

        let message_bytes = b"Hello Solana!";
        let result = service.sign_solana_message(message_bytes).await;

        assert!(result.is_err());
        match result {
            Err(CdpError::SigningError(msg)) => {
                assert!(msg.contains("Failed to sign Solana message"));
            }
            _ => panic!("Expected SigningError for malformed transaction"),
        }
    }

    #[tokio::test]
    async fn test_sign_solana_message_error_401_unauthorized() {
        let mut mock_server = mockito::Server::new_async().await;
        let _mock = setup_mock_error_401_unauthorized(
            &mut mock_server,
            r".*/v2/solana/accounts/.*/sign/message",
        )
        .await;

        let config = create_test_config_solana();
        let client = Client::new_with_client(&mock_server.url(), create_test_client());
        let service = CdpService { config, client };

        let message_bytes = b"Hello Solana!";
        let result = service.sign_solana_message(message_bytes).await;

        assert!(result.is_err());
        match result {
            Err(CdpError::SigningError(msg)) => {
                assert!(msg.contains("Failed to sign Solana message"));
            }
            _ => panic!("Expected SigningError for unauthorized"),
        }
    }

    #[tokio::test]
    async fn test_sign_solana_transaction_error_400_malformed_transaction() {
        let mut mock_server = mockito::Server::new_async().await;
        let _mock = setup_mock_error_400_malformed_transaction(
            &mut mock_server,
            r".*/v2/solana/accounts/.*/sign/transaction",
        )
        .await;

        let config = create_test_config_solana();
        let client = Client::new_with_client(&mock_server.url(), create_test_client());
        let service = CdpService { config, client };

        let transaction = "invalid-transaction-string".to_string();
        let result = service.sign_solana_transaction(transaction).await;

        assert!(result.is_err());
        match result {
            Err(CdpError::SigningError(msg)) => {
                assert!(msg.contains("Failed to sign Solana transaction"));
            }
            _ => panic!("Expected SigningError for malformed transaction"),
        }
    }

    #[tokio::test]
    async fn test_sign_solana_transaction_error_500_internal_error() {
        let mut mock_server = mockito::Server::new_async().await;
        let _mock = setup_mock_error_500_internal_error(
            &mut mock_server,
            r".*/v2/solana/accounts/.*/sign/transaction",
        )
        .await;

        let config = create_test_config_solana();
        let client = Client::new_with_client(&mock_server.url(), create_test_client());
        let service = CdpService { config, client };

        let transaction = "test-transaction-string".to_string();
        let result = service.sign_solana_transaction(transaction).await;

        assert!(result.is_err());
        match result {
            Err(CdpError::SigningError(msg)) => {
                assert!(msg.contains("Failed to sign Solana transaction"));
            }
            _ => panic!("Expected SigningError for internal error"),
        }
    }
}
