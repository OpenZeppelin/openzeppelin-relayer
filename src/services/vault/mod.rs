// src/services/vault_service.rs

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum VaultError {
    #[error("HTTP request failed: {0}")]
    HttpRequestError(#[from] reqwest::Error),
    #[error("Secret not found")]
    SecretNotFound,
    #[error("Authentication failed")]
    AuthenticationFailed,
}

#[cfg(test)]
use mockall::automock;

#[derive(Clone)]
pub struct VaultConfig {
    pub address: String,
    pub namespace: Option<String>,
    pub role_id: String,
    pub secret_id: String,
}

#[async_trait]
#[cfg_attr(test, automock)]
pub trait VaultServiceTrait: Send + Sync {
    async fn authenticate(&self) -> Result<String, VaultError>;
    async fn retrieve_secret(&self, key_name: &str) -> Result<String, VaultError>;
}

pub struct VaultService {
    pub config: VaultConfig,
    pub client: Client,
}

#[derive(Deserialize)]
struct VaultTokenResponse {
    auth: VaultAuth,
}

#[derive(Deserialize)]
struct VaultAuth {
    client_token: String,
}

#[async_trait]
impl VaultServiceTrait for VaultService {
    async fn authenticate(&self) -> Result<String, VaultError> {
        let url = format!("{}/v1/auth/approle/login", self.config.address);
        let body = serde_json::json!({
            "role_id": self.config.role_id,
            "secret_id": self.config.secret_id,
        });

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;

        let token_response: VaultTokenResponse = response.json().await?;
        Ok(token_response.auth.client_token)
    }

    async fn retrieve_secret(&self, key_name: &str) -> Result<String, VaultError> {
        let token = self.authenticate().await?;
        let url = format!("{}/v1/secret/data/{}", self.config.address, key_name);

        let response = self
            .client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?;

        let secret_data: serde_json::Value = response.json().await?;
        let secret = secret_data["data"]["data"]["value"]
            .as_str()
            .ok_or(VaultError::SecretNotFound)?
            .to_string();

        Ok(secret)
    }
}
