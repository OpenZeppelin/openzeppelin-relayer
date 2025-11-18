//! Stellar DEX service module
//! Provides quote conversion services for Stellar tokens to XLM
//! Supports native Stellar paths API and optional Soroswap integration

mod order_book_service;
pub use order_book_service::OrderBookService;

use async_trait::async_trait;
#[cfg(test)]
use mockall::automock;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum StellarDexServiceError {
    #[error("HTTP request failed: {0}")]
    HttpRequestError(#[from] reqwest::Error),
    #[error("API returned an error: {message}")]
    ApiError { message: String },
    #[error("Failed to deserialize response: {0}")]
    DeserializationError(#[from] serde_json::Error),
    #[error("Invalid asset identifier: {0}")]
    InvalidAssetIdentifier(String),
    #[error("No path found for conversion")]
    NoPathFound,
    #[error("An unknown error occurred: {0}")]
    UnknownError(String),
}

/// Quote response from DEX service
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct StellarQuoteResponse {
    /// Input asset identifier (e.g., "native" or "USDC:GA5Z...")
    pub input_asset: String,
    /// Output asset identifier (typically "native" for XLM)
    pub output_asset: String,
    /// Input amount in stroops
    pub in_amount: u64,
    /// Output amount in stroops
    pub out_amount: u64,
    /// Price impact percentage
    pub price_impact_pct: f64,
    /// Slippage in basis points
    pub slippage_bps: u32,
    /// Path information (optional route details)
    pub path: Option<Vec<PathStep>>,
}

/// Path step in a conversion route
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct PathStep {
    /// Asset code
    pub asset_code: Option<String>,
    /// Asset issuer
    pub asset_issuer: Option<String>,
    /// Amount in this step
    pub amount: u64,
}

/// Parameters for executing a swap transaction
#[derive(Debug, Clone)]
pub struct SwapTransactionParams {
    /// Source account address (the account that will sign the transaction)
    pub source_account: String,
    /// Source asset identifier (e.g., "native" or "USDC:GA5Z...")
    pub source_asset: String,
    /// Destination asset identifier (typically "native" for XLM)
    pub destination_asset: String,
    /// Amount in stroops to swap
    pub amount: u64,
    /// Slippage percentage (e.g., 1.0 for 1%)
    pub slippage_percent: f32,
    /// Sequence number for the transaction
    pub sequence_number: i64,
    /// Network passphrase for signing the transaction
    pub network_passphrase: String,
    /// Source asset decimals (defaults to 7 for native XLM)
    pub source_asset_decimals: Option<u8>,
    /// Destination asset decimals (defaults to 7 for native XLM)
    pub destination_asset_decimals: Option<u8>,
}

/// Result of executing a swap transaction
#[derive(Debug, Clone)]
pub struct SwapExecutionResult {
    /// Transaction hash of the submitted swap transaction
    pub transaction_hash: String,
    /// Destination amount received (in stroops)
    pub destination_amount: u64,
}

/// Trait for Stellar DEX services
#[async_trait]
#[cfg_attr(test, automock)]
pub trait StellarDexServiceTrait: Send + Sync {
    /// Get a quote for converting a token to XLM
    ///
    /// # Arguments
    ///
    /// * `asset_id` - Asset identifier (e.g., "native" for XLM, or "USDC:GA5Z..." for credit assets)
    /// * `amount` - Amount in stroops to convert
    /// * `slippage` - Slippage percentage (e.g., 1.0 for 1%)
    /// * `asset_decimals` - Number of decimal places for the asset (defaults to 7 if None)
    ///
    /// # Returns
    ///
    /// A quote response with conversion details
    async fn get_token_to_xlm_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
        asset_decimals: Option<u8>,
    ) -> Result<StellarQuoteResponse, StellarDexServiceError>;

    /// Get a quote for converting XLM to a token
    ///
    /// # Arguments
    ///
    /// * `asset_id` - Target asset identifier
    /// * `amount` - Amount in stroops of XLM to convert
    /// * `slippage` - Slippage percentage
    /// * `asset_decimals` - Number of decimal places for the asset (defaults to 7 if None)
    ///
    /// # Returns
    ///
    /// A quote response with conversion details
    async fn get_xlm_to_token_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
        asset_decimals: Option<u8>,
    ) -> Result<StellarQuoteResponse, StellarDexServiceError>;

    /// Execute a swap transaction
    ///
    /// This method creates, signs, and submits a Stellar transaction with a path payment
    /// operation based on the quote from `get_token_to_xlm_quote` or `get_xlm_to_token_quote`.
    ///
    /// # Arguments
    ///
    /// * `params` - Swap transaction parameters including source account, assets, amounts, sequence number, and network passphrase
    ///
    /// # Returns
    ///
    /// A `SwapExecutionResult` containing the transaction hash and destination amount received
    async fn execute_swap(
        &self,
        params: SwapTransactionParams,
    ) -> Result<SwapExecutionResult, StellarDexServiceError>;
}

/// Default implementation using Stellar Order Book service
/// Note: This type alias cannot be used directly due to generic parameters.
/// Use `OrderBookService<P, S>` where P implements `StellarProviderTrait` and S implements `StellarSignTrait`.
pub type DefaultStellarDexService<P, S> = OrderBookService<P, S>;
