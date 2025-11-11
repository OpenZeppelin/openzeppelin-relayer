//! Stellar Paths Service implementation
//! Uses Stellar Horizon API `/paths/strict-receive` endpoint for quote conversion

use super::{PathStep, StellarDexServiceError, StellarQuoteResponse};
use crate::services::stellar_dex::StellarDexServiceTrait;
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

/// Stellar Horizon API paths response
#[derive(Debug, Deserialize)]
struct PathsResponse {
    #[serde(rename = "_embedded")]
    embedded: PathsEmbedded,
}

#[derive(Debug, Deserialize)]
struct PathsEmbedded {
    records: Vec<PathRecord>,
}

#[derive(Debug, Deserialize)]
struct PathRecord {
    #[serde(rename = "source_asset_type")]
    source_asset_type: String,
    #[serde(rename = "source_asset_code")]
    source_asset_code: Option<String>,
    #[serde(rename = "source_asset_issuer")]
    source_asset_issuer: Option<String>,
    #[serde(rename = "source_amount")]
    source_amount: String,
    #[serde(rename = "destination_asset_type")]
    destination_asset_type: String,
    #[serde(rename = "destination_asset_code")]
    destination_asset_code: Option<String>,
    #[serde(rename = "destination_asset_issuer")]
    destination_asset_issuer: Option<String>,
    #[serde(rename = "destination_amount")]
    destination_amount: String,
    path: Vec<PathAsset>,
}

#[derive(Debug, Deserialize)]
struct PathAsset {
    #[serde(rename = "asset_type")]
    asset_type: String,
    #[serde(rename = "asset_code")]
    asset_code: Option<String>,
    #[serde(rename = "asset_issuer")]
    asset_issuer: Option<String>,
}

/// Service for getting quotes from Stellar Horizon Paths API
pub struct PathsService {
    horizon_base_url: String,
    client: Client,
}

impl PathsService {
    /// Create a new PathsService instance
    ///
    /// # Arguments
    ///
    /// * `horizon_base_url` - Base URL for Stellar Horizon API (e.g., "https://horizon.stellar.org")
    pub fn new(horizon_base_url: String) -> Result<Self, StellarDexServiceError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| StellarDexServiceError::HttpRequestError(e))?;

        Ok(Self {
            horizon_base_url,
            client,
        })
    }

    /// Convert asset identifier to Horizon API format
    ///
    /// # Arguments
    ///
    /// * `asset_id` - Asset identifier (e.g., "native", "USDC:GA5Z...", or "native")
    ///
    /// # Returns
    ///
    /// Tuple of (asset_type, asset_code, asset_issuer)
    fn parse_asset_id(
        &self,
        asset_id: &str,
    ) -> Result<(String, Option<String>, Option<String>), StellarDexServiceError> {
        if asset_id == "native" || asset_id.is_empty() {
            return Ok(("native".to_string(), None, None));
        }

        // Try to parse as "CODE:ISSUER" format
        if let Some(colon_pos) = asset_id.find(':') {
            let code = asset_id[..colon_pos].to_string();
            let issuer = asset_id[colon_pos + 1..].to_string();

            if code.len() <= 4 {
                Ok(("credit_alphanum4".to_string(), Some(code), Some(issuer)))
            } else if code.len() <= 12 {
                Ok(("credit_alphanum12".to_string(), Some(code), Some(issuer)))
            } else {
                Err(StellarDexServiceError::InvalidAssetIdentifier(format!(
                    "Asset code too long: {}",
                    code
                )))
            }
        } else {
            Err(StellarDexServiceError::InvalidAssetIdentifier(format!(
                "Invalid asset format. Expected 'native' or 'CODE:ISSUER', got: {}",
                asset_id
            )))
        }
    }

    /// Build Horizon API URL for paths/strict-receive endpoint
    fn build_paths_url(
        &self,
        source_asset_type: &str,
        source_asset_code: Option<&str>,
        source_asset_issuer: Option<&str>,
        destination_asset_type: &str,
        destination_asset_code: Option<&str>,
        destination_asset_issuer: Option<&str>,
        destination_amount: u64,
    ) -> String {
        let mut url = format!("{}/paths/strict-receive", self.horizon_base_url);

        // Build query parameters
        let mut params = vec![];

        // Source asset
        params.push(format!("source_asset_type={}", source_asset_type));
        if let Some(code) = source_asset_code {
            params.push(format!("source_asset_code={}", code));
        }
        if let Some(issuer) = source_asset_issuer {
            params.push(format!("source_asset_issuer={}", issuer));
        }

        // Destination asset
        params.push(format!("destination_asset_type={}", destination_asset_type));
        if let Some(code) = destination_asset_code {
            params.push(format!("destination_asset_code={}", code));
        }
        if let Some(issuer) = destination_asset_issuer {
            params.push(format!("destination_asset_issuer={}", issuer));
        }

        // Destination amount
        params.push(format!("destination_amount={}", destination_amount));

        url.push('?');
        url.push_str(&params.join("&"));

        url
    }

    /// Fetch path from Horizon API
    async fn fetch_path(
        &self,
        source_asset_type: &str,
        source_asset_code: Option<&str>,
        source_asset_issuer: Option<&str>,
        destination_asset_type: &str,
        destination_asset_code: Option<&str>,
        destination_asset_issuer: Option<&str>,
        destination_amount: u64,
    ) -> Result<PathRecord, StellarDexServiceError> {
        let url = self.build_paths_url(
            source_asset_type,
            source_asset_code,
            source_asset_issuer,
            destination_asset_type,
            destination_asset_code,
            destination_asset_issuer,
            destination_amount,
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| StellarDexServiceError::HttpRequestError(e))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(StellarDexServiceError::ApiError {
                message: format!("Horizon API returned error {}: {}", status, error_text),
            });
        }

        let paths_response: PathsResponse = response.json().await.map_err(|e| {
            StellarDexServiceError::UnknownError(format!("Failed to deserialize response: {}", e))
        })?;

        paths_response
            .embedded
            .records
            .into_iter()
            .next()
            .ok_or(StellarDexServiceError::NoPathFound)
    }
}

