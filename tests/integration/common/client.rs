//! HTTP client for OpenZeppelin Relayer API
//!
//! Provides a simple HTTP client for integration tests to interact with the relayer API.
//! This client handles authentication, request/response serialization, and basic error handling.

use eyre::{Context, Result};
use openzeppelin_relayer::models::{
    relayer::{DisabledReason, RelayerNetworkType, RpcConfig},
    ApiResponse,
};
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use std::env;

/// HTTP client for OpenZeppelin Relayer API
pub struct RelayerClient {
    base_url: String,
    api_key: String,
    client: HttpClient,
}

impl RelayerClient {
    /// Creates a new RelayerClient from environment variables
    ///
    /// # Environment Variables
    ///
    /// - `RELAYER_BASE_URL`: Base URL for the relayer API (e.g., "http://localhost:8080")
    /// - `RELAYER_API_KEY`: API key for authentication
    ///
    /// # Errors
    ///
    /// Returns an error if environment variables are not set or if the HTTP client fails to initialize
    pub fn from_env() -> Result<Self> {
        let base_url = env::var("RELAYER_BASE_URL").unwrap_or("http://localhost:8080".to_string());
        let api_key =
            env::var("RELAYER_API_KEY").wrap_err("RELAYER_API_KEY environment variable not set")?;

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            client: HttpClient::new(),
        })
    }

    /// Checks the health of the relayer service
    ///
    /// GET /api/v1/health
    pub async fn health(&self) -> Result<HealthResponse> {
        let url = format!("{}/api/v1/health", self.base_url);

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .wrap_err_with(|| format!("Failed to send request to {}", url))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .wrap_err("Failed to read response body")?;

        if !status.is_success() {
            return Err(eyre::eyre!(
                "Health check failed with status {}: {}",
                status,
                body
            ));
        }

        // Health endpoint returns plain text "OK"
        Ok(HealthResponse {
            status: body.trim().to_lowercase(),
        })
    }

    /// Creates a new relayer
    ///
    /// POST /api/v1/relayers
    pub async fn create_relayer(&self, request: CreateRelayerRequest) -> Result<RelayerResponse> {
        let url = format!("{}/api/v1/relayers", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await
            .wrap_err_with(|| format!("Failed to send request to {}", url))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .wrap_err("Failed to read response body")?;

        if !status.is_success() {
            return Err(eyre::eyre!(
                "API request failed with status {}: {}",
                status,
                body
            ));
        }

        // Parse response
        let api_response: ApiResponse<RelayerResponse> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;

        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    /// Gets a relayer by ID
    ///
    /// GET /api/v1/relayers/{id}
    pub async fn get_relayer(&self, relayer_id: &str) -> Result<RelayerResponse> {
        let url = format!("{}/api/v1/relayers/{}", self.base_url, relayer_id);

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .wrap_err_with(|| format!("Failed to send request to {}", url))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .wrap_err("Failed to read response body")?;

        if !status.is_success() {
            return Err(eyre::eyre!(
                "API request failed with status {}: {}",
                status,
                body
            ));
        }

        // Parse response
        let api_response: ApiResponse<RelayerResponse> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;

        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    /// Sends a transaction through a relayer
    ///
    /// POST /api/v1/relayers/{id}/transactions
    pub async fn send_transaction(
        &self,
        relayer_id: &str,
        request: serde_json::Value,
    ) -> Result<TransactionResponse> {
        let url = format!(
            "{}/api/v1/relayers/{}/transactions",
            self.base_url, relayer_id
        );

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await
            .wrap_err_with(|| format!("Failed to send request to {}", url))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .wrap_err("Failed to read response body")?;

        if !status.is_success() {
            return Err(eyre::eyre!(
                "API request failed with status {}: {}",
                status,
                body
            ));
        }

        // Parse response
        let api_response: ApiResponse<TransactionResponse> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;

        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    /// Gets a transaction by ID
    ///
    /// GET /api/v1/relayers/{relayer_id}/transactions/{tx_id}
    pub async fn get_transaction(
        &self,
        relayer_id: &str,
        tx_id: &str,
    ) -> Result<TransactionResponse> {
        let url = format!(
            "{}/api/v1/relayers/{}/transactions/{}",
            self.base_url, relayer_id, tx_id
        );

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .wrap_err_with(|| format!("Failed to send request to {}", url))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .wrap_err("Failed to read response body")?;

        if !status.is_success() {
            return Err(eyre::eyre!(
                "API request failed with status {}: {}",
                status,
                body
            ));
        }

        // Parse response
        let api_response: ApiResponse<TransactionResponse> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;

        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    /// Deletes a relayer by ID
    ///
    /// DELETE /api/v1/relayers/{id}
    pub async fn delete_relayer(&self, relayer_id: &str) -> Result<()> {
        let url = format!("{}/api/v1/relayers/{}", self.base_url, relayer_id);

        let response = self
            .client
            .delete(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .wrap_err_with(|| format!("Failed to send request to {}", url))?;

        let status = response.status();

        if !status.is_success() {
            let body = response
                .text()
                .await
                .wrap_err("Failed to read response body")?;
            return Err(eyre::eyre!(
                "API request failed with status {}: {}",
                status,
                body
            ));
        }

        Ok(())
    }
}

// ============================================================================
// Request/Response Models
// ============================================================================

/// Health check response
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HealthResponse {
    pub status: String,
}

/// Request to create a new relayer
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateRelayerRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub network: String,
    pub paused: bool,
    pub network_type: RelayerNetworkType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policies: Option<serde_json::Value>,
    pub signer_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notification_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_rpc_urls: Option<Vec<RpcConfig>>,
}

/// Relayer response from API
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RelayerResponse {
    pub id: String,
    pub name: String,
    pub network: String,
    pub network_type: RelayerNetworkType,
    pub paused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policies: Option<serde_json::Value>,
    pub signer_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notification_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_rpc_urls: Option<Vec<RpcConfig>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_disabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<DisabledReason>,
}

/// Transaction response from API
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TransactionResponse {
    pub id: String,
    pub relayer_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submitted_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmed_at: Option<String>,
}
