//! HTTP client for OpenZeppelin Relayer API
//!
//! Provides a simple HTTP client for integration tests to interact with the relayer API.
//! This client handles authentication, request/response serialization, and basic error handling.

use eyre::{Context, Result};
use openzeppelin_relayer::models::{
    relayer::{CreateRelayerRequest, RelayerResponse},
    signer::SignerResponse,
    ApiResponse, NotificationCreateRequest, NotificationResponse, NotificationUpdateRequest,
    PaginationMeta, PluginModel,
};
use reqwest::Client as HttpClient;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::env;

fn strict_e2e_enabled() -> bool {
    env::var("STRICT_E2E")
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

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
        let client = HttpClient::builder()
            .no_proxy()
            .build()
            .wrap_err("Failed to build HTTP client")?;

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            client,
        })
    }

    /// Builds a client and checks whether the relayer API is reachable.
    ///
    /// Returns `None` when integration prerequisites are not available in the
    /// current environment (missing env vars or unreachable relayer).
    pub async fn from_env_or_skip() -> Option<Self> {
        let strict_mode = strict_e2e_enabled();
        let client = match Self::from_env() {
            Ok(client) => client,
            Err(error) => {
                if strict_mode {
                    panic!("STRICT_E2E enabled: {error}");
                }
                eprintln!("Skipping integration test: {error}");
                return None;
            }
        };

        let url = format!("{}/api/v1/health", client.base_url);
        match client
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", client.api_key))
            .send()
            .await
        {
            Ok(_) => Some(client),
            Err(error) => {
                if error.is_connect() || error.is_timeout() {
                    if strict_mode {
                        panic!(
                            "STRICT_E2E enabled: relayer is unreachable at {}: {}",
                            url, error
                        );
                    }
                    eprintln!(
                        "Skipping integration test: relayer is unreachable at {}",
                        url
                    );
                    return None;
                }
                panic!("Failed to reach relayer at {}: {}", url, error);
            }
        }
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

    /// Checks the readiness state of the relayer service.
    ///
    /// GET /api/v1/ready
    ///
    /// This endpoint intentionally returns either:
    /// - 200 when `ready=true`
    /// - 503 when `ready=false`
    pub async fn readiness(&self) -> Result<ReadinessCheckResponse> {
        let url = format!("{}/api/v1/ready", self.base_url);

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .wrap_err_with(|| format!("Failed to send request to {}", url))?;

        let http_status = response.status();
        let body = response
            .text()
            .await
            .wrap_err("Failed to read response body")?;

        if http_status != StatusCode::OK && http_status != StatusCode::SERVICE_UNAVAILABLE {
            return Err(eyre::eyre!(
                "Readiness check failed with unexpected status {}: {}",
                http_status,
                body
            ));
        }

        let readiness: ReadinessResponse = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse readiness response: {}", body))?;

        Ok(ReadinessCheckResponse {
            http_status: http_status.as_u16(),
            readiness,
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

    /// Updates a relayer using JSON Merge Patch (RFC 7396)
    ///
    /// PATCH /api/v1/relayers/{id}
    pub async fn update_relayer(
        &self,
        relayer_id: &str,
        patch: serde_json::Value,
    ) -> Result<RelayerResponse> {
        let url = format!("{}/api/v1/relayers/{}", self.base_url, relayer_id);

        let response = self
            .client
            .patch(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&patch)
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

    /// Lists all signers with pagination support
    ///
    /// GET /api/v1/signers
    pub async fn list_signers(
        &self,
        page: Option<usize>,
        per_page: Option<usize>,
    ) -> Result<Vec<SignerResponse>> {
        let mut url = format!("{}/api/v1/signers", self.base_url);
        let mut query_params = Vec::new();
        if let Some(p) = page {
            query_params.push(format!("page={}", p));
        }
        if let Some(pp) = per_page {
            query_params.push(format!("per_page={}", pp));
        }
        if !query_params.is_empty() {
            url.push('?');
            url.push_str(&query_params.join("&"));
        }

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

        let api_response: ApiResponse<Vec<SignerResponse>> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;

        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    /// Gets a signer by ID
    ///
    /// GET /api/v1/signers/{id}
    pub async fn get_signer(&self, signer_id: &str) -> Result<SignerResponse> {
        let url = format!("{}/api/v1/signers/{}", self.base_url, signer_id);

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

        let api_response: ApiResponse<SignerResponse> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;

        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    /// Creates a new signer
    ///
    /// POST /api/v1/signers
    pub async fn create_signer(&self, request: serde_json::Value) -> Result<SignerResponse> {
        let url = format!("{}/api/v1/signers", self.base_url);

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
        let api_response: ApiResponse<SignerResponse> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;

        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    /// Deletes a signer by ID
    ///
    /// DELETE /api/v1/signers/{id}
    pub async fn delete_signer(&self, signer_id: &str) -> Result<()> {
        let url = format!("{}/api/v1/signers/{}", self.base_url, signer_id);

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

    /// Updates a signer (currently always returns 400 - updates not allowed)
    ///
    /// PATCH /api/v1/signers/{id}
    pub async fn update_signer(
        &self,
        signer_id: &str,
        request: serde_json::Value,
    ) -> Result<SignerResponse> {
        let url = format!("{}/api/v1/signers/{}", self.base_url, signer_id);

        let response = self
            .client
            .patch(&url)
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

        let api_response: ApiResponse<SignerResponse> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;

        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    /// Lists all plugins with pagination support
    ///
    /// GET /api/v1/plugins
    pub async fn list_plugins(
        &self,
        page: Option<usize>,
        per_page: Option<usize>,
    ) -> Result<Vec<PluginModel>> {
        let mut url = format!("{}/api/v1/plugins", self.base_url);
        let mut query_params = Vec::new();
        if let Some(p) = page {
            query_params.push(format!("page={}", p));
        }
        if let Some(pp) = per_page {
            query_params.push(format!("per_page={}", pp));
        }
        if !query_params.is_empty() {
            url.push('?');
            url.push_str(&query_params.join("&"));
        }

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

        let api_response: ApiResponse<Vec<PluginModel>> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;

        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    /// Gets a plugin by ID
    ///
    /// GET /api/v1/plugins/{id}
    pub async fn get_plugin(&self, plugin_id: &str) -> Result<PluginModel> {
        let url = format!("{}/api/v1/plugins/{}", self.base_url, plugin_id);

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

        let api_response: ApiResponse<PluginModel> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;

        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    /// Updates a plugin's configuration
    ///
    /// PATCH /api/v1/plugins/{id}
    pub async fn update_plugin(
        &self,
        plugin_id: &str,
        request: serde_json::Value,
    ) -> Result<PluginModel> {
        let url = format!("{}/api/v1/plugins/{}", self.base_url, plugin_id);

        let response = self
            .client
            .patch(&url)
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

        let api_response: ApiResponse<PluginModel> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;

        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    /// Lists all networks with pagination support
    ///
    /// GET /api/v1/networks
    pub async fn list_networks(
        &self,
        page: Option<usize>,
        per_page: Option<usize>,
    ) -> Result<Vec<openzeppelin_relayer::models::NetworkResponse>> {
        let mut url = format!("{}/api/v1/networks", self.base_url);
        let mut query_params = Vec::new();
        if let Some(p) = page {
            query_params.push(format!("page={}", p));
        }
        if let Some(pp) = per_page {
            query_params.push(format!("per_page={}", pp));
        }
        if !query_params.is_empty() {
            url.push('?');
            url.push_str(&query_params.join("&"));
        }

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

        let api_response: ApiResponse<Vec<openzeppelin_relayer::models::NetworkResponse>> =
            serde_json::from_str(&body)
                .wrap_err_with(|| format!("Failed to parse response: {}", body))?;

        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    /// Retrieves details of a specific network by ID
    ///
    /// GET /api/v1/networks/{network_id}
    pub async fn get_network(
        &self,
        network_id: &str,
    ) -> Result<openzeppelin_relayer::models::NetworkResponse> {
        let url = format!("{}/api/v1/networks/{}", self.base_url, network_id);

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

        let api_response: ApiResponse<openzeppelin_relayer::models::NetworkResponse> =
            serde_json::from_str(&body)
                .wrap_err_with(|| format!("Failed to parse response: {}", body))?;

        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    /// Lists all notifications with pagination support
    ///
    /// GET /api/v1/notifications
    pub async fn list_notifications(
        &self,
        page: Option<usize>,
        per_page: Option<usize>,
    ) -> Result<Vec<NotificationResponse>> {
        let mut url = format!("{}/api/v1/notifications", self.base_url);
        let mut query_params = Vec::new();
        if let Some(p) = page {
            query_params.push(format!("page={}", p));
        }
        if let Some(pp) = per_page {
            query_params.push(format!("per_page={}", pp));
        }
        if !query_params.is_empty() {
            url.push('?');
            url.push_str(&query_params.join("&"));
        }

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

        let api_response: ApiResponse<Vec<NotificationResponse>> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;

        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    /// Gets a notification by ID
    ///
    /// GET /api/v1/notifications/{id}
    pub async fn get_notification(&self, notification_id: &str) -> Result<NotificationResponse> {
        let url = format!("{}/api/v1/notifications/{}", self.base_url, notification_id);

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

        let api_response: ApiResponse<NotificationResponse> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;

        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    /// Creates a new notification
    ///
    /// POST /api/v1/notifications
    pub async fn create_notification(
        &self,
        request: NotificationCreateRequest,
    ) -> Result<NotificationResponse> {
        let url = format!("{}/api/v1/notifications", self.base_url);

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

        let api_response: ApiResponse<NotificationResponse> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;

        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    /// Updates a notification
    ///
    /// PATCH /api/v1/notifications/{id}
    pub async fn update_notification(
        &self,
        notification_id: &str,
        request: NotificationUpdateRequest,
    ) -> Result<NotificationResponse> {
        let url = format!("{}/api/v1/notifications/{}", self.base_url, notification_id);

        let response = self
            .client
            .patch(&url)
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

        let api_response: ApiResponse<NotificationResponse> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;

        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    /// Deletes a notification by ID
    ///
    /// DELETE /api/v1/notifications/{id}
    pub async fn delete_notification(&self, notification_id: &str) -> Result<()> {
        let url = format!("{}/api/v1/notifications/{}", self.base_url, notification_id);

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

    /// Updates a network's configuration
    ///
    /// PATCH /api/v1/networks/{network_id}
    pub async fn update_network(
        &self,
        network_id: &str,
        request: openzeppelin_relayer::models::UpdateNetworkRequest,
    ) -> Result<openzeppelin_relayer::models::NetworkResponse> {
        let url = format!("{}/api/v1/networks/{}", self.base_url, network_id);

        let response = self
            .client
            .patch(&url)
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

        let api_response: ApiResponse<openzeppelin_relayer::models::NetworkResponse> =
            serde_json::from_str(&body)
                .wrap_err_with(|| format!("Failed to parse response: {}", body))?;

        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    /// Lists signers and returns pagination metadata.
    pub async fn list_signers_paginated(
        &self,
        page: usize,
        per_page: usize,
    ) -> Result<PaginatedData<SignerResponse>> {
        let url = format!(
            "{}/api/v1/signers?page={}&per_page={}",
            self.base_url, page, per_page
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

        let api_response: ApiResponse<Vec<SignerResponse>> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;
        Ok(PaginatedData {
            items: api_response
                .data
                .ok_or_else(|| eyre::eyre!("API response missing data field"))?,
            pagination: api_response.pagination,
        })
    }

    /// Lists networks and returns pagination metadata.
    pub async fn list_networks_paginated(
        &self,
        page: usize,
        per_page: usize,
    ) -> Result<PaginatedData<openzeppelin_relayer::models::NetworkResponse>> {
        let url = format!(
            "{}/api/v1/networks?page={}&per_page={}",
            self.base_url, page, per_page
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

        let api_response: ApiResponse<Vec<openzeppelin_relayer::models::NetworkResponse>> =
            serde_json::from_str(&body)
                .wrap_err_with(|| format!("Failed to parse response: {}", body))?;
        Ok(PaginatedData {
            items: api_response
                .data
                .ok_or_else(|| eyre::eyre!("API response missing data field"))?,
            pagination: api_response.pagination,
        })
    }

    /// Generic helper for relayer sub-resource endpoints that return ApiResponse JSON.
    async fn get_relayer_json(&self, path: &str) -> Result<serde_json::Value> {
        let url = format!("{}/api/v1{}", self.base_url, path);
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
        let api_response: ApiResponse<serde_json::Value> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;
        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    pub async fn get_relayer_status(&self, relayer_id: &str) -> Result<serde_json::Value> {
        self.get_relayer_json(&format!("/relayers/{}/status", relayer_id))
            .await
    }

    pub async fn get_relayer_balance(&self, relayer_id: &str) -> Result<serde_json::Value> {
        self.get_relayer_json(&format!("/relayers/{}/balance", relayer_id))
            .await
    }

    pub async fn list_relayer_transactions(
        &self,
        relayer_id: &str,
        page: usize,
        per_page: usize,
    ) -> Result<serde_json::Value> {
        self.get_relayer_json(&format!(
            "/relayers/{}/transactions?page={}&per_page={}",
            relayer_id, page, per_page
        ))
        .await
    }

    pub async fn get_transaction_by_nonce(
        &self,
        relayer_id: &str,
        nonce: u64,
    ) -> Result<serde_json::Value> {
        self.get_relayer_json(&format!(
            "/relayers/{}/transactions/by-nonce/{}",
            relayer_id, nonce
        ))
        .await
    }

    pub async fn delete_pending_transactions(&self, relayer_id: &str) -> Result<()> {
        let url = format!(
            "{}/api/v1/relayers/{}/transactions/pending",
            self.base_url, relayer_id
        );
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

    pub async fn cancel_transaction(&self, relayer_id: &str, tx_id: &str) -> Result<()> {
        let url = format!(
            "{}/api/v1/relayers/{}/transactions/{}",
            self.base_url, relayer_id, tx_id
        );
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

    pub async fn replace_transaction(
        &self,
        relayer_id: &str,
        tx_id: &str,
        request: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let url = format!(
            "{}/api/v1/relayers/{}/transactions/{}",
            self.base_url, relayer_id, tx_id
        );
        let response = self
            .client
            .put(&url)
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
        let api_response: ApiResponse<serde_json::Value> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;
        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    pub async fn relayer_rpc(
        &self,
        relayer_id: &str,
        request: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let url = format!("{}/api/v1/relayers/{}/rpc", self.base_url, relayer_id);
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
        let api_response: ApiResponse<serde_json::Value> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;
        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    pub async fn sign_data(
        &self,
        relayer_id: &str,
        request: serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.post_relayer_json(relayer_id, "/sign", request).await
    }

    pub async fn sign_typed_data(
        &self,
        relayer_id: &str,
        request: serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.post_relayer_json(relayer_id, "/sign-typed-data", request)
            .await
    }

    pub async fn sign_transaction_payload(
        &self,
        relayer_id: &str,
        request: serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.post_relayer_json(relayer_id, "/sign-transaction", request)
            .await
    }

    pub async fn quote_sponsored_transaction(
        &self,
        relayer_id: &str,
        request: serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.post_relayer_json(relayer_id, "/transactions/sponsored/quote", request)
            .await
    }

    pub async fn build_sponsored_transaction(
        &self,
        relayer_id: &str,
        request: serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.post_relayer_json(relayer_id, "/transactions/sponsored/build", request)
            .await
    }

    async fn post_relayer_json(
        &self,
        relayer_id: &str,
        suffix: &str,
        request: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let url = format!("{}/api/v1/relayers/{}{}", self.base_url, relayer_id, suffix);
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
        let api_response: ApiResponse<serde_json::Value> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;
        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    pub async fn create_api_key(&self, request: serde_json::Value) -> Result<serde_json::Value> {
        let url = format!("{}/api/v1/api-keys", self.base_url);
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
        let api_response: ApiResponse<serde_json::Value> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;
        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    pub async fn list_api_keys_paginated(
        &self,
        page: usize,
        per_page: usize,
    ) -> Result<PaginatedData<serde_json::Value>> {
        let url = format!(
            "{}/api/v1/api-keys?page={}&per_page={}",
            self.base_url, page, per_page
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
        let api_response: ApiResponse<Vec<serde_json::Value>> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;
        Ok(PaginatedData {
            items: api_response
                .data
                .ok_or_else(|| eyre::eyre!("API response missing data field"))?,
            pagination: api_response.pagination,
        })
    }

    pub async fn get_api_key_permissions(&self, api_key_id: &str) -> Result<Vec<String>> {
        let url = format!(
            "{}/api/v1/api-keys/{}/permissions",
            self.base_url, api_key_id
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
        let api_response: ApiResponse<Vec<String>> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Failed to parse response: {}", body))?;
        api_response
            .data
            .ok_or_else(|| eyre::eyre!("API response missing data field"))
    }

    pub async fn delete_api_key_raw(&self, api_key_id: &str) -> Result<(u16, String)> {
        let url = format!("{}/api/v1/api-keys/{}", self.base_url, api_key_id);
        let response = self
            .client
            .delete(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .wrap_err_with(|| format!("Failed to send request to {}", url))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .wrap_err("Failed to read response body")?;
        Ok((status, body))
    }

    pub async fn call_plugin_post_raw(
        &self,
        plugin_id: &str,
        route: &str,
        request: serde_json::Value,
    ) -> Result<(u16, String)> {
        let url = format!(
            "{}/api/v1/plugins/{}/call{}",
            self.base_url, plugin_id, route
        );
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await
            .wrap_err_with(|| format!("Failed to send request to {}", url))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .wrap_err("Failed to read response body")?;
        Ok((status, body))
    }

    pub async fn call_plugin_get_raw(&self, plugin_id: &str, route: &str) -> Result<(u16, String)> {
        let url = format!(
            "{}/api/v1/plugins/{}/call{}",
            self.base_url, plugin_id, route
        );
        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .wrap_err_with(|| format!("Failed to send request to {}", url))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .wrap_err("Failed to read response body")?;
        Ok((status, body))
    }
}

// ============================================================================
// Request/Response Models
// ============================================================================

/// Health check response (not available in src, specific to health endpoint)
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HealthResponse {
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PaginatedData<T> {
    pub items: Vec<T>,
    pub pagination: Option<PaginationMeta>,
}

/// Response body from `/api/v1/ready` endpoint.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReadinessResponse {
    pub ready: bool,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub components: serde_json::Value,
    pub timestamp: String,
}

/// Full readiness check response including HTTP status and parsed JSON body.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReadinessCheckResponse {
    pub http_status: u16,
    pub readiness: ReadinessResponse,
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