#[async_trait]
impl StellarDexServiceTrait for PathsService {
    async fn get_token_to_xlm_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<StellarQuoteResponse, StellarDexServiceError> {
        // Handle native XLM - no conversion needed
        if asset_id == "native" || asset_id.is_empty() {
            return Ok(StellarQuoteResponse {
                input_asset: "native".to_string(),
                output_asset: "native".to_string(),
                in_amount: amount,
                out_amount: amount,
                price_impact_pct: 0.0,
                slippage_bps: (slippage * 100.0) as u32,
                path: None,
            });
        }

        let (source_asset_type, source_code, source_issuer) = self.parse_asset_id(asset_id)?;

        // Fetch path from token to XLM
        let path_record = self
            .fetch_path(
                &source_asset_type,
                source_code.as_deref(),
                source_issuer.as_deref(),
                "native",
                None,
                None,
                amount, // We want this amount of XLM
            )
            .await?;

        // Parse amounts
        let source_amount: u64 = path_record.source_amount.parse().map_err(|e| {
            StellarDexServiceError::UnknownError(format!("Failed to parse source_amount: {}", e))
        })?;

        let destination_amount: u64 = path_record.destination_amount.parse().map_err(|e| {
            StellarDexServiceError::UnknownError(format!(
                "Failed to parse destination_amount: {}",
                e
            ))
        })?;

        // Build path steps
        let path_steps: Vec<PathStep> = path_record
            .path
            .into_iter()
            .map(|asset| PathStep {
                asset_code: asset.asset_code,
                asset_issuer: asset.asset_issuer,
                amount: 0, // Horizon doesn't provide per-step amounts
            })
            .collect();

        Ok(StellarQuoteResponse {
            input_asset: asset_id.to_string(),
            output_asset: "native".to_string(),
            in_amount: source_amount,
            out_amount: destination_amount,
            price_impact_pct: 0.0, // Horizon doesn't provide price impact
            slippage_bps: (slippage * 100.0) as u32,
            path: if path_steps.is_empty() {
                None
            } else {
                Some(path_steps)
            },
        })
    }

    async fn get_xlm_to_token_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<StellarQuoteResponse, StellarDexServiceError> {
        // Handle native XLM - no conversion needed
        if asset_id == "native" || asset_id.is_empty() {
            return Ok(StellarQuoteResponse {
                input_asset: "native".to_string(),
                output_asset: "native".to_string(),
                in_amount: amount,
                out_amount: amount,
                price_impact_pct: 0.0,
                slippage_bps: (slippage * 100.0) as u32,
                path: None,
            });
        }

        let (dest_asset_type, dest_code, dest_issuer) = self.parse_asset_id(asset_id)?;

        // Fetch path from XLM to token
        let path_record = self
            .fetch_path(
                "native",
                None,
                None,
                &dest_asset_type,
                dest_code.as_deref(),
                dest_issuer.as_deref(),
                amount, // We want this amount of token
            )
            .await?;

        // Parse amounts
        let source_amount: u64 = path_record.source_amount.parse().map_err(|e| {
            StellarDexServiceError::UnknownError(format!("Failed to parse source_amount: {}", e))
        })?;

        let destination_amount: u64 = path_record.destination_amount.parse().map_err(|e| {
            StellarDexServiceError::UnknownError(format!(
                "Failed to parse destination_amount: {}",
                e
            ))
        })?;

        // Build path steps
        let path_steps: Vec<PathStep> = path_record
            .path
            .into_iter()
            .map(|asset| PathStep {
                asset_code: asset.asset_code,
                asset_issuer: asset.asset_issuer,
                amount: 0, // Horizon doesn't provide per-step amounts
            })
            .collect();

        Ok(StellarQuoteResponse {
            input_asset: "native".to_string(),
            output_asset: asset_id.to_string(),
            in_amount: source_amount,
            out_amount: destination_amount,
            price_impact_pct: 0.0, // Horizon doesn't provide price impact
            slippage_bps: (slippage * 100.0) as u32,
            path: if path_steps.is_empty() {
                None
            } else {
                Some(path_steps)
            },
        })
    }
}
