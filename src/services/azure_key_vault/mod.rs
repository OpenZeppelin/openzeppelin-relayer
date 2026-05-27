//! Azure Key Vault service for EVM secp256k1 signing.

use alloy::primitives::keccak256;
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use k256::ecdsa::Signature;
use reqwest::Client;
use serde_json::Value;
use std::{env, time::Duration};
use tokio::fs;

#[cfg(test)]
use mockall::automock;

use crate::{
    models::{Address, AzureKeyVaultAuthType, AzureKeyVaultSignerConfig},
    utils::{recover_public_key, recover_public_key_from_hash, Secp256k1Error},
};

const AZURE_API_VERSION: &str = "7.4";
const AZURE_SCOPE: &str = "https://vault.azure.net/.default";
const AZURE_MANAGED_IDENTITY_RESOURCE: &str = "https://vault.azure.net";
const AZURE_SIGN_ALGORITHM: &str = "ES256K";
const AZURE_IMDS_API_VERSION: &str = "2018-02-01";
const AZURE_IMDS_TOKEN_URL: &str = "http://169.254.169.254/metadata/identity/oauth2/token";
const AZURE_HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const AZURE_HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const AZURE_HTTP_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, thiserror::Error, serde::Serialize)]
pub enum AzureKeyVaultError {
    #[error("Azure Key Vault HTTP error: {0}")]
    HttpError(String),
    #[error("Azure Key Vault API error: {0}")]
    ApiError(String),
    #[error("Azure Key Vault response parse error: {0}")]
    ParseError(String),
    #[error("Azure Key Vault missing field: {0}")]
    MissingField(String),
    #[error("Azure Key Vault recovery error: {0}")]
    RecoveryError(#[from] Secp256k1Error),
}

pub type AzureKeyVaultResult<T> = Result<T, AzureKeyVaultError>;

#[async_trait]
#[cfg_attr(test, automock)]
pub trait AzureKeyVaultEvmService: Send + Sync {
    async fn get_evm_address(&self) -> AzureKeyVaultResult<Address>;
    async fn sign_payload_evm(&self, payload: &[u8]) -> AzureKeyVaultResult<Vec<u8>>;
    async fn sign_hash_evm(&self, hash: &[u8; 32]) -> AzureKeyVaultResult<Vec<u8>>;
}

#[derive(Clone, Debug)]
pub struct AzureKeyVaultService {
    pub config: AzureKeyVaultSignerConfig,
    client: Client,
}

impl AzureKeyVaultService {
    pub fn new(config: &AzureKeyVaultSignerConfig) -> AzureKeyVaultResult<Self> {
        Ok(Self {
            config: config.clone(),
            client: Self::build_http_client()?,
        })
    }

    fn build_http_client() -> AzureKeyVaultResult<Client> {
        Client::builder()
            .connect_timeout(AZURE_HTTP_CONNECT_TIMEOUT)
            .timeout(AZURE_HTTP_REQUEST_TIMEOUT)
            .pool_idle_timeout(AZURE_HTTP_POOL_IDLE_TIMEOUT)
            .build()
            .map_err(|e| AzureKeyVaultError::HttpError(e.to_string()))
    }

    fn tenant_id(&self) -> String {
        self.config
            .tenant_id
            .as_ref()
            .map(|value| value.to_str().to_string())
            .unwrap_or_default()
    }

    fn client_id(&self) -> String {
        self.config
            .client_id
            .as_ref()
            .map(|value| value.to_str().to_string())
            .unwrap_or_default()
    }

    fn client_secret(&self) -> String {
        self.config
            .client_secret
            .as_ref()
            .map(|value| value.to_str().to_string())
            .unwrap_or_default()
    }

    fn federated_token_file(&self) -> Option<String> {
        self.config
            .federated_token_file
            .as_ref()
            .map(|value| value.to_str().to_string())
            .or_else(|| env::var("AZURE_FEDERATED_TOKEN_FILE").ok())
    }

    fn vault_url(&self) -> String {
        self.config
            .vault_url
            .to_str()
            .trim_end_matches('/')
            .to_string()
    }

    fn key_name(&self) -> String {
        self.config.key_name.to_str().to_string()
    }

