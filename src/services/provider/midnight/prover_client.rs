//! Prover client for Midnight proof generation

use crate::{
    domain::{MidnightProofRequest, MidnightProofResponse},
    services::provider::ProviderError,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Client for communicating with the Midnight prover server
pub struct ProverClient {
    client: Client,
    base_url: String,
}

impl ProverClient {
    /// Creates a new prover client
    pub fn new(base_url: String) -> Result<Self, ProviderError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(300)) // 5 minute timeout for proof generation
            .build()
            .map_err(|e| ProviderError::NetworkConfiguration(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self { client, base_url })
    }

    /// Request a proof from the prover server
    pub async fn request_proof(
        &self,
        request: MidnightProofRequest,
    ) -> Result<String, ProviderError> {
        let url = format!("{}/prove", self.base_url);

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| ProviderError::Other(format!("Failed to send proof request: {}", e)))?;

        if !response.status().is_success() {
            let status_code = response.status().as_u16();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(ProviderError::RequestError {
                error: format!("Prover server error: {}", error_text),
                status_code,
            });
        }

        let proof_id: ProofRequestResponse = response
            .json()
            .await
            .map_err(|e| ProviderError::Other(format!("Failed to parse proof response: {}", e)))?;

        Ok(proof_id.request_id)
    }

    /// Check the status of a proof request
    pub async fn get_proof_status(&self, request_id: &str) -> Result<ProofStatus, ProviderError> {
        let url = format!("{}/status/{}", self.base_url, request_id);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ProviderError::Other(format!("Failed to get proof status: {}", e)))?;

        if !response.status().is_success() {
            let status_code = response.status().as_u16();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(ProviderError::RequestError {
                error: format!("Prover server error: {}", error_text),
                status_code,
            });
        }

        let status: ProofStatusResponse = response
            .json()
            .await
            .map_err(|e| ProviderError::Other(format!("Failed to parse status response: {}", e)))?;

        Ok(status.status)
    }

    /// Get a completed proof
    pub async fn get_proof(
        &self,
        request_id: &str,
    ) -> Result<MidnightProofResponse, ProviderError> {
        let url = format!("{}/proof/{}", self.base_url, request_id);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ProviderError::Other(format!("Failed to get proof: {}", e)))?;

        if !response.status().is_success() {
            let status_code = response.status().as_u16();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(ProviderError::RequestError {
                error: format!("Prover server error: {}", error_text),
                status_code,
            });
        }

        let proof: MidnightProofResponse = response
            .json()
            .await
            .map_err(|e| ProviderError::Other(format!("Failed to parse proof: {}", e)))?;

        Ok(proof)
    }

    /// Request a proof and wait for completion
    pub async fn request_and_wait_for_proof(
        &self,
        request: MidnightProofRequest,
    ) -> Result<MidnightProofResponse, ProviderError> {
        let request_id = self.request_proof(request).await?;

        // Poll for completion
        let mut attempts = 0;
        const MAX_ATTEMPTS: u32 = 60; // 5 minutes with 5 second intervals
        const POLL_INTERVAL: Duration = Duration::from_secs(5);

        loop {
            tokio::time::sleep(POLL_INTERVAL).await;

            match self.get_proof_status(&request_id).await? {
                ProofStatus::Completed => {
                    return self.get_proof(&request_id).await;
                }
                ProofStatus::Failed(error) => {
                    return Err(ProviderError::Other(format!(
                        "Proof generation failed: {}",
                        error
                    )));
                }
                ProofStatus::Pending | ProofStatus::Processing => {
                    attempts += 1;
                    if attempts >= MAX_ATTEMPTS {
                        return Err(ProviderError::Timeout);
                    }
                }
            }
        }
    }
}

// Response types for the prover server
#[derive(Debug, Serialize, Deserialize)]
struct ProofRequestResponse {
    request_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProofStatusResponse {
    request_id: String,
    status: ProofStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProofStatus {
    Pending,
    Processing,
    Completed,
    Failed(String),
}
