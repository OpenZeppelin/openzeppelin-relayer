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
use uuid::Uuid;

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
                eprintln!("Failed to delete relayer {}: {}", relayer.id, e);
            }
        }

        Ok(count)
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

    /// Gets an existing relayer or creates it if it doesn't exist
    ///
    /// This method uses a deterministic UUID v5 based on network and signer_id to enable
    /// reusing relayers across test runs without needing to wait for transaction completion.
    /// The UUID is generated from the combination of network and signer_id, ensuring the
    /// same combination always produces the same relayer ID.
    ///
    /// If a relayer already exists with the same network+signer but a different ID (created
    /// manually or before UUID v5 implementation), this method will find and return it.
    ///
    /// Returns the existing or newly created relayer.
    pub async fn get_or_create_relayer(
        &self,
        mut request: CreateRelayerRequest,
    ) -> Result<RelayerResponse> {
        let network = request.network.clone();
        let signer_id = request.signer_id.clone();

        // Generate deterministic ID
        let relayer_id = Self::generate_relayer_id(&network, &signer_id);
        request.id = Some(relayer_id.clone());

        // Try to get existing relayer by deterministic ID
        match self.get_relayer(&relayer_id).await {
            Ok(relayer) => {
                println!(
                    "Using existing relayer {} for {} with signer {}",
                    relayer_id, network, signer_id
                );
                return Ok(relayer);
            }
            Err(_) => {
                // Relayer with deterministic ID doesn't exist, try to create it
            }
        }

        // Try to create the relayer
        match self.create_relayer(request).await {
            Ok(relayer) => {
                println!(
                    "Created new relayer {} for {} with signer {}",
                    relayer_id, network, signer_id
                );
                Ok(relayer)
            }
            Err(e) => {
                let error_msg = e.to_string();

                // Check if error is due to signer already in use (list and find by network+signer)
                if error_msg.contains("signer") && error_msg.contains("already in use") {
                    println!(
                        "Signer {} already in use on {}, searching for existing relayer...",
                        signer_id, network
                    );

                    // List all relayers and find the one with matching network+signer
                    let relayers = self.list_relayers().await?;

                    if let Some(existing) = relayers
                        .iter()
                        .find(|r| r.network == network && r.signer_id == signer_id)
                    {
                        println!(
                            "Found existing relayer {} for {} with signer {}",
                            existing.id, network, signer_id
                        );
                        return Ok(existing.clone());
                    }
                }

                // Check if error indicates relayer already exists (constraint violation)
                // This can happen if there was a transient error during the GET request
                if error_msg.contains("already exists") || error_msg.contains("Constraint violated")
                {
                    println!("Relayer {} may already exist, retrying GET...", relayer_id);

                    // Retry getting the relayer
                    if let Ok(existing) = self.get_relayer(&relayer_id).await {
                        println!(
                            "Successfully retrieved existing relayer {} for {} with signer {}",
                            relayer_id, network, signer_id
                        );
                        return Ok(existing);
                    }

                    // If still not found, search by network+signer as fallback
                    let relayers = self.list_relayers().await?;
                    if let Some(existing) = relayers
                        .iter()
                        .find(|r| r.network == network && r.signer_id == signer_id)
                    {
                        println!(
                            "Found existing relayer {} via list for {} with signer {}",
                            existing.id, network, signer_id
                        );
                        return Ok(existing.clone());
                    }
                }

                // Re-throw the original error if we couldn't handle it
                Err(e)
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_relayer_id_deterministic() {
        // Same inputs should always generate the same UUID
        let id1 = RelayerClient::generate_relayer_id("sepolia", "test-signer");
        let id2 = RelayerClient::generate_relayer_id("sepolia", "test-signer");
        assert_eq!(id1, id2, "Same network+signer should generate same UUID");

        // Different inputs should generate different UUIDs
        let id3 = RelayerClient::generate_relayer_id("mainnet", "test-signer");
        assert_ne!(
            id1, id3,
            "Different networks should generate different UUIDs"
        );

        let id4 = RelayerClient::generate_relayer_id("sepolia", "other-signer");
        assert_ne!(
            id1, id4,
            "Different signers should generate different UUIDs"
        );
    }

    #[test]
    fn test_generate_relayer_id_format() {
        let id = RelayerClient::generate_relayer_id("sepolia", "test-signer");

        // Should be a valid UUID format (36 characters with hyphens)
        assert_eq!(id.len(), 36, "UUID should be exactly 36 characters");
        assert!(
            id.chars().filter(|c| *c == '-').count() == 4,
            "UUID should have 4 hyphens"
        );

        // Should be parseable as a UUID
        let parsed = Uuid::parse_str(&id);
        assert!(parsed.is_ok(), "Generated ID should be a valid UUID");
    }

    #[test]
    fn test_generate_relayer_id_long_names() {
        // Test with very long network and signer names
        let long_network = "polygon-zkevm-cardona-testnet-super-long-name";
        let long_signer = "very-long-signer-id-that-exceeds-normal-length";

        let id = RelayerClient::generate_relayer_id(long_network, long_signer);

        // Should still be exactly 36 characters regardless of input length
        assert_eq!(
            id.len(),
            36,
            "UUID should be exactly 36 characters even with long inputs"
        );

        // Should be deterministic even with long names
        let id2 = RelayerClient::generate_relayer_id(long_network, long_signer);
        assert_eq!(
            id, id2,
            "Long names should still generate deterministic UUIDs"
        );
    }
}
