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
use std::str::FromStr;
use thiserror::Error;

use crate::models::{SecretString, TurnkeySignerConfig};
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

/// Parameters for signing raw payload
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SignRawPayloadIntentV2Parameters {
    sign_with: String,
    payload: String,
    encoding: String,
    hash_function: String,
}

/// Response from activity API
#[derive(Deserialize)]
struct ActivityResponse {
    activity: Activity,
}

/// Activity details
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Activity {
    result: Option<ActivityResult>,
}

/// Activity result
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivityResult {
    sign_raw_payload_result: Option<SignRawPayloadResult>,
}

/// Sign raw payload result
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignRawPayloadResult {
    r: String,
    s: String,
}

#[cfg(test)]
use mockall::automock;

#[async_trait]
#[cfg_attr(test, automock)]
pub trait TurnkeyServiceTrait: Send + Sync {
    async fn sign(&self, message: &[u8]) -> Result<Vec<u8>, TurnkeyError>;
    async fn sign_transaction(
        &self,
        transaction: &mut Transaction,
    ) -> TurnkeyResult<(Transaction, Signature)>;
}

#[derive(Clone)]
pub struct TurnkeyService {
    api_public_key: String,
    api_private_key: SecretString,
    organization_id: String,
    private_key_id: String,
    address: Option<Pubkey>,
    client: Client,
}

impl TurnkeyService {
    pub fn new(config: TurnkeySignerConfig) -> Result<Self, TurnkeyError> {
        let address = if !config.public_key.is_empty() {
            let raw_pubkey = hex::decode(&config.public_key)
                .map_err(|e| TurnkeyError::ConfigError(format!("Invalid public key: {}", e)))?;
            let pubkey = bs58::encode(&raw_pubkey).into_string();

            Some(
                Pubkey::from_str(&pubkey)
                    .map_err(|e| TurnkeyError::ConfigError(format!("Invalid public key: {}", e)))?,
            )
        } else {
            None
        };

        info!("Creating TurnkeyService with config: {:?}", config);

        Ok(Self {
            api_public_key: config.api_public_key.clone(),
            api_private_key: config.api_private_key,
            organization_id: config.organization_id.clone(),
            private_key_id: config.private_key_id.clone(),
            address,
            client: Client::new(),
        })
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
        let encoded_stamp = base64_url_encode(&json_stamp.as_bytes());

        Ok(encoded_stamp)
    }

    /// Signs raw bytes using the Turnkey API
    async fn sign_bytes(&self, bytes: &[u8]) -> TurnkeyResult<Vec<u8>> {
        let encoded_bytes = hex::encode(bytes);

        let sign_raw_payload_body = SignRawPayloadRequest {
            activity_type: "ACTIVITY_TYPE_SIGN_RAW_PAYLOAD_V2".to_string(),
            timestamp_ms: chrono::Utc::now().timestamp_millis().to_string(),
            organization_id: self.organization_id.clone(),
            parameters: SignRawPayloadIntentV2Parameters {
                sign_with: self.private_key_id.clone(),
                payload: encoded_bytes,
                encoding: "PAYLOAD_ENCODING_HEXADECIMAL".to_string(),
                hash_function: "HASH_FUNCTION_NOT_APPLICABLE".to_string(),
            },
        };

        let body = serde_json::to_string(&sign_raw_payload_body).map_err(|e| {
            TurnkeyError::SerializationError(format!("Signing serialization error: {}", e))
        })?;
        let x_stamp = self.stamp(&body)?;

        debug!("Sending sign request to Turnkey API");
        let response = self
            .client
            .post("https://api.turnkey.com/public/v1/submit/sign_raw_payload")
            .header("Content-Type", "application/json")
            .header("X-Stamp", x_stamp)
            .body(body)
            .send()
            .await;

        let response_body = self.process_response::<ActivityResponse>(response).await?;

        if let Some(result) = response_body.activity.result {
            if let Some(result) = result.sign_raw_payload_result {
                let concatenated_hex = format!("{}{}", result.r, result.s);
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

    /// Process API responses
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
    async fn sign(&self, message: &[u8]) -> Result<Vec<u8>, TurnkeyError> {
        let signature_bytes = self.sign_bytes(message).await?;
        Ok(signature_bytes)
    }

    async fn sign_transaction(
        &self,
        transaction: &mut Transaction,
    ) -> TurnkeyResult<(Transaction, Signature)> {
        let serialized_message = transaction.message_data();

        let public_key = match self.address {
            Some(pubkey) => pubkey,
            None => return Err(TurnkeyError::ConfigError("Public key not available".into())),
        };

        let signature_bytes = self.sign_bytes(&serialized_message).await?;

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

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use serde_json::json;
//     use wiremock::matchers::{body_json, header, method, path};
//     use wiremock::{Mock, MockServer, ResponseTemplate};

//     #[test]
//     fn test_select_key() {
//         let service = TurnkeyService {
//             api_public_key: "test-api-public-key".to_string(),
//             api_private_key: SecretString::new("test-api-private-key"),
//             organization_id: "test-org-id".to_string(),
//             private_key_id: "test-private-key-id".to_string(),
//             public_key: "test-public-key".to_string(),
//             address: Some(Pubkey::new_unique()),
//             client: Client::new(),
//         };

//         let key_info = service.get_key_info();
//         assert_eq!(key_info.private_key_id, "test-private-key-id");
//         assert!(key_info.public_key.is_some());
//     }

//     // Setup a mock for sign_raw_payload response
//     async fn setup_mock_sign_raw_payload(mock_server: &MockServer) {
//         Mock::given(method("POST"))
//             .and(path("/public/v1/submit/sign_raw_payload"))
//             .respond_with(ResponseTemplate::new(200).set_body_json(json!({
//                 "activity": {
//                     "result": {
//                         "sign_raw_payload_result": {
//                             "r": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
//                             "s": "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"
//                         }
//                     }
//                 }
//             })))
//             .mount(mock_server)
//             .await;
//     }

//     #[tokio::test]
//     async fn test_sign_bytes() {
//         let mock_server = MockServer::start().await;
//         setup_mock_sign_raw_payload(&mock_server).await;

//         // Create a mock TurnkeyService
//         let service = TurnkeyService {
//             api_public_key: "test-api-public-key".to_string(),
//             api_private_key: SecretString::new(
//                 "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
//             ), // Using a mock hex string
//             organization_id: "test-org-id".to_string(),
//             private_key_id: "test-private-key-id".to_string(),
//             public_key: "test-public-key".to_string(),
//             address: Some(Pubkey::new_unique()),
//             client: reqwest::Client::new(),
//         };

//         // This test will fail since we can't actually connect to the mock server with our real client
//         // In a real test, we would need to modify the client to use the mock server URL
//         // For now, let's just create the test structure
//         // let result = service.sign_bytes(b"test message", "test-private-key-id".to_string()).await;
//         // assert!(result.is_ok());
//     }

//     // Additional test cases would be added for other methods
// }