    fn oauth_token_url(&self) -> String {
        let tenant_id = self.tenant_id();
        if tenant_id.starts_with("http://") || tenant_id.starts_with("https://") {
            format!("{}/oauth2/v2.0/token", tenant_id.trim_end_matches('/'))
        } else {
            format!(
                "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
                tenant_id
            )
        }
    }

    fn key_path(&self) -> String {
        match self.config.key_version.as_deref() {
            Some(version) if !version.is_empty() => format!("keys/{}/{}", self.key_name(), version),
            _ => format!("keys/{}", self.key_name()),
        }
    }

    fn key_url(&self) -> String {
        format!(
            "{}/{}?api-version={}",
            self.vault_url(),
            self.key_path(),
            AZURE_API_VERSION
        )
    }

    fn sign_url(&self) -> String {
        format!(
            "{}/{}/sign?api-version={}",
            self.vault_url(),
            self.key_path(),
            AZURE_API_VERSION
        )
    }

    fn auth_type(&self) -> AzureKeyVaultAuthType {
        self.config.auth_type()
    }

    fn managed_identity_token_url(&self) -> String {
        env::var("AZURE_IMDS_TOKEN_URL").unwrap_or_else(|_| AZURE_IMDS_TOKEN_URL.to_string())
    }

    async fn get_client_secret_access_token(&self) -> AzureKeyVaultResult<String> {
        let client_id = self.client_id();
        let client_secret = self.client_secret();
        let url = self.oauth_token_url();

        let response = self
            .client
            .post(url)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", client_id.as_str()),
                ("client_secret", client_secret.as_str()),
                ("scope", AZURE_SCOPE),
            ])
            .send()
            .await
            .map_err(|e| AzureKeyVaultError::HttpError(e.to_string()))?;

