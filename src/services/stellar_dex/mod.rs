//! Stellar DEX service module
//! Provides quote conversion services for Stellar tokens to XLM
//! Supports native Stellar paths API and optional Soroswap integration

mod order_book_service;
mod stellar_dex_service;

pub use order_book_service::OrderBookService;
pub use stellar_dex_service::{DexServiceWrapper, StellarDexService};

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

/// Asset types supported by DEX services
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssetType {
    /// Native XLM
    Native,
    /// Classic assets (CODE:ISSUER format)
    Classic,
    /// Soroban contract tokens (C... format)
    Contract,
}

/// Trait for Stellar DEX services
#[async_trait]
#[cfg_attr(test, automock)]
pub trait StellarDexServiceTrait: Send + Sync {
    /// Returns the asset types this DEX service can handle
    ///
    /// # Returns
    /// A set of supported asset types (Native, Classic, Contract, or combinations)
    fn supported_asset_types(&self) -> std::collections::HashSet<AssetType>;

    /// Checks if this service can handle a specific asset
    ///
    /// # Arguments
    /// * `asset_id` - Asset identifier (e.g., "native", "USDC:GA5Z...", "C...")
    ///
    /// # Returns
    /// True if this service can process swaps for this asset
    fn can_handle_asset(&self, asset_id: &str) -> bool {
        let supported = self.supported_asset_types();

        // Check native
        if (asset_id == "native" || asset_id.is_empty()) && supported.contains(&AssetType::Native) {
            return true;
        }

        // Check contract (C... format, 56 chars)
        if asset_id.starts_with('C')
            && asset_id.len() == 56
            && !asset_id.contains(':')
            && stellar_strkey::Contract::from_string(asset_id).is_ok()
            && supported.contains(&AssetType::Contract)
        {
            return true;
        }

        // Check classic (CODE:ISSUER format)
        if asset_id.contains(':') && supported.contains(&AssetType::Classic) {
            return true;
        }

        false
    }

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

    /// Prepare a swap transaction (get quote and build XDR) without executing it
    ///
    /// This method creates an unsigned transaction XDR for a swap operation.
    /// The transaction can then be queued for background processing.
    ///
    /// # Arguments
    ///
    /// * `params` - Swap transaction parameters including source account, assets, amounts, sequence number, and network passphrase
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// * `String` - Unsigned transaction XDR (base64 encoded)
    /// * `StellarQuoteResponse` - Quote information including destination amount
    async fn prepare_swap_transaction(
        &self,
        params: SwapTransactionParams,
    ) -> Result<(String, StellarQuoteResponse), StellarDexServiceError>;

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
