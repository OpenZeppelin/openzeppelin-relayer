//! # Turnkey Service Module
//!
//! This module provides integration with Turnkey API for secure wallet management
//! and cryptographic operations.
//!
//! ## Features
//!
//! - API key-based authentication
//! - Digital signature generation
//! - Message signing via Turnkey API
//! - Secure transaction signing for blockchain operations
//!
//! ## Architecture
//!
//! ```text
//! TurnkeyService (implements TurnkeyServiceTrait)
//!   ├── Authentication (API key-based)
//!   ├── Digital Stamping
//!   ├── Transaction Signing
//!   └── Raw Payload Signing
//! ```
use std::str::FromStr;

use alloy::primitives::keccak256;
use async_trait::async_trait;
use chrono;
use log::{debug, info};
use p256::{
    ecdsa::{signature::Signer, Signature as P256Signature, SigningKey},
    FieldBytes,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use solana_sdk::{pubkey::Pubkey, signature::Signature, transaction::Transaction};
use thiserror::Error;

use crate::models::{Address, SecretString, TurnkeySignerConfig};
use crate::utils::base64_url_encode;

#[derive(Error, Debug, Serialize)]
pub enum TurnkeyError {
    #[error("HTTP error: {0}")]
    HttpError(String),

    #[error("API method error: {0:?}")]
    MethodError(TurnkeyResponseError),

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

    #[error("Invalid pubkey: {0}")]
    PubkeyError(#[from] solana_sdk::pubkey::PubkeyError),

    #[error("Other error: {0}")]
    OtherError(String),
}

/// Error response from Turnkey API
#[derive(Debug, Deserialize, Serialize)]
pub struct TurnkeyResponseError {
    pub error: TurnkeyErrorDetails,
}

/// Error details from Turnkey API
#[derive(Debug, Deserialize, Serialize)]
pub struct TurnkeyErrorDetails {
    pub code: i32,
    pub message: String,
}

/// Result type for Turnkey operations
pub type TurnkeyResult<T> = Result<T, TurnkeyError>;

/// Digital stamp for API authentication
#[derive(Serialize)]
struct ApiStamp {
    pub public_key: String,
    pub signature: String,
    pub scheme: String,
}

/// Request to sign raw payload
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SignRawPayloadRequest {
    #[serde(rename = "type")]
    activity_type: String,
    timestamp_ms: String,
    organization_id: String,
    parameters: SignRawPayloadIntentV2Parameters,
}

/// Parameters for signing transaction payload
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SignEvmTransactionRequest {
    #[serde(rename = "type")]
    activity_type: String,
    timestamp_ms: String,
    organization_id: String,
    parameters: SignEvmTransactionV2Parameters,
}

/// Parameters for signing raw payload
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SignRawPayloadIntentV2Parameters {
    sign_with: String,
    payload: String,
    encoding: String,
    hash_function: String,
}

/// Parameters for signing raw payload
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SignEvmTransactionV2Parameters {
    sign_with: String,
    #[serde(rename = "type")]
    sign_type: String,
    unsigned_transaction: String,
}

/// Response from activity API
#[derive(Deserialize, Serialize)]
struct ActivityResponse {
    activity: Activity,
}

/// Activity details
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Activity {
    id: Option<String>,
    status: Option<String>,
    result: Option<ActivityResult>,
}

/// Activity result
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ActivityResult {
    sign_raw_payload_result: Option<SignRawPayloadResult>,
    sign_transaction_result: Option<SignTransactionResult>,
}

/// Sign raw payload result
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SignRawPayloadResult {
    r: String,
    s: String,
    v: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SignTransactionResult {
    signed_transaction: String,
}

#[cfg(test)]
use mockall::automock;

#[async_trait]
#[cfg_attr(test, automock)]
pub trait TurnkeyServiceTrait: Send + Sync {
    /// Returns the Solana address derived from the configured public key
    fn address_solana(&self) -> Result<Address, TurnkeyError>;

    /// Returns the EVM address derived from the configured public key
    fn address_evm(&self) -> Result<Address, TurnkeyError>;

    /// Signs a message using the Solana signing scheme
    async fn sign_solana(&self, message: &[u8]) -> Result<Vec<u8>, TurnkeyError>;

    /// Signs a message using the EVM signing scheme
    async fn sign_evm(&self, message: &[u8]) -> Result<Vec<u8>, TurnkeyError>;

    /// Signs an EVM transaction using the Turnkey API
    async fn sign_evm_transaction(&self, message: &[u8]) -> Result<Vec<u8>, TurnkeyError>;

    /// Signs a Solana transaction and returns both the transaction and signature
    async fn sign_solana_transaction(
        &self,
        transaction: &mut Transaction,
    ) -> TurnkeyResult<(Transaction, Signature)>;
}

#[derive(Clone)]
pub struct TurnkeyService {
    pub api_public_key: String,
    pub api_private_key: SecretString,
    pub organization_id: String,
    pub private_key_id: String,
    pub public_key: String,
    pub base_url: String,
    client: Client,
}

impl TurnkeyService {
    pub fn new(config: TurnkeySignerConfig) -> Result<Self, TurnkeyError> {
        Ok(Self {
            api_public_key: config.api_public_key.clone(),
            api_private_key: config.api_private_key,
            organization_id: config.organization_id.clone(),
            private_key_id: config.private_key_id.clone(),
            public_key: config.public_key.clone(),
            base_url: String::from("https://api.turnkey.com"),
            client: Client::new(),
        })
    }

    /// Converts the public key to an Solana address
    pub fn address_solana(&self) -> Result<Address, TurnkeyError> {
        if self.public_key.is_empty() {
            return Err(TurnkeyError::ConfigError("Public key is empty".to_string()));
        }

        let raw_pubkey = hex::decode(&self.public_key)
            .map_err(|e| TurnkeyError::ConfigError(format!("Invalid public key hex: {}", e)))?;

        let pubkey_bs58 = bs58::encode(&raw_pubkey).into_string();

        Ok(Address::Solana(pubkey_bs58))
    }

    /// Converts the public key to an EVM address
    pub fn address_evm(&self) -> Result<Address, TurnkeyError> {
        let public_key = hex::decode(&self.public_key)
            .map_err(|e| TurnkeyError::ConfigError(format!("Invalid public key hex: {}", e)))?;

        // Remove the first byte (0x04 prefix)
        let pub_key_no_prefix = &public_key[1..];

        let hash = keccak256(pub_key_no_prefix);

        // Ethereum addresses are the last 20 bytes of the Keccak-256 hash.
        // Since the hash is 32 bytes, the address is bytes 12..32.
        let address_bytes = &hash[12..];

        if address_bytes.len() != 20 {
            return Err(TurnkeyError::ConfigError(format!(
                "EVM address should be 20 bytes, got {} bytes",
                address_bytes.len()
            )));
        }

        let mut array = [0u8; 20];
        array.copy_from_slice(address_bytes);

        Ok(Address::Evm(array))
    }

    /// Creates a digital stamp for API authentication
    fn stamp(&self, message: &str) -> TurnkeyResult<String> {
        let private_api_key_bytes =
            hex::decode(self.api_private_key.to_str().as_str()).map_err(|e| {
                TurnkeyError::ConfigError(format!("Failed to decode private key: {}", e))
            })?;

        let signing_key: SigningKey =
            SigningKey::from_bytes(FieldBytes::from_slice(&private_api_key_bytes))
                .map_err(|e| TurnkeyError::SigningError(format!("Turnkey stamp error: {}", e)))?;

        let signature: P256Signature = signing_key.sign(message.as_bytes());

        let stamp = ApiStamp {
            public_key: self.api_public_key.clone(),
            signature: hex::encode(signature.to_der()),
            scheme: "SIGNATURE_SCHEME_TK_API_P256".into(),
        };

        let json_stamp = serde_json::to_string(&stamp).map_err(|e| {
            TurnkeyError::SerializationError(format!("Serialization stamp error: {}", e))
        })?;
        let encoded_stamp = base64_url_encode(json_stamp.as_bytes());

        Ok(encoded_stamp)
    }

    /// Helper method to make Turnkey API requests
    async fn make_turnkey_request<T, R>(&self, endpoint: &str, request_body: &T) -> TurnkeyResult<R>
    where
        T: Serialize,
        R: for<'de> Deserialize<'de> + 'static,
    {
        // Serialize the request body
        let body = serde_json::to_string(request_body).map_err(|e| {
            TurnkeyError::SerializationError(format!("Request serialization error: {}", e))
        })?;

        // Create the authentication stamp
        let x_stamp = self.stamp(&body)?;

        debug!("Sending request to Turnkey API: {}", endpoint);
        let response = self
            .client
            .post(format!("{}/public/v1/submit/{}", self.base_url, endpoint))
            .header("Content-Type", "application/json")
            .header("X-Stamp", x_stamp)
            .body(body)
            .send()
            .await;

        self.process_response::<R>(response).await
    }

    /// Helper method to sign raw payloads with configurable hash function and v inclusion
    async fn sign_raw_payload(
        &self,
        payload: &[u8],
        hash_function: &str,
        include_v: bool,
    ) -> TurnkeyResult<Vec<u8>> {
        let encoded_payload = hex::encode(payload);

        let sign_raw_payload_body = SignRawPayloadRequest {
            activity_type: "ACTIVITY_TYPE_SIGN_RAW_PAYLOAD_V2".to_string(),
            timestamp_ms: chrono::Utc::now().timestamp_millis().to_string(),
            organization_id: self.organization_id.clone(),
            parameters: SignRawPayloadIntentV2Parameters {
                sign_with: self.private_key_id.clone(),
                payload: encoded_payload,
                encoding: "PAYLOAD_ENCODING_HEXADECIMAL".to_string(),
                hash_function: hash_function.to_string(),
            },
        };

        let response_body = self
            .make_turnkey_request::<_, ActivityResponse>("sign_raw_payload", &sign_raw_payload_body)
            .await?;

        if let Some(result) = response_body.activity.result {
            if let Some(result) = result.sign_raw_payload_result {
                let concatenated_hex = if include_v {
                    format!("{}{}{}", result.r, result.s, result.v)
                } else {
                    format!("{}{}", result.r, result.s)
                };

                let signature_bytes = hex::decode(&concatenated_hex).map_err(|e| {
                    TurnkeyError::SigningError(format!("Turnkey signing error {}", e))
                })?;

                return Ok(signature_bytes);
            }
        }

        Err(TurnkeyError::OtherError(
            "Missing SIGN_RAW_PAYLOAD result".into(),
        ))
    }

    /// Signs raw bytes using the Turnkey API (for Solana)
    async fn sign_bytes_solana(&self, bytes: &[u8]) -> TurnkeyResult<Vec<u8>> {
        self.sign_raw_payload(bytes, "HASH_FUNCTION_NOT_APPLICABLE", false)
            .await
    }

    /// Signs raw bytes using the Turnkey API (for EVM)
    async fn sign_bytes_evm(&self, bytes: &[u8]) -> TurnkeyResult<Vec<u8>> {
        let result = self
            .sign_raw_payload(bytes, "HASH_FUNCTION_NO_OP", true)
            .await?;
        debug!("EVM signature length: {}", result.len());
        Ok(result)
    }

    /// Signs an EVM transaction using the Turnkey API
    async fn sign_evm_transaction(&self, bytes: &[u8]) -> TurnkeyResult<Vec<u8>> {
        let encoded_bytes = hex::encode(bytes);

        // Create the request body
        let sign_transaction_body = SignEvmTransactionRequest {
            activity_type: "ACTIVITY_TYPE_SIGN_TRANSACTION_V2".to_string(),
            timestamp_ms: chrono::Utc::now().timestamp_millis().to_string(),
            organization_id: self.organization_id.clone(),
            parameters: SignEvmTransactionV2Parameters {
                sign_with: self.private_key_id.clone(),
                sign_type: "TRANSACTION_TYPE_ETHEREUM".to_string(),
                unsigned_transaction: encoded_bytes,
            },
        };

        // Make the API request and get the response
        let response_body = self
            .make_turnkey_request::<_, ActivityResponse>("sign_transaction", &sign_transaction_body)
            .await?;

        // Extract the signed transaction
        response_body
            .activity
            .result
            .and_then(|result| result.sign_transaction_result)
            .map(|tx_result| hex::decode(&tx_result.signed_transaction))
            .transpose()
            .map_err(|e| {
                TurnkeyError::SigningError(format!("Failed to decode transaction: {}", e))
            })?
            .ok_or_else(|| TurnkeyError::OtherError("Missing transaction result".into()))
    }

    async fn process_response<T>(
        &self,
        response: Result<reqwest::Response, reqwest::Error>,
    ) -> TurnkeyResult<T>
    where
        T: for<'de> Deserialize<'de> + 'static,
    {
        match response {
            Ok(res) => {
                let status = res.status();
                let headers = res.headers().clone();
                let content_type = headers
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("unknown");

                if res.status().is_success() {
                    // On success, deserialize the response into the expected type T
                    res.json::<T>()
                        .await
                        .map_err(|e| TurnkeyError::HttpError(e.to_string()))
                } else {
                    // For error responses, try to get the body text first
                    match res.text().await {
                        Ok(body_text) => {
                            debug!("Error response ({}): {}", status, body_text);

                            if content_type.contains("application/json") {
                                match serde_json::from_str::<TurnkeyResponseError>(&body_text) {
                                    Ok(error) => Err(TurnkeyError::MethodError(error)),
                                    Err(e) => {
                                        debug!("Failed to parse error response as JSON: {}", e);
                                        Err(TurnkeyError::HttpError(format!(
                                            "HTTP {} error: {}",
                                            status, body_text
                                        )))
                                    }
                                }
                            } else {
                                Err(TurnkeyError::HttpError(format!(
                                    "HTTP {} error: {}",
                                    status, body_text
                                )))
                            }
                        }
                        Err(e) => {
                            info!("Failed to read error response body: {}", e);
                            Err(TurnkeyError::HttpError(format!(
                                "HTTP {} error (failed to read body): {}",
                                status, e
                            )))
                        }
                    }
                }
            }
            Err(e) => {
                debug!("Turnkey API request error: {:?}", e);
                // On a reqwest error, convert it into a TurnkeyError::HttpError
                Err(TurnkeyError::HttpError(e.to_string()))
            }
        }
    }
}

#[async_trait]
impl TurnkeyServiceTrait for TurnkeyService {
    fn address_solana(&self) -> Result<Address, TurnkeyError> {
        self.address_solana()
    }

    fn address_evm(&self) -> Result<Address, TurnkeyError> {
        self.address_evm()
    }

    async fn sign_solana(&self, message: &[u8]) -> Result<Vec<u8>, TurnkeyError> {
        let signature_bytes = self.sign_bytes_solana(message).await?;
        Ok(signature_bytes)
    }

    async fn sign_evm(&self, message: &[u8]) -> Result<Vec<u8>, TurnkeyError> {
        let signature_bytes = self.sign_bytes_evm(message).await?;
        Ok(signature_bytes)
    }

    async fn sign_evm_transaction(&self, message: &[u8]) -> Result<Vec<u8>, TurnkeyError> {
        let signature_bytes = self.sign_evm_transaction(message).await?;
        Ok(signature_bytes)
    }

    async fn sign_solana_transaction(
        &self,
        transaction: &mut Transaction,
    ) -> TurnkeyResult<(Transaction, Signature)> {
        let serialized_message = transaction.message_data();

        let public_key = Pubkey::from_str(&self.address_solana()?.to_string())
            .map_err(|e| TurnkeyError::ConfigError(format!("Invalid pubkey: {}", e)))?;

        let signature_bytes = self.sign_bytes_solana(&serialized_message).await?;

        let signature = Signature::try_from(signature_bytes.as_slice())
            .map_err(|e| TurnkeyError::SignatureError(format!("Invalid signature: {}", e)))?;

        let index = transaction
            .message
            .account_keys
            .iter()
            .position(|key| key == &public_key);

        match index {
            Some(i) if i < transaction.signatures.len() => {
                transaction.signatures[i] = signature;
                Ok((transaction.clone(), signature))
            }
            _ => Err(TurnkeyError::OtherError(
                "Unknown signer or index out of bounds".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn create_solana_test_config() -> TurnkeySignerConfig {
        TurnkeySignerConfig {
            api_public_key: "test-api-public-key".to_string(),
            api_private_key: SecretString::new(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            ),
            organization_id: "test-org-id".to_string(),
            private_key_id: "test-private-key-id".to_string(),
            public_key: "5720be8aa9d2bb4be8e91f31d2c44c8629e42da16981c2cebabd55cafa0b76bd"
                .to_string(),
        }
    }

    fn create_evm_test_config() -> TurnkeySignerConfig {
        TurnkeySignerConfig {
            api_public_key: "test-api-public-key".to_string(),
            api_private_key: SecretString::new("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
            organization_id: "test-org-id".to_string(),
            private_key_id: "test-private-key-id".to_string(),
            public_key: "047d3bb8e0317927700cf19fed34e0627367be1390ec247dddf8c239e4b4321a49aea80090e49b206b6a3e577a4f11d721ab063482001ee10db40d6f2963233eec".to_string(),
        }
    }

    #[test]
    fn test_new_turnkey_service() {
        let config = create_evm_test_config();
        let service = TurnkeyService::new(config);

        assert!(service.is_ok());
        let service = service.unwrap();
        assert_eq!(service.api_public_key, "test-api-public-key");
        assert_eq!(service.organization_id, "test-org-id");
        assert_eq!(service.private_key_id, "test-private-key-id");
    }

    #[test]
    fn test_address_evm() {
        let config = create_evm_test_config();
        let service = TurnkeyService::new(config).unwrap();

        let address = service.address_evm();
        assert!(address.is_ok());

        let address = address.unwrap();

        assert_eq!(
            address.to_string(),
            "0xb726167dc2ef2ac582f0a3de4c08ac4abb90626a"
        );
    }

    #[test]
    fn test_address_solana() {
        let config = create_solana_test_config();
        let service = TurnkeyService::new(config).unwrap();

        let address = service.address_solana();
        assert!(address.is_ok());

        let address_str = address.unwrap().to_string();
        assert_eq!(address_str, "6s7RsvzcdXFJi1tXeDoGfSKZFzN3juVt9fTar6WEhEm2");
    }

    #[test]
    fn test_address_with_empty_pubkey() {
        let mut config = create_solana_test_config();
        config.public_key = "".to_string();
        let service = TurnkeyService::new(config).unwrap();

        let result = service.address_solana();
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(matches!(e, TurnkeyError::ConfigError(_)));
            assert_eq!(e.to_string(), "Configuration error: Public key is empty");
        }
    }

    #[test]
    fn test_address_with_invalid_pubkey() {
        let mut config = create_solana_test_config();
        config.public_key = "invalid-hex".to_string();
        let service = TurnkeyService::new(config).unwrap();

        let result = service.address_evm();
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(matches!(e, TurnkeyError::ConfigError(_)));
            assert!(e.to_string().contains("Invalid public key hex"));
        }
    }

    // Setup mock for signing raw payload
    async fn setup_mock_sign_raw_payload(mock_server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/public/v1/submit/sign_raw_payload"))
            .and(header("Content-Type", "application/json"))
            .and(header_exists("X-Stamp"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "activity": {
                    "id": "test-activity-id",
                    "status": "ACTIVITY_STATUS_COMPLETE",
                    "result": {
                        "signRawPayloadResult": {
                            "r": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                            "s": "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210",
                            "v": "1b"
                        }
                    }
                }
            })))
            .mount(mock_server)
            .await;
    }

    // Setup mock for signing EVM transaction
    async fn setup_mock_sign_evm_transaction(mock_server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/public/v1/submit/sign_transaction"))
            .and(header("Content-Type", "application/json"))
            .and(header_exists("X-Stamp"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "activity": {
                    "id": "test-activity-id",
                    "status": "ACTIVITY_STATUS_COMPLETE",
                    "result": {
                        "signTransactionResult": {
                            "signedTransaction": "02f1010203050607080910" // Example signed transaction hex
                        }
                    }
                }
            })))
            .mount(mock_server)
            .await;
    }

    // Setup mock for error response
    async fn setup_mock_error_response(mock_server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/public/v1/submit/sign_raw_payload"))
            .and(header("Content-Type", "application/json"))
            .and(header_exists("X-Stamp"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": {
                    "code": 400,
                    "message": "Invalid payload format"
                }
            })))
            .mount(mock_server)
            .await;
    }

    // Helper function to create a modified client for testing
    fn create_test_client() -> Client {
        reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn test_sign_solana() {
        let mock_server = MockServer::start().await;
        setup_mock_sign_raw_payload(&mock_server).await;

        let config = create_solana_test_config();

        let service = TurnkeyService {
            api_public_key: config.api_public_key,
            api_private_key: config.api_private_key,
            organization_id: config.organization_id,
            private_key_id: config.private_key_id,
            public_key: config.public_key,
            base_url: mock_server.uri(),
            client: create_test_client(),
        };

        let message = b"test message";
        let result = service.sign_solana(message).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sign_evm() {
        let mock_server = MockServer::start().await;
        setup_mock_sign_raw_payload(&mock_server).await;

        let config = create_evm_test_config();
        let service = TurnkeyService {
            api_public_key: config.api_public_key,
            api_private_key: config.api_private_key,
            organization_id: config.organization_id,
            private_key_id: config.private_key_id,
            public_key: config.public_key,
            base_url: mock_server.uri(),
            client: create_test_client(),
        };

        let message = b"test message";
        let result = service.sign_evm(message).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sign_evm_transaction() {
        let mock_server = MockServer::start().await;
        setup_mock_sign_evm_transaction(&mock_server).await;

        let config = create_evm_test_config();
        let service = TurnkeyService {
            api_public_key: config.api_public_key,
            api_private_key: config.api_private_key,
            organization_id: config.organization_id,
            private_key_id: config.private_key_id,
            public_key: config.public_key,
            base_url: mock_server.uri(),
            client: create_test_client(),
        };

        let message = b"test transaction";
        let result = service.sign_evm_transaction(message).await;

        assert!(result.is_ok());
        let result = result.unwrap();
        let expected = hex::decode("02f1010203050607080910").unwrap();
        assert_eq!(result, expected)
    }

    #[tokio::test]
    async fn test_error_handling() {
        let mock_server = MockServer::start().await;
        setup_mock_error_response(&mock_server).await;

        let config = create_solana_test_config();
        let service = TurnkeyService {
            api_public_key: config.api_public_key,
            api_private_key: config.api_private_key,
            organization_id: config.organization_id,
            private_key_id: config.private_key_id,
            public_key: config.public_key,
            base_url: mock_server.uri(),
            client: create_test_client(),
        };

        let message = b"test message";
        let result = service.sign_solana(message).await;
        assert!(result.is_err());
        match result {
            Err(TurnkeyError::MethodError(e)) => {
                assert!(e.error.message.contains("Invalid payload format"));
            }
            _ => panic!("Expected MethodError for Solana signing"),
        }
    }
}
