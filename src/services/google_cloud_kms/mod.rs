//! # Google Cloud KMS Service Module
//!
//! This module provides integration with Google Cloud KMS for secure key management
//! and cryptographic operations such as public key retrieval and message signing.
//!
//! ## Features
//!
//! - Service account authentication using google-cloud-auth
//! - Public key retrieval from KMS
//! - Message signing via KMS
//!
//! ## Architecture
//!
//! ```text
//! GoogleCloudKmsService (implements GoogleCloudKmsServiceTrait)
//!   ├── Authentication (service account)
//!   ├── Public Key Retrieval
//!   └── Message Signing
//! ```

use alloy::primitives::Keccak256;
use async_trait::async_trait;
use google_cloud_auth::credentials::{service_account::Builder as GcpCredBuilder, Credentials};
#[cfg_attr(test, allow(unused_imports))]
use http::{Extensions, HeaderMap};
use log::debug;
use reqwest::Client;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[cfg(test)]
use mockall::automock;

use crate::models::GoogleCloudKmsSignerConfig;
use crate::utils::{base64_decode, base64_encode};

#[derive(Debug, thiserror::Error)]
pub enum GoogleCloudKmsError {
    #[error("KMS HTTP error: {0}")]
    HttpError(String),
    #[error("KMS API error: {0}")]
    ApiError(String),
    #[error("KMS response parse error: {0}")]
    ParseError(String),
    #[error("KMS missing field: {0}")]
    MissingField(String),
    #[error("KMS config error: {0}")]
    ConfigError(String),
    #[error("Other error: {0}")]
    Other(String),
}

pub type GoogleCloudKmsResult<T> = Result<T, GoogleCloudKmsError>;

#[async_trait]
#[cfg_attr(test, automock)]
pub trait GoogleCloudKmsServiceTrait: Send + Sync {
    async fn get_solana_address(&self) -> GoogleCloudKmsResult<String>;
    async fn sign_solana(&self, message: &[u8]) -> GoogleCloudKmsResult<Vec<u8>>;
    async fn get_evm_address(&self) -> GoogleCloudKmsResult<String>;
    async fn sign_evm(&self, message: &[u8]) -> GoogleCloudKmsResult<Vec<u8>>;
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct GoogleCloudKmsService {
    pub config: GoogleCloudKmsSignerConfig,
    credentials: Arc<Credentials>,
    client: Client,
}

impl GoogleCloudKmsService {
    pub fn new(config: &GoogleCloudKmsSignerConfig) -> GoogleCloudKmsResult<Self> {
        let credentials_json = serde_json::json!({
            "type": "service_account",
            "project_id": config.service_account.project_id,
            "private_key_id": config.service_account.private_key_id.to_str().to_string(),
            "private_key": config.service_account.private_key.to_str().to_string(),
            "client_email": config.service_account.client_email.to_str().to_string(),
            "client_id": config.service_account.client_id,
            "auth_uri": config.service_account.auth_uri,
            "token_uri": config.service_account.token_uri,
            "auth_provider_x509_cert_url": config.service_account.auth_provider_x509_cert_url,
            "client_x509_cert_url": config.service_account.client_x509_cert_url,
            "universe_domain": config.service_account.universe_domain,
        });
        let credentials = GcpCredBuilder::new(credentials_json)
            .build()
            .map_err(|e| GoogleCloudKmsError::ConfigError(e.to_string()))?;

        Ok(Self {
            config: config.clone(),
            credentials: Arc::new(credentials),
            client: Client::new(),
        })
    }

    async fn get_auth_headers(&self) -> GoogleCloudKmsResult<HeaderMap> {
        // makes writing tests easier
        #[cfg(test)]
        {
            // In test mode, return empty headers or mock headers
            let mut headers = HeaderMap::new();
            headers.insert("Authorization", "Bearer test-token".parse().unwrap());
            Ok(headers)
        }

        #[cfg(not(test))]
        {
            self.credentials
                .headers(Extensions::new())
                .await
                .map_err(|e| GoogleCloudKmsError::ConfigError(e.to_string()))
        }
    }

