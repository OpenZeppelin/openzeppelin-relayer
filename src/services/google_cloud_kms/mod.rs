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
//! GoogleCloudKmsService (implements GoogleCloudKmsServiceTrait, GoogleCloudKmsEvmService)
//!   ├── Authentication (service account)
//!   ├── Public Key Retrieval
//!   └── Message Signing
//! ```

use alloy::primitives::keccak256;
use async_trait::async_trait;
use google_cloud_auth::credentials::{service_account::Builder as GcpCredBuilder, Credentials};
#[cfg_attr(test, allow(unused_imports))]
use http::{Extensions, HeaderMap};
use log::debug;
use reqwest::Client;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::RwLock;

#[cfg(test)]
use mockall::automock;

use crate::models::{Address, GoogleCloudKmsSignerConfig};
use crate::utils::{
    self, base64_decode, base64_encode, derive_ethereum_address_from_pem,
    extract_public_key_from_der,
};

#[derive(Debug, thiserror::Error, serde::Serialize)]
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
    #[error("KMS conversion error: {0}")]
    ConvertError(String),
    #[error("KMS public key error: {0}")]
    RecoveryError(#[from] utils::Secp256k1Error),
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

#[async_trait]
#[cfg_attr(test, automock)]
pub trait GoogleCloudKmsEvmService: Send + Sync {
    /// Returns the EVM address derived from the configured public key.
    async fn get_evm_address(&self) -> GoogleCloudKmsResult<Address>;
    /// Signs a payload using the EVM signing scheme.
    /// Pre-hashes the message with keccak-256.
    async fn sign_payload_evm(&self, payload: &[u8]) -> GoogleCloudKmsResult<Vec<u8>>;
}

#[async_trait]
#[cfg_attr(test, automock)]
pub trait GoogleCloudKmsK256: Send + Sync {
    /// Fetches the PEM-encoded public key from Google Cloud KMS.
    async fn get_pem_public_key(&self) -> GoogleCloudKmsResult<String>;
    /// Signs a digest using ECDSA_SHA256. Returns DER-encoded signature.
    async fn sign_digest(&self, digest: [u8; 32]) -> GoogleCloudKmsResult<Vec<u8>>;
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct GoogleCloudKmsService {
    pub config: GoogleCloudKmsSignerConfig,
    credentials: Arc<Credentials>,
    client: Client,
    cached_headers: Arc<RwLock<Option<HeaderMap>>>,
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
            cached_headers: Arc::new(RwLock::new(None)),
        })
    }

    async fn get_auth_headers(&self) -> GoogleCloudKmsResult<HeaderMap> {
        #[cfg(test)]
        {
            // In test mode, return empty headers or mock headers
            let mut headers = HeaderMap::new();
            headers.insert("Authorization", "Bearer test-token".parse().unwrap());
            Ok(headers)
        }

        #[cfg(not(test))]
        {
            let cacheable_headers = self
                .credentials
                .headers(Extensions::new())
                .await
                .map_err(|e| GoogleCloudKmsError::ConfigError(e.to_string()))?;

            match cacheable_headers {
                google_cloud_auth::credentials::CacheableResource::New { data, .. } => {
                    let mut cached = self.cached_headers.write().await;
                    *cached = Some(data.clone());
                    Ok(data)
                }
                google_cloud_auth::credentials::CacheableResource::NotModified => {
                    let cached = self.cached_headers.read().await;
                    if let Some(headers) = cached.as_ref() {
                        Ok(headers.clone())
                    } else {
                        Err(GoogleCloudKmsError::ConfigError(
                            "KMS auth token not modified, but not found in cache".to_string(),
                        ))
                    }
                }
            }
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
            "projects/{}/locations/{}/keyRings/{}/cryptoKeys/{}/cryptoKeyVersions/{}",
            self.config.service_account.project_id,
            self.config.key.location,
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

    /// Signs a bytes with the private key stored in Google Cloud KMS.
    ///
    /// Pre-hashes the message with keccak256.
    pub async fn sign_bytes_evm(&self, bytes: &[u8]) -> GoogleCloudKmsResult<Vec<u8>> {
        let digest = keccak256(bytes).0;
        let der_signature = self.sign_digest(digest).await?;

        // Parse DER into Secp256k1 format
        let rs = k256::ecdsa::Signature::from_der(&der_signature)
            .map_err(|e| GoogleCloudKmsError::ParseError(e.to_string()))?;

        let pem_str = self.get_pem().await?;

        // Convert PEM to DER first, then extract public key
        let pem_parsed =
            pem::parse(&pem_str).map_err(|e| GoogleCloudKmsError::ParseError(e.to_string()))?;
        let der_pk = pem_parsed.contents();

        let pk = extract_public_key_from_der(der_pk)
            .map_err(|e| GoogleCloudKmsError::ConvertError(e.to_string()))?;

        let v = utils::recover_public_key(&pk, &rs, bytes)?;

        // Adjust v value for Ethereum legacy transaction.
        let eth_v = 27 + v;

        let mut sig_bytes = rs.to_vec();
        sig_bytes.push(eth_v);

        Ok(sig_bytes)
    }
}

#[async_trait]
impl GoogleCloudKmsK256 for GoogleCloudKmsService {
    async fn get_pem_public_key(&self) -> GoogleCloudKmsResult<String> {
        self.get_pem().await
    }

    async fn sign_digest(&self, digest: [u8; 32]) -> GoogleCloudKmsResult<Vec<u8>> {
        let base_url = self.get_base_url();
        let key_path = self.get_key_path();
        let url = format!("{}/v1/{}:asymmetricSign", base_url, key_path);

        let digest_b64 = base64_encode(&digest);

        let body = serde_json::json!({
            "name": key_path,
            "digest": {
                "sha256": digest_b64
            }
        });

        let resp = self.kms_post(&url, &body).await?;
        let signature_b64 = resp
            .get("signature")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GoogleCloudKmsError::MissingField("signature".to_string()))?;

        let signature = base64_decode(signature_b64)
            .map_err(|e| GoogleCloudKmsError::ParseError(e.to_string()))?;

        Ok(signature)
    }
}