        Self::parse_access_token_response(response).await
    }

    async fn get_managed_identity_access_token(&self) -> AzureKeyVaultResult<String> {
        let request = self
            .client
            .get(self.managed_identity_token_url())
            .header("Metadata", "true");
        let client_id = self.client_id();

        let mut query = vec![
            ("api-version", AZURE_IMDS_API_VERSION),
            ("resource", AZURE_MANAGED_IDENTITY_RESOURCE),
        ];
        if !client_id.is_empty() {
            query.push(("client_id", client_id.as_str()));
        }

        let response = request
            .query(&query)
            .send()
            .await
            .map_err(|e| AzureKeyVaultError::HttpError(e.to_string()))?;

        Self::parse_access_token_response(response).await
    }

    async fn get_workload_identity_access_token(&self) -> AzureKeyVaultResult<String> {
        let client_id = self.client_id();
        let url = self.oauth_token_url();
        let token_file = self
            .federated_token_file()
            .ok_or_else(|| AzureKeyVaultError::MissingField("federated_token_file".to_string()))?;
        let federated_token = fs::read_to_string(&token_file).await.map_err(|e| {
            AzureKeyVaultError::HttpError(format!(
                "failed to read federated token file {token_file}: {e}"
            ))
        })?;

        let response = self
            .client
            .post(url)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", client_id.as_str()),
                (
                    "client_assertion_type",
                    "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
                ),
                ("client_assertion", federated_token.trim()),
                ("scope", AZURE_SCOPE),
            ])
            .send()
            .await
            .map_err(|e| AzureKeyVaultError::HttpError(e.to_string()))?;

        Self::parse_access_token_response(response).await
    }

    async fn parse_access_token_response(
        response: reqwest::Response,
    ) -> AzureKeyVaultResult<String> {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(AzureKeyVaultError::ApiError(format!(
                "token request failed ({status}): {text}"
            )));
        }

        let body: Value = serde_json::from_str(&text)
            .map_err(|e| AzureKeyVaultError::ParseError(format!("{e}: {text}")))?;

        body.get("access_token")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| AzureKeyVaultError::MissingField("access_token".to_string()))
    }

    async fn get_access_token(&self) -> AzureKeyVaultResult<String> {
        match self.auth_type() {
            AzureKeyVaultAuthType::ClientSecret => self.get_client_secret_access_token().await,
            AzureKeyVaultAuthType::ManagedIdentity => {
                self.get_managed_identity_access_token().await
            }
            AzureKeyVaultAuthType::WorkloadIdentity => {
                self.get_workload_identity_access_token().await
            }
        }
    }

    async fn key_vault_get(&self, url: &str) -> AzureKeyVaultResult<Value> {
        let token = self.get_access_token().await?;
        let response = self
            .client
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| AzureKeyVaultError::HttpError(e.to_string()))?;

        let status = response.status();
        let text = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(AzureKeyVaultError::ApiError(format!(
                "key vault request failed ({status}): {text}"
            )));
        }

        serde_json::from_str(&text)
            .map_err(|e| AzureKeyVaultError::ParseError(format!("{e}: {text}")))
    }

    async fn key_vault_post(&self, url: &str, body: &Value) -> AzureKeyVaultResult<Value> {
        let token = self.get_access_token().await?;
        let response = self
            .client
            .post(url)
            .bearer_auth(token)
            .json(body)
            .send()
            .await
            .map_err(|e| AzureKeyVaultError::HttpError(e.to_string()))?;

        let status = response.status();
        let text = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(AzureKeyVaultError::ApiError(format!(
                "key vault request failed ({status}): {text}"
            )));
        }

        serde_json::from_str(&text)
            .map_err(|e| AzureKeyVaultError::ParseError(format!("{e}: {text}")))
    }

    async fn get_public_key(&self) -> AzureKeyVaultResult<[u8; 64]> {
        let body = self.key_vault_get(&self.key_url()).await?;
        let key = body
            .get("key")
            .ok_or_else(|| AzureKeyVaultError::MissingField("key".to_string()))?;

        let x = key
            .get("x")
            .and_then(Value::as_str)
            .ok_or_else(|| AzureKeyVaultError::MissingField("key.x".to_string()))?;
        let y = key
            .get("y")
            .and_then(Value::as_str)
            .ok_or_else(|| AzureKeyVaultError::MissingField("key.y".to_string()))?;

        let x_bytes = URL_SAFE_NO_PAD
            .decode(x)
            .map_err(|e| AzureKeyVaultError::ParseError(e.to_string()))?;
        let y_bytes = URL_SAFE_NO_PAD
            .decode(y)
            .map_err(|e| AzureKeyVaultError::ParseError(e.to_string()))?;

        if x_bytes.len() != 32 || y_bytes.len() != 32 {
            return Err(AzureKeyVaultError::ParseError(format!(
                "expected 32-byte secp256k1 coordinates, got x={}, y={}",
                x_bytes.len(),
                y_bytes.len()
            )));
        }

        let mut public_key = [0u8; 64];
        public_key[..32].copy_from_slice(&x_bytes);
        public_key[32..].copy_from_slice(&y_bytes);

        Ok(public_key)
    }

    async fn sign_digest(&self, digest: [u8; 32]) -> AzureKeyVaultResult<Vec<u8>> {
        let body = serde_json::json!({
            "alg": AZURE_SIGN_ALGORITHM,
            "value": URL_SAFE_NO_PAD.encode(digest),
        });

        let response = self.key_vault_post(&self.sign_url(), &body).await?;
        let signature = response
            .get("value")
            .and_then(Value::as_str)
            .ok_or_else(|| AzureKeyVaultError::MissingField("value".to_string()))?;

        URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|e| AzureKeyVaultError::ParseError(e.to_string()))
    }

    async fn sign_and_recover_evm(
        &self,
        digest: [u8; 32],
        original_bytes: &[u8],
        use_prehash_recovery: bool,
    ) -> AzureKeyVaultResult<Vec<u8>> {
        let raw_signature = self.sign_digest(digest).await?;
        if raw_signature.len() != 64 {
            return Err(AzureKeyVaultError::ParseError(format!(
                "expected 64-byte ES256K signature, got {} bytes",
                raw_signature.len()
            )));
        }

        let mut rs = Signature::from_slice(&raw_signature)
            .map_err(|e| AzureKeyVaultError::ParseError(e.to_string()))?;

        if let Some(normalized) = rs.normalize_s() {
            rs = normalized;
        }

        let public_key = self.get_public_key().await?;
        let recovery_id = if use_prehash_recovery {
            recover_public_key_from_hash(&public_key, &rs, &digest)?
        } else {
            recover_public_key(&public_key, &rs, original_bytes)?
        };

        let mut signature = rs.to_vec();
        signature.push(27 + recovery_id);
        Ok(signature)
    }
}