    fn get_base_url(&self) -> String {
        if self
            .config
            .service_account
            .universe_domain
            .starts_with("http")
        {
            self.config.service_account.universe_domain.clone()
        } else {
            format!(
                "https://cloudkms.{}",
                self.config.service_account.universe_domain
            )
        }
    }

    async fn kms_get(&self, url: &str) -> GoogleCloudKmsResult<Value> {
        let headers = self.get_auth_headers().await?;
        let resp = self
            .client
            .get(url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| GoogleCloudKmsError::HttpError(e.to_string()))?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_else(|_| "".to_string());

        if !status.is_success() {
            return Err(GoogleCloudKmsError::ApiError(format!(
                "KMS request failed ({}): {}",
                status, text
            )));
        }

        serde_json::from_str(&text)
            .map_err(|e| GoogleCloudKmsError::ParseError(format!("{}: {}", e, text)))
    }

    async fn kms_post(&self, url: &str, body: &Value) -> GoogleCloudKmsResult<Value> {
        let headers = self.get_auth_headers().await?;
        let resp = self
            .client
            .post(url)
            .headers(headers)
            .json(body)
            .send()
            .await
            .map_err(|e| GoogleCloudKmsError::HttpError(e.to_string()))?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_else(|_| "".to_string());

        if !status.is_success() {
            return Err(GoogleCloudKmsError::ApiError(format!(
                "KMS request failed ({}): {}",
                status, text
            )));
        }

        serde_json::from_str(&text)
            .map_err(|e| GoogleCloudKmsError::ParseError(format!("{}: {}", e, text)))
    }

    fn get_key_path(&self) -> String {
        format!(
            "projects/{}/locations/global/keyRings/{}/cryptoKeys/{}/cryptoKeyVersions/{}",
            self.config.service_account.project_id,
            self.config.key.key_ring_id,
            self.config.key.key_id,
            self.config.key.key_version
        )
    }

    /// Fetches the PEM-encoded public key from KMS.
    async fn get_pem(&self) -> GoogleCloudKmsResult<String> {
        let base_url = self.get_base_url();
        let key_path = self.get_key_path();
        let url = format!("{}/v1/{}/publicKey", base_url, key_path,);
        debug!("KMS publicKey URL: {}", url);

        let body = self.kms_get(&url).await?;
        let pem_str = body
            .get("pem")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GoogleCloudKmsError::MissingField("pem".to_string()))?;

        Ok(pem_str.to_string())
    }

    /// Derives a Solana address from a PEM-encoded public key.
    fn derive_solana_address(pem_str: &str) -> GoogleCloudKmsResult<String> {
        let pkey =
            pem::parse(pem_str).map_err(|e| GoogleCloudKmsError::ParseError(e.to_string()))?;
        let content = pkey.contents();

        let mut array = [0u8; 32];

        match content.len() {
            32 => array.copy_from_slice(content),
            44 => array.copy_from_slice(&content[12..]),
            _ => {
                return Err(GoogleCloudKmsError::Other(format!(
                    "Unexpected ed25519 public key length: got {} bytes (expected 32 or 44).",
                    content.len()
                )));
            }
        }

        let solana_address = bs58::encode(array).into_string();
        Ok(solana_address)
    }

    fn derive_ethereum_address(pem_str: &str) -> GoogleCloudKmsResult<String> {
        let pkey =
            pem::parse(pem_str).map_err(|e| GoogleCloudKmsError::ParseError(e.to_string()))?;
        let der = pkey.contents();

        // Parse ASN.1 to extract the public key (as SEC1 bytes)
        let spki = simple_asn1::from_der(der)
            .map_err(|e| GoogleCloudKmsError::ParseError(format!("ASN.1 parse error: {e}")))?;
        let pubkey_bytes = if let Some(simple_asn1::ASN1Block::Sequence(_, blocks)) = spki.first() {
            if let Some(simple_asn1::ASN1Block::BitString(_, _, ref bytes)) = blocks.get(1) {
                bytes
            } else {
                return Err(GoogleCloudKmsError::ParseError(
                    "Invalid ASN.1 structure for public key".to_string(),
                ));
            }
        } else {
            return Err(GoogleCloudKmsError::ParseError(
                "Invalid ASN.1 structure for public key".to_string(),
            ));
        };

        // Compute Keccak-256 hash of the public key (skip the 0x04 prefix)
        let mut hasher = Keccak256::new();
        hasher.update(&pubkey_bytes[1..]);
        let hash = hasher.finalize();

        // Take the last 20 bytes of the hash
        let address_bytes = &hash[hash.len() - 20..];

        // Convert to hexadecimal string
        Ok(format!("0x{}", hex::encode(address_bytes)))
    }
}

