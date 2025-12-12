//! HTTP client for OpenZeppelin Relayer API
//!
//! Provides a simple HTTP client for integration tests to interact with the relayer API.
//! This client handles authentication, request/response serialization, and basic error handling.

use eyre::{Context, Result};
use openzeppelin_relayer::models::{
    relayer::{CreateRelayerRequest, RelayerResponse},
    ApiResponse,
};
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use std::env;
use tracing::{debug, error, info};
use uuid::Uuid;

/// HTTP client for OpenZeppelin Relayer API
#[derive(Debug)]
pub struct RelayerClient {
    base_url: String,
    api_key: String,
    client: HttpClient,
}

/// Generates a deterministic relayer ID from network and signer using UUID v5
///
/// Uses UUID v5 with OID namespace to generate a deterministic UUID from
/// the network and signer_id combination. This ensures:
/// - Same network+signer always generates the same UUID (deterministic)
/// - ID is exactly 36 characters (meets validation requirement)
/// - Different network+signer combinations generate different UUIDs
///
/// Format: UUID v5 based on "{network}:{signer_id}"
fn generate_relayer_id(network: &str, signer_id: &str) -> String {
    let name = format!("{}:{}", network, signer_id);
    Uuid::new_v5(&Uuid::NAMESPACE_OID, name.as_bytes()).to_string()
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

    /// Lists all relayers
    ///
    /// GET /api/v1/relayers
    pub async fn list_relayers(&self) -> Result<Vec<RelayerResponse>> {
        let url = format!("{}/api/v1/relayers", self.base_url);

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

        let api_response: ApiResponse<Vec<RelayerResponse>> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;

        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    /// Deletes all relayers for a specific network
    ///
    /// This is a convenience method that lists all relayers and deletes those matching the network
    pub async fn delete_all_relayers_by_network(&self, network: &str) -> Result<usize> {
        let relayers = self.list_relayers().await?;

        let network_relayers: Vec<_> = relayers.iter().filter(|r| r.network == network).collect();

        let count = network_relayers.len();

        for relayer in network_relayers {
            if let Err(e) = self.delete_relayer(&relayer.id).await {
                error!(relayer_id = %relayer.id, error = %e, "Failed to delete relayer");
            }
        }

        Ok(count)
    }
}

// ============================================================================
// Request/Response Models
// ============================================================================

// Note: CreateRelayerRequest and RelayerResponse are imported from
// openzeppelin_relayer::models::relayer to avoid duplication

/// Health check response (not available in src, specific to health endpoint)
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HealthResponse {
    pub status: String,
}

/// Transaction response from API (simplified version for integration tests)
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
