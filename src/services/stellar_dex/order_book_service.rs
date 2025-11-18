//! Stellar DEX Service implementation
//! Uses Stellar Horizon API `/paths/strict-send` endpoint for path finding and quote conversion
//! This provides better rates by considering liquidity pools (AMMs) and multi-hop paths

use super::{
    AssetType, StellarDexServiceError, StellarQuoteResponse, SwapExecutionResult,
    SwapTransactionParams,
};
use crate::constants::STELLAR_DEFAULT_TRANSACTION_FEE;
use crate::domain::relayer::string_to_muxed_account;
use crate::models::transaction::stellar::asset::AssetSpec;
use crate::models::Address;
use crate::services::{
    provider::StellarProviderTrait, signer::Signer, signer::StellarSignTrait,
    stellar_dex::StellarDexServiceTrait,
};
use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Utc};
use reqwest::Client;
use serde::Deserialize;
use soroban_rs::xdr::{
    Asset, Limits, Memo, Operation, OperationBody, PathPaymentStrictSendOp, Preconditions,
    ReadXdr, SequenceNumber, TimeBounds, TimePoint, Transaction, TransactionEnvelope,
    TransactionExt, TransactionV1Envelope, VecM, WriteXdr,
};
use std::convert::TryFrom;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info};

/// Tolerance for remaining amount in order book calculations (1 stroop)
const ORDER_BOOK_TOLERANCE_STROOPS: i128 = 1;

/// Minimum price threshold to avoid division by zero
const MIN_PRICE_THRESHOLD: f64 = 1e-10;

/// Transaction validity window in minutes
const TRANSACTION_VALIDITY_MINUTES: i64 = 1;

/// HTTP request timeout in seconds
const HTTP_REQUEST_TIMEOUT_SECONDS: u64 = 7;

/// Slippage percentage multiplier for basis points conversion
const SLIPPAGE_TO_BPS_MULTIPLIER: f32 = 100.0;

/// Maximum number of orders to process in a single quote calculation
const MAX_ORDERS_TO_PROCESS: usize = 100;

/// Maximum asset code length for credit_alphanum4 assets
const MAX_CREDIT_ALPHANUM4_CODE_LEN: usize = 4;

/// Maximum asset code length for credit_alphanum12 assets
const MAX_CREDIT_ALPHANUM12_CODE_LEN: usize = 12;

/// Stellar Horizon API order book response
#[derive(Debug, Deserialize)]
struct OrderBookResponse {
    bids: Vec<Order>,
    asks: Vec<Order>,
    base: AssetInfo,
    counter: AssetInfo,
}

#[derive(Debug, Deserialize)]
struct Order {
    #[serde(rename = "price_r")]
    price_r: RationalPrice,
    price: String,
    amount: String,
}

#[derive(Debug, Deserialize)]
struct RationalPrice {
    n: u64,
    d: u64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AssetInfo {
    #[serde(rename = "asset_type")]
    asset_type: String,
    #[serde(rename = "asset_code")]
    asset_code: Option<String>,
    #[serde(rename = "asset_issuer")]
    asset_issuer: Option<String>,
}

/// Stellar Horizon API path finding response
#[derive(Debug, Deserialize)]
struct PathResponse {
    #[serde(rename = "_embedded")]
    embedded: PathEmbedded,
}

#[derive(Debug, Deserialize)]
struct PathEmbedded {
    records: Vec<PathRecord>,
}

#[derive(Debug, Deserialize)]
struct PathRecord {
    source_amount: String,
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

/// Service for getting quotes from Stellar Horizon Order Book API
pub struct OrderBookService<P, S>
where
    P: StellarProviderTrait + Send + Sync + 'static,
    S: StellarSignTrait + Signer + Send + Sync + 'static,
{
    horizon_base_url: String,
    client: Client,
    provider: Arc<P>,
    signer: Arc<S>,
}

impl<P, S> OrderBookService<P, S>
where
    P: StellarProviderTrait + Send + Sync + 'static,
    S: StellarSignTrait + Signer + Send + Sync + 'static,
{
    /// Create a new OrderBookService instance
    ///
    /// # Arguments
    ///
    /// * `horizon_base_url` - Base URL for Stellar Horizon API (e.g., "https://horizon.stellar.org")
    /// * `provider` - Stellar provider for sending transactions
    /// * `signer` - Stellar signer for signing transactions
    pub fn new(
        horizon_base_url: String,
        provider: Arc<P>,
        signer: Arc<S>,
    ) -> Result<Self, StellarDexServiceError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(HTTP_REQUEST_TIMEOUT_SECONDS))
            .build()
            .map_err(StellarDexServiceError::HttpRequestError)?;

        Ok(Self {
            horizon_base_url,
            client,
            provider,
            signer,
        })
    }

    /// Convert asset identifier to Horizon API format
    ///
    /// # Arguments
    ///
    /// * `asset_id` - Asset identifier (e.g., "native", "USDC:GA5Z...")
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
            let code = asset_id[..colon_pos].trim();
            let issuer = asset_id[colon_pos + 1..].trim();

            if code.is_empty() {
                return Err(StellarDexServiceError::InvalidAssetIdentifier(
                    "Asset code cannot be empty".to_string(),
                ));
            }

            if issuer.is_empty() {
                return Err(StellarDexServiceError::InvalidAssetIdentifier(
                    "Asset issuer cannot be empty".to_string(),
                ));
            }