#[async_trait]
impl GoogleCloudKmsServiceTrait for GoogleCloudKmsService {
    async fn get_solana_address(&self) -> GoogleCloudKmsResult<String> {
        let pem_str = self.get_pem().await?;

        println!("PEM solana: {}", pem_str);

        Self::derive_solana_address(&pem_str)
    }

    async fn get_evm_address(&self) -> GoogleCloudKmsResult<String> {
        let pem_str = self.get_pem().await?;

        println!("PEM evm: {}", pem_str);

        Self::derive_ethereum_address(&pem_str)
    }

    async fn sign_solana(&self, message: &[u8]) -> GoogleCloudKmsResult<Vec<u8>> {
        let base_url = self.get_base_url();
        let key_path = self.get_key_path();

        let url = format!("{}/v1/{}:asymmetricSign", base_url, key_path,);
        debug!("KMS asymmetricSign URL: {}", url);

        let body = serde_json::json!({
            "name": key_path,
            "data": base64_encode(message)
        });

        print!("KMS asymmetricSign body: {}", body);

        let resp = self.kms_post(&url, &body).await?;
        let signature_b64 = resp
            .get("signature")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GoogleCloudKmsError::MissingField("signature".to_string()))?;

        println!("KMS asymmetricSign response: {}", resp);

        let signature = base64_decode(signature_b64)
            .map_err(|e| GoogleCloudKmsError::ParseError(e.to_string()))?;

