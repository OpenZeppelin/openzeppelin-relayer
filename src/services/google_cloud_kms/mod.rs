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
        self.credentials
            .headers(Extensions::new())
            .await
            .map_err(|e| GoogleCloudKmsError::ConfigError(e.to_string()))
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
        let key_path = self.get_key_path();
        let url = format!(
            "https://cloudkms.{}/v1/{}/publicKey",
            self.config.service_account.universe_domain, key_path,
        );
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
        let pubkey_bytes = if let Some(simple_asn1::ASN1Block::Sequence(_, blocks)) = spki.get(0) {
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

        Self::derive_solana_address(&pem_str)
    }

    async fn get_evm_address(&self) -> GoogleCloudKmsResult<String> {
        let pem_str = self.get_pem().await?;

        Self::derive_ethereum_address(&pem_str)
    }

    async fn sign_solana(&self, message: &[u8]) -> GoogleCloudKmsResult<Vec<u8>> {
        let key_path = self.get_key_path();
        let url = format!(
            "https://cloudkms.{}/v1/{}:asymmetricSign",
            self.config.service_account.universe_domain, key_path,
        );
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
        let key_path = self.get_key_path();
        let url = format!(
            "https://cloudkms.{}/v1/{}:asymmetricSign",
            self.config.service_account.universe_domain, key_path,
        );
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
    use crate::models::{
        GoogleCloudKmsSignerKeyConfig, GoogleCloudKmsSignerServiceAccountConfig, SecretString,
    };

    use super::*;

    #[tokio::test]
    async fn test_get_address_error() {
        let service = GoogleCloudKmsService::new({
            &GoogleCloudKmsSignerConfig {
                service_account: GoogleCloudKmsSignerServiceAccountConfig {
                    project_id: "forward-emitter-459820-r7".to_string(),
                    private_key_id: SecretString::new("fa20d3a09900acf096dfa321af6bfcd9099fe0f3"),
                    private_key: SecretString::new("-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCmygTJMdF0I0op\n7FUn0RXaopMj4RuA9oMBMhO/ZQpubg9Yue9EV8NJrYRVk51SGi7QUiQ2mCUk/Wwi\nimepN+EY2GVBDtKhIhmJtwaOlPD3oydswPmaBqsHtBolwe7kVzP4gcWckV0e5RTJ\nP2AXfxeKobjA2rnOJEv8qaSlVMx5bPiE7II1Ppg/B35yXMjdEkzHy6Ckbo/j2/VX\nnKVFy9SSfwXbPx2O+AO+OcDX/hR9FYnOlB0SUlvw3vAwix1XS6INFzoWCYFmiNyF\nUxtQ0dddFkRM68IfSPNctA2oITXrUigGecnMxSkwoJpe43pUHh0Kackv1brVwZ0C\n3x86YzyTAgMBAAECggEAJ+daWMZt/bK5uij8QJ/x8hKGUIW5XlPcCOuxtM9YPQ5g\n5pHpkDjNFLIKYK0F7RAtlhKo7lTdQinNdsAVR/OCC78uDnAc7Ycqg/vBelhhBGZI\n9uh+bx7cSWYKAXGSFZhVa0WlAS34EP2uyO08MLLr73N8r8tEu/CMK4Fszk9o8j3A\neMT4kH3FTGaEjDdLxERXH1UjeA6/PlZam49Vdirum3vatvLZAilsyxJYds3Hgd8c\noqjDLj0rnfAHzrYLgmOoOeWfbIZQlFFwetOT/+UHmecwycqEyVX5WIh7UIvY4vsL\nq1vgVbDuKbk3mRcIDJy1RJ3LgpVCVYocZy+SoegcpQKBgQDatTM+CueIRxMg42Zc\nHhSKCbKuQRYg+GTng9LL6v1V35ssynrJssOwCCH5p/hJhCHhQBKqos7rmicSlw9M\nJdP453W9fNPv5XP/gorDT+cTEpxyUcOd7CbT0otRf7s1fRuIorWqStWAZDswS5Q3\nPvmU3YNRuePZJH8E5YMzhNICtQKBgQDDOoUWdVt/pI8ZB63gNb6LTzVAxWLHm2EY\nU7Y9aj7emNtvrXb6l4SAcHbCls48KEQA9H6G7ycQDm2EMmOPSkglqhQMY26lY25n\nd9SNEzo0wssVw1y83CvRiX4WTqI9zoUDtnyCbITXjnxlyNkVpnUZh6MGk9XVRVTA\nNYYDzNZnJwKBgQCGy3cxnfblfyjC9GR6EfAGw8Nksqi42V8XcZ/SHprU+mPhT0ou\nVgdVzy1hea0FYnKfKaZXlNCDVRcP1hqPjCEBH2bpyq21BW5g5Ewx5GU+1BGoQ8yU\n4J9tni5PpLH1XY5CwEXHFyhPYXc5ZNuM0TtyDLSLAk7z3hKLKgmbDxmAoQKBgFOO\nK0HGbqe9tWUADWHlfqy+9MrI8BMAJFk2EsxMOaYpg9lTQ5XS3WnfOGTmCFRk414J\nRlHX7z8G/cZTjprYLvK3zSbUM5njaXAtMDJE5WeJa0PgPkOyc6qVjvpbI0MSrYk+\nRCHJ8j0ThZhGkuqaOIn5rEN3aFCEANbW0Ym01JqHAoGAL+9s4OWvniibf6enP0h3\nr+ugnQrEz5Pgxot5hY7fRNl87hL6LUNRWkWhluBvzwKM/QVvKuBr2GxLuUziiZms\nqDV0h+oSQGSuYXEp8zqSaFq0J38AKg0Ug4JwG+geqTMnEp6H8h9uRDsnweQzadPR\njwNmjvi4t3+xOgvVotK4gbQ=\n-----END PRIVATE KEY-----\n"),
                    client_email: SecretString::new("solana-signer@forward-emitter-459820-r7.iam.gserviceaccount.com"),
                    client_id: "102715424486165246122".to_string(),
                    auth_uri: "https://accounts.google.com/o/oauth2/auth".to_string(),
                    token_uri: "https://oauth2.googleapis.com/token".to_string(),
                    auth_provider_x509_cert_url: "https://www.googleapis.com/oauth2/v1/certs".to_string(),
                    client_x509_cert_url: "https://www.googleapis.com/robot/v1/metadata/x509/solana-signer%40forward-emitter-459820-r7.iam.gserviceaccount.com".to_string(),
                    universe_domain: "googleapis.com".to_string(),
                },
                key: GoogleCloudKmsSignerKeyConfig {
                    key_ring_id: "solana-test".to_string(),
                    key_id: "eth".to_string(),
                    key_version: 1,
                },
            }
        })
        .unwrap();
        let result = service.get_evm_address().await;

        let test = result.unwrap();

        println!("Public Key: {}", test);

        // assert!(result.is_err());
    }
}