#[async_trait]
impl AzureKeyVaultEvmService for AzureKeyVaultService {
    async fn get_evm_address(&self) -> AzureKeyVaultResult<Address> {
        let public_key = self.get_public_key().await?;
        let hash = keccak256(public_key);
        let mut address = [0u8; 20];
        address.copy_from_slice(&hash[12..]);
        Ok(Address::Evm(address))
    }

    async fn sign_payload_evm(&self, payload: &[u8]) -> AzureKeyVaultResult<Vec<u8>> {
        let digest = keccak256(payload).0;
        self.sign_and_recover_evm(digest, payload, false).await
    }

    async fn sign_hash_evm(&self, hash: &[u8; 32]) -> AzureKeyVaultResult<Vec<u8>> {
        self.sign_and_recover_evm(*hash, hash, true).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SecretString;
    use alloy::primitives::utils::eip191_message;
    use k256::{
        ecdsa::{signature::hazmat::PrehashSigner, SigningKey},
        elliptic_curve::rand_core::OsRng,
    };
    use mockito::Server;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn test_config(base_url: &str) -> AzureKeyVaultSignerConfig {
        AzureKeyVaultSignerConfig {
            auth_type: Some(AzureKeyVaultAuthType::ClientSecret),
            tenant_id: Some(SecretString::new(base_url)),
            client_id: Some(SecretString::new("test-client")),
            client_secret: Some(SecretString::new("test-secret")),
            federated_token_file: None,
            vault_url: SecretString::new(base_url),
            key_name: SecretString::new("test-key"),
            key_version: Some("test-version".to_string()),
        }
    }

    fn managed_identity_config(base_url: &str) -> AzureKeyVaultSignerConfig {
        AzureKeyVaultSignerConfig {
            auth_type: Some(AzureKeyVaultAuthType::ManagedIdentity),
            tenant_id: None,
            client_id: Some(SecretString::new("managed-client-id")),
            client_secret: None,
            federated_token_file: None,
            vault_url: SecretString::new(base_url),
            key_name: SecretString::new("test-key"),
            key_version: Some("test-version".to_string()),
        }
    }

    #[tokio::test]
    async fn test_get_evm_address() {
        let mut server = Server::new_async().await;
        let signing_key = SigningKey::random(&mut OsRng);
        let point = signing_key.verifying_key().to_encoded_point(false);
        let x = URL_SAFE_NO_PAD.encode(point.x().unwrap());
        let y = URL_SAFE_NO_PAD.encode(point.y().unwrap());

        let _token = server
            .mock("POST", "/oauth2/v2.0/token")
            .match_body(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"test-token"}"#)
            .expect(1)
            .create_async()
            .await;

        let _key = server
            .mock("GET", "/keys/test-key/test-version")
            .match_query(mockito::Matcher::UrlEncoded(
                "api-version".into(),
                AZURE_API_VERSION.into(),
            ))
            .match_header("authorization", "Bearer test-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "key": {
                        "x": x,
                        "y": y,
                    }
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;

        let service = AzureKeyVaultService::new(&test_config(&server.url())).unwrap();
        let address = service.get_evm_address().await.unwrap();
        assert!(matches!(address, Address::Evm(_)));
    }

    #[tokio::test]
    async fn test_sign_payload_evm() {
        let mut server = Server::new_async().await;
        let signing_key = SigningKey::random(&mut OsRng);
        let point = signing_key.verifying_key().to_encoded_point(false);
        let x = URL_SAFE_NO_PAD.encode(point.x().unwrap());
        let y = URL_SAFE_NO_PAD.encode(point.y().unwrap());

        let message = eip191_message(b"hello azure");
        let digest = keccak256(&message).0;
        let signature: Signature = signing_key.sign_prehash(&digest).unwrap();
        let raw_signature = signature.to_bytes();

        let _token_1 = server
            .mock("POST", "/oauth2/v2.0/token")
            .match_body(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"test-token"}"#)
            .expect(1)
            .create_async()
            .await;

        let _sign = server
            .mock("POST", "/keys/test-key/test-version/sign")
            .match_query(mockito::Matcher::UrlEncoded(
                "api-version".into(),
                AZURE_API_VERSION.into(),
            ))
            .match_header("authorization", "Bearer test-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "value": URL_SAFE_NO_PAD.encode(raw_signature),
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;

        let _token_2 = server
            .mock("POST", "/oauth2/v2.0/token")
            .match_body(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"test-token"}"#)
            .expect(1)
            .create_async()
            .await;

        let _key = server
            .mock("GET", "/keys/test-key/test-version")
            .match_query(mockito::Matcher::UrlEncoded(
                "api-version".into(),
                AZURE_API_VERSION.into(),
            ))
            .match_header("authorization", "Bearer test-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "key": {
                        "x": x,
                        "y": y,
                    }
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;

        let service = AzureKeyVaultService::new(&test_config(&server.url())).unwrap();
        let signature = service.sign_payload_evm(&message).await.unwrap();

        assert_eq!(signature.len(), 65);
        assert!(signature[64] == 27 || signature[64] == 28);
    }

    #[tokio::test]
    async fn test_managed_identity_access_token() {
        let mut server = Server::new_async().await;
        unsafe {
            env::set_var(
                "AZURE_IMDS_TOKEN_URL",
                format!("{}/metadata/identity/oauth2/token", server.url()),
            );
        }
        let _token = server
            .mock("GET", "/metadata/identity/oauth2/token")
            .match_header("metadata", "true")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("api-version".into(), AZURE_IMDS_API_VERSION.into()),
                mockito::Matcher::UrlEncoded(
                    "resource".into(),
                    AZURE_MANAGED_IDENTITY_RESOURCE.into(),
                ),
                mockito::Matcher::UrlEncoded("client_id".into(), "managed-client-id".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"managed-token"}"#)
            .expect(1)
            .create_async()
            .await;

        let service = AzureKeyVaultService {
            config: managed_identity_config(&server.url()),
            client: AzureKeyVaultService::build_http_client().unwrap(),
        };

        let token = service.get_access_token().await.unwrap();
        assert_eq!(token, "managed-token");

        unsafe {
            env::remove_var("AZURE_IMDS_TOKEN_URL");
        }
    }

    #[tokio::test]
    async fn test_workload_identity_access_token() {
        let mut server = Server::new_async().await;
        let mut token_file = NamedTempFile::new().unwrap();
        writeln!(token_file, "federated-jwt").unwrap();

        let _token = server
            .mock("POST", "/oauth2/v2.0/token")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("grant_type".into(), "client_credentials".into()),
                mockito::Matcher::UrlEncoded("client_id".into(), "workload-client-id".into()),
                mockito::Matcher::UrlEncoded(
                    "client_assertion_type".into(),
                    "urn:ietf:params:oauth:client-assertion-type:jwt-bearer".into(),
                ),
                mockito::Matcher::UrlEncoded("client_assertion".into(), "federated-jwt".into()),
                mockito::Matcher::UrlEncoded("scope".into(), AZURE_SCOPE.into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"workload-token"}"#)
            .expect(1)
            .create_async()
            .await;

        let config = AzureKeyVaultSignerConfig {
            auth_type: Some(AzureKeyVaultAuthType::WorkloadIdentity),
            tenant_id: Some(SecretString::new(&server.url())),
            client_id: Some(SecretString::new("workload-client-id")),
            client_secret: None,
            federated_token_file: Some(SecretString::new(
                token_file.path().to_string_lossy().as_ref(),
            )),
            vault_url: SecretString::new(&server.url()),
            key_name: SecretString::new("test-key"),
            key_version: Some("test-version".to_string()),
        };

        let service = AzureKeyVaultService::new(&config).unwrap();
        let token = service.get_access_token().await.unwrap();
        assert_eq!(token, "workload-token");
    }
}