        Ok(signature)
    }

    async fn sign_evm(&self, message: &[u8]) -> GoogleCloudKmsResult<Vec<u8>> {
        let base_url = self.get_base_url();
        let key_path = self.get_key_path();
        let url = format!("{}/v1/{}:asymmetricSign", base_url, key_path,);
        debug!("KMS asymmetricSign URL: {}", url);

        let hash = Sha256::digest(message);
        let digest = base64_encode(&hash);

        let body = serde_json::json!({
            "name": key_path,
            "digest": {
                "sha256": digest
            }
        });

        print!("KMS asymmetricSign body: {}", body);

        let resp = self.kms_post(&url, &body).await?;
        let signature = resp
            .get("signature")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GoogleCloudKmsError::MissingField("signature".to_string()))?;

        println!("KMS asymmetricSign response: {}", resp);
        let signature_b64 =
            base64_decode(signature).map_err(|e| GoogleCloudKmsError::ParseError(e.to_string()))?;
        print!("Signature b64 decoded: {:?}", signature_b64);
        Ok(signature_b64)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        GoogleCloudKmsSignerConfig, GoogleCloudKmsSignerKeyConfig,
        GoogleCloudKmsSignerServiceAccountConfig, SecretString,
    };
    use serde_json::json;
    use wiremock::matchers::{header_exists, method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn create_test_config(uri: &str) -> GoogleCloudKmsSignerConfig {
        GoogleCloudKmsSignerConfig {
            service_account: GoogleCloudKmsSignerServiceAccountConfig {
                project_id: "test-project".to_string(),
                private_key_id: SecretString::new("test-private-key-id"),
                private_key: SecretString::new("-----BEGIN EXAMPLE PRIVATE KEY-----\nFAKEKEYDATA\n-----END EXAMPLE PRIVATE KEY-----\n"),
                client_email: SecretString::new("test-service-account@example.com"),
                client_id: "test-client-id".to_string(),
                auth_uri: "https://accounts.google.com/o/oauth2/auth".to_string(),
                token_uri: "https://oauth2.googleapis.com/token".to_string(),
                client_x509_cert_url: "https://www.googleapis.com/robot/v1/metadata/x509/test-service-account%40example.com".to_string(),
                auth_provider_x509_cert_url: "https://www.googleapis.com/oauth2/v1/certs".to_string(),
                universe_domain: uri.to_string(),
            },
            key: GoogleCloudKmsSignerKeyConfig {
                key_id: "test-key-id".to_string(),
                key_ring_id: "test-key-ring-id".to_string(),
                key_version: 1,
            },
        }
    }

    #[tokio::test]
    async fn test_new_google_cloud_kms_service() {
        let config = create_test_config("server_uri");
        let service = GoogleCloudKmsService::new(&config);
        assert!(service.is_ok());
    }

    #[tokio::test]
    async fn test_get_key_path() {
        let config = create_test_config("server_uri");
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let key_path = service.get_key_path();
        let expected_path = "projects/test-project/locations/global/keyRings/test-key-ring-id/cryptoKeys/test-key-id/cryptoKeyVersions/1";

        assert_eq!(key_path, expected_path);
    }

    #[test]
    fn test_derive_ethereum_address() {
        let pem = "not-a-valid-pem";
        let result = GoogleCloudKmsService::derive_ethereum_address(pem);
        assert!(result.is_err());

        static VALID_SECP256K1_PEM: &str = "-----BEGIN PUBLIC KEY-----\nMFYwEAYHKoZIzj0CAQYFK4EEAAoDQgAEjJaJh5wfZwvj8b3bQ4GYikqDTLXWUjMh
kFs9lGj2N9B17zo37p4PSy99rDio0QHLadpso0rtTJDSISRW9MdOqA==\n-----END PUBLIC KEY-----\n";

        let result = GoogleCloudKmsService::derive_ethereum_address(VALID_SECP256K1_PEM);
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            "0xeeb8861f51b3f3f2204d64bbf7a7eb25e1b4d6cd"
        );
    }

    #[test]
    fn test_derive_solana_address() {
        let pem = "not-a-valid-pem";
        let result = GoogleCloudKmsService::derive_solana_address(pem);
        assert!(result.is_err());

        static VALID_ED25519_PEM: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAnUV+ReQWxMZ3Z2pC/5aOPPjcc8jzOo0ZgSl7+j4AMLo=\n-----END PUBLIC KEY-----\n";
        let result = GoogleCloudKmsService::derive_solana_address(VALID_ED25519_PEM);
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            "BavUBpkD77FABnevMkBVqV8BDHv7gX8sSoYYJY9WU9L5"
        );
    }

    // Setup mock for getting public key
    async fn setup_mock_get_public_key(mock_server: &MockServer) {
        Mock::given(method("GET"))
            .and(path_regex(r"/v1/projects/.*/locations/global/keyRings/.*/cryptoKeys/.*/cryptoKeyVersions/.*/publicKey"))
            .and(header_exists("Authorization"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "pem": "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAVyC+iqnSu0vo6R8x0sRMhintQtoZgcLOur1VyvCrdrs=\n-----END PUBLIC KEY-----\n",
                "algorithm": "ECDSA_P256_SHA256"
            })))
            .mount(mock_server)
            .await;
    }

    // Setup mock for signing Solana transactions
    async fn setup_mock_sign_solana(mock_server: &MockServer) {
        Mock::given(method("POST"))
            .and(path_regex(r"/v1/projects/.*/locations/global/keyRings/.*/cryptoKeys/.*/cryptoKeyVersions/.*:asymmetricSign"))
            .and(header_exists("Authorization"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "signature": "ZHVtbXlzaWduYXR1cmU="  // Base64 encoded "dummysignature"
            })))
            .mount(mock_server)
            .await;
    }

    // Setup mock for signing EVM transactions
    async fn setup_mock_sign_evm(mock_server: &MockServer) {
        Mock::given(method("POST"))
            .and(path_regex(r"/v1/projects/.*/locations/global/keyRings/.*/cryptoKeys/.*/cryptoKeyVersions/.*:asymmetricSign"))
            .and(header_exists("Authorization"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "signature": "ZHVtbXlzaWduYXR1cmU="  // Base64 encoded "dummysignature"
            })))
            .mount(mock_server)
            .await;
    }

    async fn setup_mock_error_response(mock_server: &MockServer) {
        Mock::given(method("POST"))
            .and(path_regex(r"/v1/projects/.*/locations/global/keyRings/.*/cryptoKeys/.*/cryptoKeyVersions/.*:asymmetricSign"))
            .and(header_exists("Authorization"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": {
                    "code": 400,
                    "message": "Invalid request",
                    "status": "INVALID_ARGUMENT"
                }
            })))
            .mount(mock_server)
            .await;
    }

    #[tokio::test]
    async fn test_get_solana_address_with_mock() {
        let mock_server = MockServer::start().await;

        setup_mock_get_public_key(&mock_server).await;

        let config = create_test_config(&mock_server.uri());
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let result = service.get_solana_address().await;
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            "6s7RsvzcdXFJi1tXeDoGfSKZWjCDNJLiu74rd72zLy6J"
        );
    }

    #[tokio::test]
    async fn test_get_evm_address_with_mock() {
        let mock_server = MockServer::start().await;

        setup_mock_get_public_key(&mock_server).await;

        let config = create_test_config(&mock_server.uri());
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let result = service.get_evm_address().await;
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            "0xcb9955746ac0d84666e8ed2f1e72ecc9f8e1e87d"
        );
    }

    #[tokio::test]
    async fn test_sign_solana_with_mock() {
        let mock_server = MockServer::start().await;

        setup_mock_sign_solana(&mock_server).await;

        let config = create_test_config(&mock_server.uri());
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let result = service.sign_solana(b"test message").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "dummysignature".as_bytes().to_vec());
    }

    #[tokio::test]
    async fn test_sign_evm_with_mock() {
        let mock_server = MockServer::start().await;

        setup_mock_sign_evm(&mock_server).await;

        let config = create_test_config(&mock_server.uri());
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let result = service.sign_evm(b"test message").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "dummysignature".as_bytes().to_vec());
    }

    #[tokio::test]
    async fn test_sign_evm_with_mock_err() {
        let mock_server = MockServer::start().await;

        setup_mock_error_response(&mock_server).await;

        let config = create_test_config(&mock_server.uri());
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let result = service.sign_evm(b"test message").await;
        assert!(result.is_err());
        match result {
            Err(GoogleCloudKmsError::ApiError(_)) => {}
            Err(e) => {
                panic!("Unexpected error type: {:?}", e);
            }
            _ => panic!("Expected error for invalid request"),
        }
    }

    #[tokio::test]
    async fn test_sign_evm_error() {
        let config = create_test_config("server_uri");
        let service = GoogleCloudKmsService::new(&config).unwrap();
        // This will fail because the credentials are not valid and no mock server is used
        let result = service.sign_evm(b"test").await;
        assert!(result.is_err());
        match result {
            Err(GoogleCloudKmsError::HttpError(_)) | Err(GoogleCloudKmsError::ConfigError(_)) => {}
            Err(e) => {
                panic!("Unexpected error type: {:?}", e);
            }
            _ => panic!("Expected error for invalid credentials"),
        }
    }

    #[tokio::test]
    async fn test_sign_solana_error() {
        let config = create_test_config("server_uri");
        let service = GoogleCloudKmsService::new(&config).unwrap();
        // This will fail because the credentials are not valid and no mock server is used
        let result = service.sign_solana(b"test").await;
        assert!(result.is_err());
        match result {
            Err(GoogleCloudKmsError::HttpError(_)) | Err(GoogleCloudKmsError::ConfigError(_)) => {}
            Err(e) => {
                panic!("Unexpected error type: {:?}", e);
            }
            _ => panic!("Expected error for invalid credentials"),
        }
    }

    #[tokio::test]
    async fn test_get_solana_address_error() {
        let config = create_test_config("server_uri");
        let service = GoogleCloudKmsService::new(&config).unwrap();
        let result = service.get_solana_address().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_evm_address_error() {
        let config = create_test_config("server_uri");
        let service = GoogleCloudKmsService::new(&config).unwrap();
        let result = service.get_evm_address().await;
        assert!(result.is_err());
    }
}