#[async_trait]
impl GoogleCloudKmsServiceTrait for GoogleCloudKmsService {
    async fn get_solana_address(&self) -> GoogleCloudKmsResult<String> {
        let pem_str = self.get_pem().await?;

        println!("PEM solana: {}", pem_str);

        utils::derive_solana_address_from_pem(&pem_str).map_err(GoogleCloudKmsError::from)
    }

    async fn get_evm_address(&self) -> GoogleCloudKmsResult<String> {
        let pem_str = self.get_pem().await?;

        println!("PEM evm: {}", pem_str);

        let address_bytes =
            utils::derive_ethereum_address_from_pem(&pem_str).map_err(GoogleCloudKmsError::from)?;
        Ok(format!("0x{}", hex::encode(address_bytes)))
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

#[async_trait]
impl GoogleCloudKmsEvmService for GoogleCloudKmsService {
    async fn get_evm_address(&self) -> GoogleCloudKmsResult<Address> {
        let pem_str = self.get_pem().await?;
        let eth_address = derive_ethereum_address_from_pem(&pem_str)
            .map_err(|e| GoogleCloudKmsError::ParseError(e.to_string()))?;
        Ok(Address::Evm(eth_address))
    }

    async fn sign_payload_evm(&self, payload: &[u8]) -> GoogleCloudKmsResult<Vec<u8>> {
        self.sign_bytes_evm(payload).await
    }
}

impl From<utils::AddressDerivationError> for GoogleCloudKmsError {
    fn from(value: utils::AddressDerivationError) -> Self {
        match value {
            utils::AddressDerivationError::ParseError(msg) => GoogleCloudKmsError::ParseError(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        GoogleCloudKmsSignerKeyConfig, GoogleCloudKmsSignerServiceAccountConfig, SecretString,
    };
    use alloy::primitives::utils::eip191_message;
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
                location: "global".to_string(),
                key_id: "test-key-id".to_string(),
                key_ring_id: "test-key-ring-id".to_string(),
                key_version: 1,
            },
        }
    }

    #[tokio::test]
    async fn test_service_creation_success() {
        let config = create_test_config("https://example.com");
        let result = GoogleCloudKmsService::new(&config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_key_path_format() {
        let config = create_test_config("https://example.com");
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let key_path = service.get_key_path();
        let expected = "projects/test-project/locations/global/keyRings/test-key-ring-id/cryptoKeys/test-key-id/cryptoKeyVersions/1";

        assert_eq!(key_path, expected);
    }

    #[tokio::test]
    async fn test_get_base_url_with_http_prefix() {
        let config = create_test_config("http://localhost:8080");
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let base_url = service.get_base_url();
        assert_eq!(base_url, "http://localhost:8080");
    }

    #[tokio::test]
    async fn test_get_base_url_without_http_prefix() {
        let config = create_test_config("googleapis.com");
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let base_url = service.get_base_url();
        assert_eq!(base_url, "https://cloudkms.googleapis.com");
    }

    // Mock setup helpers
    async fn setup_mock_solana_public_key(mock_server: &MockServer) {
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

    async fn setup_mock_evm_public_key(mock_server: &MockServer) {
        Mock::given(method("GET"))
            .and(path_regex(r"/v1/projects/.*/locations/global/keyRings/.*/cryptoKeys/.*/cryptoKeyVersions/.*/publicKey"))
            .and(header_exists("Authorization"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "pem": "-----BEGIN PUBLIC KEY-----\nMFYwEAYHKoZIzj0CAQYFK4EEAAoDQgAEjJaJh5wfZwvj8b3bQ4GYikqDTLXWUjMh\nkFs9lGj2N9B17zo37p4PSy99rDio0QHLadpso0rtTJDSISRW9MdOqA==\n-----END PUBLIC KEY-----\n", // noboost
                "algorithm": "ECDSA_SECP256K1_SHA256"
            })))
            .mount(mock_server)
            .await;
    }

    async fn setup_mock_sign_success(mock_server: &MockServer) {
        Mock::given(method("POST"))
            .and(path_regex(r"/v1/projects/.*/locations/global/keyRings/.*/cryptoKeys/.*/cryptoKeyVersions/.*:asymmetricSign"))
            .and(header_exists("Authorization"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "signature": "ZHVtbXlzaWduYXR1cmU="  // Base64 encoded "dummysignature"
            })))
            .mount(mock_server)
            .await;
    }