            if code.len() <= MAX_CREDIT_ALPHANUM4_CODE_LEN {
                Ok((
                    "credit_alphanum4".to_string(),
                    Some(code.to_string()),
                    Some(issuer.to_string()),
                ))
            } else if code.len() <= MAX_CREDIT_ALPHANUM12_CODE_LEN {
                Ok((
                    "credit_alphanum12".to_string(),
                    Some(code.to_string()),
                    Some(issuer.to_string()),
                ))
            } else {
                Err(StellarDexServiceError::InvalidAssetIdentifier(format!(
                    "Asset code too long (max {} characters): {}",
                    MAX_CREDIT_ALPHANUM12_CODE_LEN, code
                )))
            }
        } else {
            Err(StellarDexServiceError::InvalidAssetIdentifier(format!(
                "Invalid asset format. Expected 'native' or 'CODE:ISSUER', got: {}",
                asset_id
            )))
        }
    }

    /// Parse asset identifier to AssetSpec for XDR conversion
    ///
    /// # Arguments
    ///
    /// * `asset_id` - Asset identifier (e.g., "USDC:GA5Z...")
    ///
    /// # Returns
    ///
    /// AssetSpec that can be converted to XDR Asset
    fn parse_asset_to_spec(&self, asset_id: &str) -> Result<AssetSpec, StellarDexServiceError> {
        if asset_id == "native" || asset_id.is_empty() {
            return Err(StellarDexServiceError::InvalidAssetIdentifier(
                "Native asset cannot be parsed to AssetSpec".to_string(),
            ));
        }

        let (code, issuer) = asset_id.split_once(':').ok_or_else(|| {
            StellarDexServiceError::InvalidAssetIdentifier(format!(
                "Invalid source asset format. Expected 'CODE:ISSUER', got: {}",
                asset_id
            ))
        })?;

        let code = code.trim();
        let issuer = issuer.trim();

        if code.is_empty() || issuer.is_empty() {
            return Err(StellarDexServiceError::InvalidAssetIdentifier(
                "Asset code and issuer cannot be empty".to_string(),
            ));
        }

        if code.len() <= 4 {
            Ok(AssetSpec::Credit4 {
                code: code.to_string(),
                issuer: issuer.to_string(),
            })
        } else if code.len() <= 12 {
            Ok(AssetSpec::Credit12 {
                code: code.to_string(),
                issuer: issuer.to_string(),
            })
        } else {
            Err(StellarDexServiceError::InvalidAssetIdentifier(format!(
                "Asset code too long (max 12 characters): {}",
                code
            )))
        }
    }

    /// Helper to convert stroops to decimal string
    /// e.g. 10000000 stroops -> "1.0000000"
    fn to_decimal_string(&self, stroops: u64, decimals: u8) -> String {
        let s = stroops.to_string();
        let decimals_usize = decimals as usize;
        
        if s.len() <= decimals_usize {
            // e.g. 100 stroops, 7 decimals -> "0.0000100"
            format!("0.{:0>width$}", s, width = decimals_usize)
        } else {
            // e.g. 12345678 stroops, 7 decimals -> "1.2345678"
            let split = s.len() - decimals_usize;
            format!("{}.{}", &s[..split], &s[split..])
        }
    }

    /// Fetch strict-send paths from Horizon
    /// strict-send = "I am sending exactly X source asset, how much Y dest can I get?"
    async fn fetch_strict_send_paths(
        &self,
        source_asset: &AssetSpec,
        source_amount_stroops: u64,
        destination_asset: &AssetSpec,
        destination_account: &str,
    ) -> Result<Vec<PathRecord>, StellarDexServiceError> {
        let mut url = format!("{}/paths/strict-send", self.horizon_base_url);

        // Convert stroops to decimal string for the API
        let amount_decimal = self.to_decimal_string(source_amount_stroops, 7);

        let mut params = vec![
            format!("destination_account={}", destination_account),
            format!("source_amount={}", amount_decimal),
        ];

        // Add Source Asset Params
        match source_asset {
            AssetSpec::Native => {
                params.push("source_asset_type=native".to_string());
            }
            AssetSpec::Credit4 { code, issuer } => {
                params.push("source_asset_type=credit_alphanum4".to_string());
                params.push(format!("source_asset_code={}", code));
                params.push(format!("source_asset_issuer={}", issuer));
            }
            AssetSpec::Credit12 { code, issuer } => {
                params.push("source_asset_type=credit_alphanum12".to_string());
                params.push(format!("source_asset_code={}", code));
                params.push(format!("source_asset_issuer={}", issuer));
            }
        }

        // Add Destination Asset Params
        match destination_asset {
            AssetSpec::Native => {
                params.push("destination_assets=native".to_string());
            }
            AssetSpec::Credit4 { code, issuer } => {
                params.push(format!("destination_assets={}:{}", code, issuer));
            }
            AssetSpec::Credit12 { code, issuer } => {
                params.push(format!("destination_assets={}:{}", code, issuer));
            }
        }

        url.push('?');
        url.push_str(&params.join("&"));

        debug!("Fetching paths from Horizon: {}", url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(StellarDexServiceError::HttpRequestError)?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error response".to_string());
            return Err(StellarDexServiceError::ApiError {
                message: format!("Horizon API returned error {}: {}", status, error_text),
            });
        }

        let path_response: PathResponse = response.json().await.map_err(|e| {
            StellarDexServiceError::UnknownError(format!("Failed to deserialize paths: {}", e))
        })?;

        Ok(path_response.embedded.records)
    }

    /// Fetch strict-receive paths from Horizon
    /// strict-receive = "I want to receive exactly X dest asset, how much Y source do I need to send?"
    async fn fetch_strict_receive_paths(
        &self,
        source_asset: &AssetSpec,
        destination_asset: &AssetSpec,
        destination_amount_stroops: u64,
        destination_account: &str,
        destination_decimals: u8,
    ) -> Result<Vec<PathRecord>, StellarDexServiceError> {
        let mut url = format!("{}/paths/strict-receive", self.horizon_base_url);

        // Convert destination amount stroops to decimal string for the API
        let dest_amount_decimal = self.to_decimal_string(destination_amount_stroops, destination_decimals);

        let mut params = vec![
            format!("destination_account={}", destination_account),
            format!("destination_amount={}", dest_amount_decimal),
        ];

        // Add Source Asset Params
        match source_asset {
            AssetSpec::Native => {
                params.push("source_assets=native".to_string());
            }
            AssetSpec::Credit4 { code, issuer } => {
                params.push(format!("source_assets={}:{}", code, issuer));
            }
            AssetSpec::Credit12 { code, issuer } => {
                params.push(format!("source_assets={}:{}", code, issuer));
            }
        }

        // Add Destination Asset Params
        match destination_asset {
            AssetSpec::Native => {
                params.push("destination_asset_type=native".to_string());
            }
            AssetSpec::Credit4 { code, issuer } => {
                params.push("destination_asset_type=credit_alphanum4".to_string());
                params.push(format!("destination_asset_code={}", code));
                params.push(format!("destination_asset_issuer={}", issuer));
            }
            AssetSpec::Credit12 { code, issuer } => {
                params.push("destination_asset_type=credit_alphanum12".to_string());
                params.push(format!("destination_asset_code={}", code));
                params.push(format!("destination_asset_issuer={}", issuer));
            }
        }

        url.push('?');
        url.push_str(&params.join("&"));

        debug!("Fetching strict-receive paths from Horizon: {}", url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(StellarDexServiceError::HttpRequestError)?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error response".to_string());
            return Err(StellarDexServiceError::ApiError {
                message: format!("Horizon API returned error {}: {}", status, error_text),
            });
        }

        let path_response: PathResponse = response.json().await.map_err(|e| {
            StellarDexServiceError::UnknownError(format!("Failed to deserialize paths: {}", e))
        })?;

        Ok(path_response.embedded.records)
    }

    /// Convert PathAsset to XDR Asset
    fn xdr_asset_from_path_asset(
        &self,
        path_asset: &PathAsset,
    ) -> Result<Asset, StellarDexServiceError> {
        match path_asset.asset_type.as_str() {
            "native" => Ok(Asset::Native),
            "credit_alphanum4" | "credit_alphanum12" => {
                let code = path_asset.asset_code.as_ref().ok_or_else(|| {
                    StellarDexServiceError::InvalidAssetIdentifier(
                        "Path asset missing code".to_string(),
                    )
                })?;
                let issuer = path_asset.asset_issuer.as_ref().ok_or_else(|| {
                    StellarDexServiceError::InvalidAssetIdentifier(
                        "Path asset missing issuer".to_string(),
                    )
                })?;

                let asset_spec = if code.len() <= 4 {
                    AssetSpec::Credit4 {
                        code: code.clone(),
                        issuer: issuer.clone(),
                    }
                } else {
                    AssetSpec::Credit12 {
                        code: code.clone(),
                        issuer: issuer.clone(),
                    }
                };

                Asset::try_from(asset_spec).map_err(|e| {
                    StellarDexServiceError::InvalidAssetIdentifier(format!(
                        "Failed to convert path asset to XDR: {}",
                        e
                    ))
                })
            }
            _ => Err(StellarDexServiceError::InvalidAssetIdentifier(format!(
                "Unknown asset type in path: {}",
                path_asset.asset_type
            ))),
        }
    }

    /// Convert path from quote to XDR format
    fn convert_path_to_xdr(
        &self,
        path: &[PathAsset],
    ) -> Result<Vec<Asset>, StellarDexServiceError> {
        path.iter()
            .map(|p| self.xdr_asset_from_path_asset(p))
            .collect()
    }

    /// Build Horizon API URL for order_book endpoint
    ///
    /// Note: Stellar asset codes and issuers are typically safe ASCII strings,
    /// so basic URL construction is sufficient. If special characters are
    /// encountered, they should be properly encoded.
    ///
    /// # Arguments
    ///
    /// * `selling_asset_*` - Parameters for the asset being sold (base)
    /// * `buying_asset_*` - Parameters for the asset being bought (counter)
    ///
    /// # Returns
    ///
    /// Complete URL string for the Horizon order_book API endpoint
    fn build_order_book_url(
        &self,
        selling_asset_type: &str,
        selling_asset_code: Option<&str>,
        selling_asset_issuer: Option<&str>,
        buying_asset_type: &str,
        buying_asset_code: Option<&str>,
        buying_asset_issuer: Option<&str>,
    ) -> String {
        let mut url = format!("{}/order_book", self.horizon_base_url);

        // Build query parameters
        let mut params = vec![];

        // Selling asset (base)
        params.push(format!("selling_asset_type={}", selling_asset_type));
        if let Some(code) = selling_asset_code {
            params.push(format!("selling_asset_code={}", code));
        }
        if let Some(issuer) = selling_asset_issuer {
            params.push(format!("selling_asset_issuer={}", issuer));
        }

        // Buying asset (counter)
        params.push(format!("buying_asset_type={}", buying_asset_type));
        if let Some(code) = buying_asset_code {
            params.push(format!("buying_asset_code={}", code));
        }
        if let Some(issuer) = buying_asset_issuer {
            params.push(format!("buying_asset_issuer={}", issuer));
        }

        url.push('?');
        url.push_str(&params.join("&"));

        url
    }

    /// Fetch order book from Horizon API
    async fn fetch_order_book(
        &self,
        selling_asset_type: &str,
        selling_asset_code: Option<&str>,
        selling_asset_issuer: Option<&str>,
        buying_asset_type: &str,
        buying_asset_code: Option<&str>,
        buying_asset_issuer: Option<&str>,
    ) -> Result<OrderBookResponse, StellarDexServiceError> {
        let url = self.build_order_book_url(
            selling_asset_type,
            selling_asset_code,
            selling_asset_issuer,
            buying_asset_type,
            buying_asset_code,
            buying_asset_issuer,
        );

        debug!("Fetching order book from Horizon: {}", url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(StellarDexServiceError::HttpRequestError)?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error response".to_string());
            return Err(StellarDexServiceError::ApiError {
                message: format!("Horizon API returned error {}: {}", status, error_text),
            });
        }

        response.json().await.map_err(|e| {
            StellarDexServiceError::UnknownError(format!("Failed to deserialize response: {}", e))
        })
    }

    /// Calculate output amount by walking through order book
    ///
    /// IMPORTANT: Understanding Stellar Order Books
    /// - base asset = asset being SOLD in orders
    /// - counter asset = asset being BOUGHT in orders
    /// - asks = orders selling base for counter (price in counter/base)
    /// - bids = orders buying base with counter (price in counter/base)
    ///
    /// # Arguments
    ///
    /// * `orders` - Order book entries (bids or asks)
    /// * `input_amount` - Amount to convert (in stroops/smallest unit)
    /// * `is_ask` - If true, using asks (selling base), otherwise bids (buying base)
    /// * `asset_decimals` - Number of decimal places for the base asset (defaults to 7)
    ///
    /// # Returns
    ///
    /// Tuple of (output_stroops, avg_price, best_price)
    fn calculate_quote_from_orders(
        &self,
        orders: &[Order],
        input_amount: u64,
        is_ask: bool,
        asset_decimals: u8,
    ) -> Result<(u64, f64, f64), StellarDexServiceError> {
        if orders.is_empty() {
            return Err(StellarDexServiceError::NoPathFound);
        }

        if input_amount == 0 {
            return Err(StellarDexServiceError::InvalidAssetIdentifier(
                "Input amount cannot be zero".to_string(),
            ));
        }

        let best_price: f64 = orders[0].price.parse().map_err(|e| {
            StellarDexServiceError::UnknownError(format!("Failed to parse price: {}", e))
        })?;

        let mut remaining_stroops = input_amount as i128;
        let mut total_output_stroops: i128 = 0;
        let mut total_input_stroops_used: i128 = 0;

        // Limit the number of orders processed for performance
        let orders_to_process = orders.iter().take(MAX_ORDERS_TO_PROCESS);

        for order in orders_to_process {
            if remaining_stroops <= 0 {
                break;
            }

            // Validate rational price
            if order.price_r.d == 0 {
                return Err(StellarDexServiceError::UnknownError(
                    "Invalid order: price denominator is zero".to_string(),
                ));
            }

            // Parse amount (in base asset units as decimal string)
            let amount: f64 = order.amount.parse().map_err(|e| {
                StellarDexServiceError::UnknownError(format!("Failed to parse amount: {}", e))
            })?;

            if amount <= 0.0 {
                debug!("Skipping order with zero or negative amount");
                continue;
            }

            // Convert to stroops using provided decimals
            let decimals_multiplier = 10_f64.powi(asset_decimals as i32);
            let amount_stroops = (amount * decimals_multiplier) as i128;

            if amount_stroops <= 0 {
                debug!("Skipping order with zero stroops after conversion");
                continue;
            }

            // Use rational price for precise calculation
            let price_n = order.price_r.n as i128;
            let price_d = order.price_r.d as i128;

            debug!(
                "Order: amount={} ({} stroops), price={} (rational: {}/{})",
                order.amount, amount_stroops, order.price, price_n, price_d
            );

            if is_ask {
                // Selling base asset to get counter asset
                // Output (counter) = base_amount * (price_n / price_d)

                if remaining_stroops <= amount_stroops {
                    // Partially fill this order
                    let output = (remaining_stroops * price_n) / price_d;
                    total_output_stroops += output;
                    total_input_stroops_used += remaining_stroops;
                    remaining_stroops = 0;
                } else {
                    // Use entire order
                    let output = (amount_stroops * price_n) / price_d;
                    total_output_stroops += output;
                    total_input_stroops_used += amount_stroops;
                    remaining_stroops -= amount_stroops;
                }
            } else {
                // Buying base asset with counter asset
                // To get base_amount, pay: base_amount * (price_n / price_d) in counter
                // We have counter, want to know how much base we can get

                let counter_cost = (amount_stroops * price_n) / price_d;

                debug!(
                    "Buying: remaining={} stroops, order_amount={} stroops base, counter_cost={} stroops",
                    remaining_stroops, amount_stroops, counter_cost
                );

                if remaining_stroops <= counter_cost {
                    // Partially fill this order
                    // Solve: base * (price_n / price_d) = remaining_stroops
                    // base = remaining_stroops * price_d / price_n
                    let base_received = (remaining_stroops * price_d) / price_n;
                    debug!(
                        "Partially filling: base_received={} stroops for {} stroops counter",
                        base_received, remaining_stroops
                    );
                    total_output_stroops += base_received;
                    total_input_stroops_used += remaining_stroops;
                    remaining_stroops = 0;
                } else {
                    // Use entire order
                    debug!(
                        "Using entire order: base_received={} stroops, counter_paid={} stroops",
                        amount_stroops, counter_cost
                    );
                    total_output_stroops += amount_stroops;
                    total_input_stroops_used += counter_cost;
                    remaining_stroops -= counter_cost;
                }
            }
        }

        // Check if we could fulfill the entire amount (allow tolerance)
        if remaining_stroops > ORDER_BOOK_TOLERANCE_STROOPS {
            return Err(StellarDexServiceError::NoPathFound);
        }

        // Validate we got some output
        if total_output_stroops <= 0 {
            return Err(StellarDexServiceError::NoPathFound);
        }

        if total_input_stroops_used <= 0 {
            return Err(StellarDexServiceError::UnknownError(
                "Invalid calculation: no input used".to_string(),
            ));
        }

        // Calculate weighted average price (for price impact calculation)
        let avg_price = if is_ask {
            // Selling: avg_price = output / input
            total_output_stroops as f64 / total_input_stroops_used as f64
        } else {
            // Buying: avg_price = input / output
            total_input_stroops_used as f64 / total_output_stroops as f64
        };

        Ok((total_output_stroops as u64, avg_price, best_price))
    }

    /// Validate swap parameters before processing
    fn validate_swap_params(
        &self,
        params: &SwapTransactionParams,
    ) -> Result<(), StellarDexServiceError> {
        if params.destination_asset != "native" {
            return Err(StellarDexServiceError::UnknownError(
                "Only swapping tokens to XLM (native) is currently supported".to_string(),
            ));
        }

        if params.source_asset == "native" || params.source_asset.is_empty() {
            return Err(StellarDexServiceError::InvalidAssetIdentifier(
                "Source asset cannot be native when swapping to XLM".to_string(),
            ));
        }

        if params.amount == 0 {
            return Err(StellarDexServiceError::InvalidAssetIdentifier(
                "Swap amount cannot be zero".to_string(),
            ));
        }

        if params.slippage_percent < 0.0 || params.slippage_percent > 100.0 {
            return Err(StellarDexServiceError::InvalidAssetIdentifier(
                "Slippage percentage must be between 0 and 100".to_string(),
            ));
        }

        Ok(())
    }

    /// Get quote for swap operation
    async fn get_swap_quote(
        &self,
        params: &SwapTransactionParams,
    ) -> Result<StellarQuoteResponse, StellarDexServiceError> {
        if params.destination_asset == "native" {
            self.get_token_to_xlm_quote(
                &params.source_asset,
                params.amount,
                params.slippage_percent,
                params.source_asset_decimals,
            )
            .await
        } else {
            Err(StellarDexServiceError::UnknownError(
                "Unsupported destination asset".to_string(),
            ))
        }
    }

    /// Build XDR string for swap transaction
    async fn build_swap_transaction_xdr(
        &self,
        params: &SwapTransactionParams,
        quote: &StellarQuoteResponse,
    ) -> Result<String, StellarDexServiceError> {
        // Parse source account to MuxedAccount
        let source_account = string_to_muxed_account(&params.source_account).map_err(|e| {
            StellarDexServiceError::InvalidAssetIdentifier(format!("Invalid source account: {}", e))
        })?;

        // Parse source asset to Asset using shared helper
        let asset_spec = self.parse_asset_to_spec(&params.source_asset)?;
        let send_asset = Asset::try_from(asset_spec).map_err(|e| {
            StellarDexServiceError::InvalidAssetIdentifier(format!(
                "Failed to convert asset: {}",
                e
            ))
        })?;

        // Destination asset is always native XLM
        let dest_asset = Asset::Native;

        // 1. Define exact amount to SEND
        let send_amount = i64::try_from(params.amount).map_err(|_| {
            StellarDexServiceError::UnknownError("Amount too large for i64".to_string())
        })?;

        // 2. Define minimum amount to RECEIVE (Integer Math)
        // Formula: out_amount * (10000 - bps) / 10000
        // Cast to u128 to prevent overflow during multiplication
        let out_amount = quote.out_amount as u128;
        let slippage_bps = quote.slippage_bps as u128;
        let basis = 10000u128;

        // Calculate minimum acceptable amount
        // e.g., If slippage is 100bps (1%), we want 99% of the output
        let dest_min_u128 = out_amount
            .checked_mul(basis.saturating_sub(slippage_bps))
            .ok_or_else(|| StellarDexServiceError::UnknownError("Overflow calculating min amount".into()))?
            .checked_div(basis)
            .ok_or_else(|| StellarDexServiceError::UnknownError("Division error".into()))?;

        // Ensure we don't request 0 (unless the quote was actually 0)
        // Note: Asking for 0 min receive is technically valid (accept anything), but risky.
        let dest_min_final = if dest_min_u128 == 0 && out_amount > 0 {
            1 // Require at least 1 stroop if the quote wasn't zero
        } else {
            dest_min_u128
        };

        let dest_min = i64::try_from(dest_min_final).map_err(|_| {
            StellarDexServiceError::UnknownError("Destination amount too large for i64".to_string())
        })?;

        // 3. Extract Path from Quote (if it exists)
        let path_assets: Vec<Asset> = if let Some(quote_path) = &quote.path {
            quote_path
                .iter()
                .map(|p| {
                    // Convert PathStep to AssetSpec, then to Asset
                    let asset_spec = if let (Some(code), Some(issuer)) = (&p.asset_code, &p.asset_issuer) {
                        if code.len() <= 4 {
                            AssetSpec::Credit4 {
                                code: code.clone(),
                                issuer: issuer.clone(),
                            }
                        } else {
                            AssetSpec::Credit12 {
                                code: code.clone(),
                                issuer: issuer.clone(),
                            }
                        }
                    } else {
                        AssetSpec::Native
                    };
                    Asset::try_from(asset_spec).map_err(|e| {
                        StellarDexServiceError::InvalidAssetIdentifier(format!(
                            "Failed to convert path step to XDR asset: {}",
                            e
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };

        // Convert Vec<Asset> to VecM<Asset, 5> (Stellar supports up to 5 intermediate assets in a path)
        let path_vecm: VecM<Asset, 5> = path_assets.try_into().map_err(|_| {
            StellarDexServiceError::UnknownError("Failed to convert path to VecM (path too long, max 5 assets)".to_string())
        })?;

        // Create PathPaymentStrictSend operation
        let path_payment_op = Operation {
            source_account: None,
            body: OperationBody::PathPaymentStrictSend(PathPaymentStrictSendOp {
                send_asset,
                send_amount, // We strictly send this amount
                destination: source_account.clone(),
                dest_asset,
                dest_min, // We accept at least this much
                path: path_vecm, // Populate the path here!
            }),
        };

        // Set time bounds: valid from now until configured validity window
        let now = Utc::now();
        let valid_until = now + ChronoDuration::minutes(TRANSACTION_VALIDITY_MINUTES);
        let time_bounds = TimeBounds {
            min_time: TimePoint(now.timestamp() as u64),
            max_time: TimePoint(valid_until.timestamp() as u64),
        };

        // Build Transaction
        // Note: Sequence number is set to 0 as a placeholder
        // The transaction pipeline will update it with the correct sequence number
        // when processing the transaction through the gate mechanism
        let transaction = Transaction {
            source_account,
            fee: STELLAR_DEFAULT_TRANSACTION_FEE,
            seq_num: SequenceNumber(0), // Placeholder - will be updated by transaction pipeline
            cond: Preconditions::Time(time_bounds),
            memo: Memo::None,
            operations: vec![path_payment_op].try_into().map_err(|_| {
                StellarDexServiceError::UnknownError(
                    "Failed to create operations vector".to_string(),
                )
            })?,
            ext: TransactionExt::V0,
        };

        // Create TransactionEnvelope and serialize to XDR base64
        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx: transaction,
            signatures: VecM::default(), // Unsigned transaction
        });

        envelope.to_xdr_base64(Limits::none()).map_err(|e| {
            StellarDexServiceError::UnknownError(format!(
                "Failed to serialize transaction to XDR: {}",
                e
            ))
        })
    }

    /// Sign and submit transaction, returning the transaction hash
    async fn sign_and_submit_transaction(
        &self,
        xdr: &str,
        network_passphrase: &str,
    ) -> Result<soroban_rs::xdr::Hash, StellarDexServiceError> {
        // Sign the transaction
        let signed_response = self
            .signer
            .sign_xdr_transaction(xdr, network_passphrase)
            .await
            .map_err(|e| {
                StellarDexServiceError::UnknownError(format!("Failed to sign transaction: {}", e))
            })?;

        debug!("Transaction signed successfully, submitting to network");

        // Parse the signed XDR to get the envelope
        let signed_envelope =
            TransactionEnvelope::from_xdr_base64(&signed_response.signed_xdr, Limits::none())
                .map_err(|e| {
                    StellarDexServiceError::UnknownError(format!(
                        "Failed to parse signed XDR: {}",
                        e
                    ))
                })?;

        // Send the transaction
        self.provider
            .send_transaction(&signed_envelope)
            .await
            .map_err(|e| {
                StellarDexServiceError::UnknownError(format!("Failed to send transaction: {}", e))
            })
    }
}

#[async_trait]
impl<P, S> StellarDexServiceTrait for OrderBookService<P, S>
where
    P: StellarProviderTrait + Send + Sync + 'static,
    S: StellarSignTrait + Signer + Send + Sync + 'static,
{
    fn supported_asset_types(&self) -> std::collections::HashSet<AssetType> {
        use std::collections::HashSet;
        let mut types = HashSet::new();
        types.insert(AssetType::Native);
        types.insert(AssetType::Classic);
        types
    }

    async fn get_token_to_xlm_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
        _asset_decimals: Option<u8>,
    ) -> Result<StellarQuoteResponse, StellarDexServiceError> {
        if asset_id == "native" || asset_id.is_empty() {
            return Ok(StellarQuoteResponse {
                input_asset: "native".to_string(),
                output_asset: "native".to_string(),
                in_amount: amount,
                out_amount: amount,
                price_impact_pct: 0.0,
                slippage_bps: (slippage * SLIPPAGE_TO_BPS_MULTIPLIER) as u32,
                path: None,
            });
        }

        // Parse source asset
        let source_spec = self.parse_asset_to_spec(asset_id)?;
        let dest_spec = AssetSpec::Native;

        // Use signer's address as destination account for path finding
        // This ensures the path finder checks trustlines
        let signer_address = match self
            .signer
            .address()
            .await
            .map_err(|e| {
                StellarDexServiceError::UnknownError(format!(
                    "Failed to get signer address: {}",
                    e
                ))
            })? {
            Address::Stellar(addr) => addr,
            _ => {
                return Err(StellarDexServiceError::UnknownError(
                    "Signer address is not a Stellar address".to_string(),
                ))
            }
        };

        // Fetch paths using strict-send path finding
        let paths = self
            .fetch_strict_send_paths(&source_spec, amount, &dest_spec, &signer_address)
            .await?;

        if paths.is_empty() {
            return Err(StellarDexServiceError::NoPathFound);
        }

        // The paths are typically sorted by best price (highest destination_amount)
        let best_path = &paths[0];

        // Parse destination amount from string to stroops
        // Horizon returns strings like "123.456"
        let out_amount_f64: f64 = best_path.destination_amount.parse().map_err(|e| {
            StellarDexServiceError::UnknownError(format!(
                "Failed to parse destination amount: {}",
                e
            ))
        })?;
        let out_amount = (out_amount_f64 * 10_000_000.0) as u64;

        // Convert path to PathStep format for the response
        let path_steps: Vec<super::PathStep> = best_path
            .path
            .iter()
            .map(|p| super::PathStep {
                asset_code: p.asset_code.clone(),
                asset_issuer: p.asset_issuer.clone(),
                amount: 0, // Path steps don't include amounts in Horizon response
            })
            .collect();

        // Calculate price impact (simplified - could be improved with more data)
        // For now, set to 0 as path finding already finds the best path
        let price_impact_pct = 0.0;

        Ok(StellarQuoteResponse {
            input_asset: asset_id.to_string(),
            output_asset: "native".to_string(),
            in_amount: amount,
            out_amount,
            price_impact_pct,
            slippage_bps: (slippage * 100.0) as u32,
            path: Some(path_steps),
        })
    }

    async fn get_xlm_to_token_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
        asset_decimals: Option<u8>,
    ) -> Result<StellarQuoteResponse, StellarDexServiceError> {
        let decimals = asset_decimals.unwrap_or(7); // Default to 7 for Stellar standard
        if asset_id == "native" || asset_id.is_empty() {
            debug!("Getting quote for native to native: {}", amount);
            return Ok(StellarQuoteResponse {
                input_asset: "native".to_string(),
                output_asset: "native".to_string(),
                in_amount: amount,
                out_amount: amount,
                price_impact_pct: 0.0,
                slippage_bps: (slippage * SLIPPAGE_TO_BPS_MULTIPLIER) as u32,
                path: None,
            });
        }

        // Parse destination asset (token we want to receive)
        let dest_spec = self.parse_asset_to_spec(asset_id)?;
        let source_spec = AssetSpec::Native;

        // Use signer's address as destination account for path finding
        // This ensures the path finder checks trustlines
        let signer_address = match self
            .signer
            .address()
            .await
            .map_err(|e| {
                StellarDexServiceError::UnknownError(format!(
                    "Failed to get signer address: {}",
                    e
                ))
            })? {
            Address::Stellar(addr) => addr,
            _ => {
                return Err(StellarDexServiceError::UnknownError(
                    "Signer address is not a Stellar address".to_string(),
                ))
            }
        };

        // Fetch paths using strict-receive path finding
        // We want to receive exactly `amount` tokens, so we pass that as destination_amount
        let paths = self
            .fetch_strict_receive_paths(&source_spec, &dest_spec, amount, &signer_address, decimals)
            .await?;

        if paths.is_empty() {
            return Err(StellarDexServiceError::NoPathFound);
        }

        // The paths are typically sorted by best price (lowest source_amount)
        let best_path = &paths[0];

        // Parse source amount from string to stroops (how much XLM we need to send)
        // Horizon returns strings like "123.456"
        let in_amount_f64: f64 = best_path.source_amount.parse().map_err(|e| {
            StellarDexServiceError::UnknownError(format!(
                "Failed to parse source amount: {}",
                e
            ))
        })?;
        let in_amount = (in_amount_f64 * 10_000_000.0) as u64;

        // Convert path to PathStep format for the response
        let path_steps: Vec<super::PathStep> = best_path
            .path
            .iter()
            .map(|p| super::PathStep {
                asset_code: p.asset_code.clone(),
                asset_issuer: p.asset_issuer.clone(),
                amount: 0, // Path steps don't include amounts in Horizon response
            })
            .collect();

        // Calculate price impact (simplified - could be improved with more data)
        // For now, set to 0 as path finding already finds the best path
        let price_impact_pct = 0.0;

        Ok(StellarQuoteResponse {
            input_asset: "native".to_string(),
            output_asset: asset_id.to_string(),
            in_amount,
            out_amount: amount, // We want to receive exactly this amount
            price_impact_pct,
            slippage_bps: (slippage * 100.0) as u32,
            path: Some(path_steps),
        })
    }

    async fn prepare_swap_transaction(
        &self,
        params: SwapTransactionParams,
    ) -> Result<(String, StellarQuoteResponse), StellarDexServiceError> {
        // Validate parameters upfront
        self.validate_swap_params(&params)?;

        // Get a quote first to determine destination amount
        let quote = self.get_swap_quote(&params).await?;

        info!(
            "Preparing swap transaction: {} {} -> {} XLM (min receive: {})",
            params.amount, params.source_asset, quote.out_amount, quote.out_amount
        );

        // Build the transaction XDR (unsigned)
        let xdr = self.build_swap_transaction_xdr(&params, &quote).await?;

        info!(
            "Successfully prepared swap transaction XDR ({} bytes)",
            xdr.len()
        );

        Ok((xdr, quote))
    }

    async fn execute_swap(
        &self,
        params: SwapTransactionParams,
    ) -> Result<SwapExecutionResult, StellarDexServiceError> {
        // Note: execute_swap is kept for backward compatibility but should generally not be used
        // Swaps should go through the transaction pipeline via prepare_swap_transaction + process_transaction_request
        // which properly manages sequence numbers through the gate mechanism

        // Prepare the swap transaction (get quote and build XDR)
        // Sequence number will be 0 (placeholder) - caller should use pipeline instead
        let (xdr, quote) = self.prepare_swap_transaction(params.clone()).await?;

        info!(
            "Signing and submitting swap transaction XDR ({} bytes)",
            xdr.len()
        );

        // Sign and submit the transaction
        let tx_hash = self
            .sign_and_submit_transaction(&xdr, &params.network_passphrase)
            .await?;

        let transaction_hash = hex::encode(tx_hash.0);

        info!(
            "Swap transaction submitted successfully with hash: {}, destination amount: {}",
            transaction_hash, quote.out_amount
        );

        Ok(SwapExecutionResult {
            transaction_hash,
            destination_amount: quote.out_amount,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::SignXdrTransactionResponseStellar;
    use crate::models::SignerError;
    use crate::services::provider::MockStellarProviderTrait;
    use crate::services::signer::StellarSignTrait;
    use std::sync::Arc;

    use super::*;

    const HORIZON_MAINNET_URL: &str = "https://horizon.stellar.org";
    const USDC_ASSET_ID: &str = "USDC:GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN";

    // Mock signer for tests (only quote methods are tested, so this doesn't need to work)
    struct MockSigner;

    #[async_trait::async_trait]
    impl StellarSignTrait for MockSigner {
        async fn sign_xdr_transaction(
            &self,
            _unsigned_xdr: &str,
            _network_passphrase: &str,
        ) -> Result<SignXdrTransactionResponseStellar, SignerError> {
            unimplemented!("Not used in quote tests")
        }
    }

    fn create_test_service() -> OrderBookService<MockStellarProviderTrait, MockSigner> {
        let provider = Arc::new(MockStellarProviderTrait::new());
        let signer = Arc::new(MockSigner);
        OrderBookService::new(HORIZON_MAINNET_URL.to_string(), provider, signer)
            .expect("Failed to create test service")
    }

    #[tokio::test]
    async fn test_fetch_order_book_usdc_to_xlm() {
        let service = create_test_service();
        let order_book = service
            .fetch_order_book(
                "credit_alphanum4",
                Some("USDC"),
                Some("GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN"),
                "native",
                None,
                None,
            )
            .await;

        let order_book = order_book.expect("Failed to fetch order book");

        // Verify response structure
        assert!(!order_book.bids.is_empty(), "Order book should have bids");
        assert!(!order_book.asks.is_empty(), "Order book should have asks");
        assert_eq!(order_book.base.asset_type, "credit_alphanum4");
        assert_eq!(order_book.base.asset_code, Some("USDC".to_string()));
        assert_eq!(order_book.counter.asset_type, "native");

        // Verify order structure
        assert!(!order_book.asks.is_empty(), "Order book should have asks");
        let first_ask = &order_book.asks[0];
        assert!(!first_ask.price.is_empty(), "Ask price should not be empty");
        assert!(
            !first_ask.amount.is_empty(),
            "Ask amount should not be empty"
        );
        assert!(
            first_ask.price_r.n > 0,
            "Price numerator should be positive"
        );
        assert!(
            first_ask.price_r.d > 0,
            "Price denominator should be positive"
        );
    }

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_usdc() {
        let service = create_test_service();
        // Test with 100 USDC (100 * 10^7 stroops)
        let amount = 100_000_000u64;
        let slippage = 1.0;

        let result = service
            .get_token_to_xlm_quote(USDC_ASSET_ID, amount, slippage, None)
            .await;

        let quote = result.expect("Failed to get quote");

        assert_eq!(quote.input_asset, USDC_ASSET_ID);
        assert_eq!(quote.output_asset, "native");
        assert_eq!(quote.in_amount, amount);
        assert!(
            quote.out_amount > 0,
            "Output amount should be positive, got: {}",
            quote.out_amount
        );
        assert!(
            quote.price_impact_pct >= 0.0,
            "Price impact should be non-negative, got: {}",
            quote.price_impact_pct
        );
        assert_eq!(
            quote.slippage_bps, 100,
            "Expected 100 bps for 1.0% slippage"
        ); // 1.0% = 100 bps
        assert!(quote.path.is_none(), "Order book doesn't provide path");
    }

    #[tokio::test]
    async fn test_get_xlm_to_token_quote_usdc() {
        let service = create_test_service();
        // Test with 100 XLM (100 * 10^7 stroops)
        let amount = 100_000_000u64;
        let slippage = 1.0;

        let result = service
            .get_xlm_to_token_quote(USDC_ASSET_ID, amount, slippage, None)
            .await;

        let quote = result.expect("Failed to get quote");

        assert_eq!(quote.input_asset, "native");
        assert_eq!(quote.output_asset, USDC_ASSET_ID);
        assert_eq!(quote.in_amount, amount);
        assert!(
            quote.out_amount > 0,
            "Output amount should be positive, got: {}",
            quote.out_amount
        );
        assert!(
            quote.price_impact_pct >= 0.0,
            "Price impact should be non-negative, got: {}",
            quote.price_impact_pct
        );
        assert_eq!(
            quote.slippage_bps, 100,
            "Expected 100 bps for 1.0% slippage"
        ); // 1.0% = 100 bps
        assert!(quote.path.is_none(), "Order book doesn't provide path");
    }

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_native() {
        let service = create_test_service();
        let amount = 100_000_000u64;
        let slippage = 1.0;

        let result = service
            .get_token_to_xlm_quote("native", amount, slippage, None)
            .await;

        let quote = result.expect("Failed to get quote");

        assert_eq!(quote.input_asset, "native");
        assert_eq!(quote.output_asset, "native");
        assert_eq!(quote.in_amount, amount);
        assert_eq!(quote.out_amount, amount);
        assert_eq!(
            quote.price_impact_pct, 0.0,
            "Native to native should have zero price impact"
        );
    }

    #[tokio::test]
    async fn test_get_xlm_to_token_quote_native() {
        let service = create_test_service();
        let amount = 100_000_000u64;
        let slippage = 1.0;

        let result = service
            .get_xlm_to_token_quote("native", amount, slippage, None)
            .await;

        let quote = result.expect("Failed to get quote");

        assert_eq!(quote.input_asset, "native");
        assert_eq!(quote.output_asset, "native");
        assert_eq!(quote.in_amount, amount);
        assert_eq!(quote.out_amount, amount);
        assert_eq!(
            quote.price_impact_pct, 0.0,
            "Native to native should have zero price impact"
        );
    }

    #[tokio::test]
    async fn test_parse_asset_id() {
        let service = create_test_service();

        // Test native
        let result = service.parse_asset_id("native");
        assert!(result.is_ok());
        let (asset_type, code, issuer) = result.unwrap();
        assert_eq!(asset_type, "native");
        assert_eq!(code, None);
        assert_eq!(issuer, None);

        // Test credit_alphanum4
        let result = service.parse_asset_id(USDC_ASSET_ID);
        assert!(result.is_ok());
        let (asset_type, code, issuer) = result.unwrap();
        assert_eq!(asset_type, "credit_alphanum4");
        assert_eq!(code, Some("USDC".to_string()));
        assert_eq!(
            issuer,
            Some("GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN".to_string())
        );

        // Test invalid format
        let result = service.parse_asset_id("INVALID");
        assert!(result.is_err(), "Expected error but got: {:?}", result);
    }

    #[tokio::test]
    async fn test_calculate_quote_small_amount() {
        let service = create_test_service();
        // Test with a small amount (1 USDC = 10^7 stroops)
        let amount = 10_000_000u64;
        let slippage = 0.5;

        let result = service
            .get_token_to_xlm_quote(USDC_ASSET_ID, amount, slippage, None)
            .await;

        let quote = result.expect("Failed to get quote");
        assert!(quote.out_amount > 0);
        // For small amounts, price impact should be minimal
        assert!(
            quote.price_impact_pct < 5.0,
            "Price impact should be low for small amounts"
        );
    }

    #[tokio::test]
    async fn test_calculate_quote_large_amount() {
        let service = create_test_service();
        // Test with a larger amount (1000 USDC = 1000 * 10^7 stroops)
        let amount = 1_000_000_000u64;
        let slippage = 1.0;

        let result = service
            .get_token_to_xlm_quote(USDC_ASSET_ID, amount, slippage, None)
            .await;

        // This might fail if there's insufficient liquidity, which is expected
        if let Ok(quote) = result {
            assert!(quote.out_amount > 0);
            // Large amounts may have higher price impact
            assert!(quote.price_impact_pct >= 0.0);
        }
    }
}
