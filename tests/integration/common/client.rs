//! HTTP client for OpenZeppelin Relayer API
//!
//! Provides a simple HTTP client for integration tests to interact with the relayer API.
//! This client handles authentication, request/response serialization, and basic error handling.

use eyre::{Context, Result};
use openzeppelin_relayer::models::{relayer::RelayerResponse, ApiResponse};
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use std::env;

/// HTTP client for OpenZeppelin Relayer API
#[derive(Debug)]
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
    /// - `API_KEY`: API key for authentication
    ///
    /// # Errors
    ///
    /// Returns an error if environment variables are not set or if the HTTP client fails to initialize
    pub fn from_env() -> Result<Self> {
        let base_url = env::var("RELAYER_BASE_URL").unwrap_or("http://localhost:8080".to_string());
        let api_key = env::var("API_KEY").wrap_err("API_KEY environment variable not set")?;

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            client: HttpClient::new(),
        })
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
}

// ============================================================================
// Request/Response Models
// ============================================================================

// Note: RelayerResponse is imported from openzeppelin_relayer::models::relayer
// to avoid duplication

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