    async fn setup_mock_sign_error(mock_server: &MockServer) {
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

    async fn setup_mock_get_key_error(mock_server: &MockServer) {
        Mock::given(method("GET"))
            .and(path_regex(r"/v1/projects/.*/locations/global/keyRings/.*/cryptoKeys/.*/cryptoKeyVersions/.*/publicKey"))
            .and(header_exists("Authorization"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "error": {
                    "code": 404,
                    "message": "Key not found",
                    "status": "NOT_FOUND"
                }
            })))
            .mount(mock_server)
            .await;
    }

    async fn setup_mock_malformed_response(mock_server: &MockServer) {
        Mock::given(method("GET"))
            .and(path_regex(r"/v1/projects/.*/locations/global/keyRings/.*/cryptoKeys/.*/cryptoKeyVersions/.*/publicKey"))
            .and(header_exists("Authorization"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "algorithm": "ED25519"
                // Missing "pem" field
            })))
            .mount(mock_server)
            .await;
    }

    // GoogleCloudKmsServiceTrait tests
    #[tokio::test]
    async fn test_get_solana_address_success() {
        let mock_server = MockServer::start().await;
        setup_mock_solana_public_key(&mock_server).await;

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
    async fn test_get_solana_address_api_error() {
        let mock_server = MockServer::start().await;
        setup_mock_get_key_error(&mock_server).await;

        let config = create_test_config(&mock_server.uri());
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let result = service.get_solana_address().await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GoogleCloudKmsError::ApiError(_)
        ));
    }

    #[tokio::test]
    async fn test_get_evm_address_success() {
        let mock_server = MockServer::start().await;
        setup_mock_evm_public_key(&mock_server).await;

        let config = create_test_config(&mock_server.uri());
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let result = GoogleCloudKmsServiceTrait::get_evm_address(&service).await;
        assert!(result.is_ok());

        let address = result.unwrap();
        assert!(address.starts_with("0x"));
        assert_eq!(address.len(), 42);
    }

    #[tokio::test]
    async fn test_sign_solana_success() {
        let mock_server = MockServer::start().await;
        setup_mock_sign_success(&mock_server).await;

        let config = create_test_config(&mock_server.uri());
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let result = service.sign_solana(b"test message").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), b"dummysignature");
    }

    #[tokio::test]
    async fn test_sign_solana_api_error() {
        let mock_server = MockServer::start().await;
        setup_mock_sign_error(&mock_server).await;

        let config = create_test_config(&mock_server.uri());
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let result = service.sign_solana(b"test message").await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GoogleCloudKmsError::ApiError(_)
        ));
    }

    #[tokio::test]
    async fn test_sign_evm_success() {
        let mock_server = MockServer::start().await;
        setup_mock_sign_success(&mock_server).await;

        let config = create_test_config(&mock_server.uri());
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let result = service.sign_evm(b"test message").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), b"dummysignature");
    }

    #[tokio::test]
    async fn test_sign_evm_api_error() {
        let mock_server = MockServer::start().await;
        setup_mock_sign_error(&mock_server).await;

        let config = create_test_config(&mock_server.uri());
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let result = service.sign_evm(b"test message").await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GoogleCloudKmsError::ApiError(_)
        ));
    }

    // GoogleCloudKmsEvmService tests
    #[tokio::test]
    async fn test_evm_service_get_address_success() {
        let mock_server = MockServer::start().await;
        setup_mock_evm_public_key(&mock_server).await;

        let config = create_test_config(&mock_server.uri());
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let result = GoogleCloudKmsEvmService::get_evm_address(&service).await;
        assert!(result.is_ok());

        let address = result.unwrap();
        assert!(matches!(address, Address::Evm(_)));
        if let Address::Evm(addr) = address {
            assert_eq!(addr.len(), 20);
        }
    }

    #[tokio::test]
    async fn test_evm_service_get_address_api_error() {
        let mock_server = MockServer::start().await;
        setup_mock_get_key_error(&mock_server).await;

        let config = create_test_config(&mock_server.uri());
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let result = GoogleCloudKmsEvmService::get_evm_address(&service).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GoogleCloudKmsError::ApiError(_)
        ));
    }

    #[tokio::test]
    async fn test_sign_payload_evm_network_error() {
        let config = create_test_config("http://invalid-host:9999");
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let message = eip191_message(b"Hello World!");
        let result = GoogleCloudKmsEvmService::sign_payload_evm(&service, &message).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GoogleCloudKmsError::HttpError(_)
        ));
    }

    #[tokio::test]
    async fn test_get_pem_public_key_success() {
        let mock_server = MockServer::start().await;
        setup_mock_evm_public_key(&mock_server).await;

        let config = create_test_config(&mock_server.uri());
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let result = GoogleCloudKmsK256::get_pem_public_key(&service).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("BEGIN PUBLIC KEY"));
    }

    #[tokio::test]
    async fn test_get_pem_public_key_missing_field() {
        let mock_server = MockServer::start().await;
        setup_mock_malformed_response(&mock_server).await;

        let config = create_test_config(&mock_server.uri());
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let result = GoogleCloudKmsK256::get_pem_public_key(&service).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GoogleCloudKmsError::MissingField(_)
        ));
    }

    #[tokio::test]
    async fn test_sign_digest_success() {
        let mock_server = MockServer::start().await;
        setup_mock_sign_success(&mock_server).await;

        let config = create_test_config(&mock_server.uri());
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let digest = [0u8; 32];
        let result = GoogleCloudKmsK256::sign_digest(&service, digest).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), b"dummysignature");
    }

    #[tokio::test]
    async fn test_sign_digest_api_error() {
        let mock_server = MockServer::start().await;
        setup_mock_sign_error(&mock_server).await;

        let config = create_test_config(&mock_server.uri());
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let digest = [0u8; 32];
        let result = GoogleCloudKmsK256::sign_digest(&service, digest).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GoogleCloudKmsError::ApiError(_)
        ));
    }

    #[tokio::test]
    async fn test_network_failure_handling() {
        let config = create_test_config("http://localhost:99999"); // Invalid port
        let service = GoogleCloudKmsService::new(&config).unwrap();

        // Test all methods fail gracefully with network errors
        let solana_addr_result = service.get_solana_address().await;
        assert!(solana_addr_result.is_err());
        assert!(matches!(
            solana_addr_result.unwrap_err(),
            GoogleCloudKmsError::HttpError(_)
        ));

        let evm_addr_result = GoogleCloudKmsServiceTrait::get_evm_address(&service).await;
        assert!(evm_addr_result.is_err());
        assert!(matches!(
            evm_addr_result.unwrap_err(),
            GoogleCloudKmsError::HttpError(_)
        ));

        let sign_solana_result = service.sign_solana(b"test").await;
        assert!(sign_solana_result.is_err());
        assert!(matches!(
            sign_solana_result.unwrap_err(),
            GoogleCloudKmsError::HttpError(_)
        ));

        let sign_evm_result = service.sign_evm(b"test").await;
        assert!(sign_evm_result.is_err());
        assert!(matches!(
            sign_evm_result.unwrap_err(),
            GoogleCloudKmsError::HttpError(_)
        ));
    }

    #[tokio::test]
    async fn test_config_with_different_universe_domains() {
        let config1 = create_test_config("googleapis.com");
        let service1 = GoogleCloudKmsService::new(&config1).unwrap();
        assert_eq!(service1.get_base_url(), "https://cloudkms.googleapis.com");

        let config2 = create_test_config("https://custom-domain.com");
        let service2 = GoogleCloudKmsService::new(&config2).unwrap();
        assert_eq!(service2.get_base_url(), "https://custom-domain.com");
    }

    #[tokio::test]
    async fn test_solana_address_derivation() {
        let valid_ed25519_pem = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAnUV+ReQWxMZ3Z2pC/5aOPPjcc8jzOo0ZgSl7+j4AMLo=\n-----END PUBLIC KEY-----\n";
        let result = utils::derive_solana_address_from_pem(valid_ed25519_pem);
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            "BavUBpkD77FABnevMkBVqV8BDHv7gX8sSoYYJY9WU9L5"
        );
    }

    #[tokio::test]
    async fn test_malformed_json_response() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path_regex(r"/v1/projects/.*/locations/global/keyRings/.*/cryptoKeys/.*/cryptoKeyVersions/.*/publicKey"))
            .and(header_exists("Authorization"))
            .respond_with(ResponseTemplate::new(200).set_body_string("invalid json"))
            .mount(&mock_server)
            .await;

        let config = create_test_config(&mock_server.uri());
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let result = service.get_solana_address().await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GoogleCloudKmsError::ParseError(_)
        ));
    }

    #[tokio::test]
    async fn test_missing_signature_field_in_response() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex(r"/v1/projects/.*/locations/global/keyRings/.*/cryptoKeys/.*/cryptoKeyVersions/.*:asymmetricSign"))
            .and(header_exists("Authorization"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "test-key"
                // Missing "signature" field
            })))
            .mount(&mock_server)
            .await;

        let config = create_test_config(&mock_server.uri());
        let service = GoogleCloudKmsService::new(&config).unwrap();

        let result = service.sign_solana(b"test").await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GoogleCloudKmsError::MissingField(_)
        ));
    }
}
