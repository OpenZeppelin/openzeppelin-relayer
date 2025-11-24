//! Stellar DEX Service implementation
//! Uses Stellar Horizon API `/paths/strict-send` for ALL swaps (Sell Logic).
//! This unifies the Transaction Builder logic and guarantees input amounts.
//! Provides better rates by considering liquidity pools (AMMs) and multi-hop paths

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
    Asset, Limits, Memo, Operation, OperationBody, PathPaymentStrictSendOp, Preconditions, ReadXdr,
    SequenceNumber, TimeBounds, TimePoint, Transaction, TransactionEnvelope, TransactionExt,
    TransactionV1Envelope, VecM, WriteXdr,
};
use std::convert::TryFrom;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info};

/// Transaction validity window in minutes
const TRANSACTION_VALIDITY_MINUTES: i64 = 5;

/// HTTP request timeout in seconds
const HTTP_REQUEST_TIMEOUT_SECONDS: u64 = 7;

/// Slippage percentage multiplier for basis points conversion
const SLIPPAGE_TO_BPS_MULTIPLIER: f32 = 100.0;

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
#[allow(dead_code)]
struct PathRecord {
    source_amount: String,
    destination_amount: String,
    path: Vec<PathAsset>,
}

#[derive(Debug, Deserialize)]
struct PathAsset {
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

    /// Parse asset identifier to AssetSpec for XDR conversion
    /// Handles both native and non-native assets
    ///
    /// # Arguments
    ///
    /// * `asset_id` - Asset identifier (e.g., "native", "USDC:GA5Z...")
    ///
    /// # Returns
    ///
    /// AssetSpec that can be converted to XDR Asset
    fn parse_asset_to_spec(&self, asset_id: &str) -> Result<AssetSpec, StellarDexServiceError> {
        if asset_id == "native" || asset_id.is_empty() {
            return Ok(AssetSpec::Native);
        }

        let (code, issuer) = asset_id.split_once(':').ok_or_else(|| {
            StellarDexServiceError::InvalidAssetIdentifier(format!(
                "Invalid asset format. Expected 'native' or 'CODE:ISSUER', got: {asset_id}"
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
                "Asset code too long (max 12 characters): {code}",
            )))
        }
    }

    /// Helper to convert stroops to decimal string
    fn to_decimal_string(&self, stroops: u64, decimals: u8) -> String {
        let s = stroops.to_string();
        let decimals_usize = decimals as usize;

        if s.len() <= decimals_usize {
            format!("0.{s:0>decimals_usize$}")
        } else {
            let split = s.len() - decimals_usize;
            format!("{}.{}", &s[..split], &s[split..])
        }
    }

    /// Helper to safely parse decimal string to stroops without f64
    /// e.g., "123.4567890" (decimals 7) -> 1234567890
    /// This avoids IEEE 754 floating-point precision errors
    /// Handles edge cases: "100." (trailing decimal), "100" (no decimal)
    fn parse_string_amount_to_stroops(
        &self,
        amount_str: &str,
        decimals: u8,
    ) -> Result<u64, StellarDexServiceError> {
        let parts: Vec<&str> = amount_str.split('.').collect();

        // Parse integer part into u128 for safe arithmetic
        let int_part = parts[0].parse::<u128>().map_err(|_| {
            StellarDexServiceError::UnknownError(format!("Invalid amount string: {amount_str}"))
        })?;

        // Compute multiplier as u128 to avoid overflow
        let multiplier = 10u128.pow(decimals as u32);

        // Calculate integer part contribution in stroops (u128)
        let mut stroops = int_part.checked_mul(multiplier).ok_or_else(|| {
            StellarDexServiceError::UnknownError(format!(
                "Amount overflow: integer part too large for {decimals} decimals: {amount_str}",
            ))
        })?;

        // Check if we have a decimal part AND it's not empty (handles "100." case)
        if parts.len() > 1 && !parts[1].is_empty() {
            let fraction_str = parts[1];
            let frac_parsed = fraction_str.parse::<u128>().map_err(|_| {
                StellarDexServiceError::UnknownError(format!("Invalid fraction: {amount_str}"))
            })?;

            let frac_len = fraction_str.len() as u32;
            let target_decimals = decimals as u32;

            let frac_stroops = if frac_len > target_decimals {
                // Truncate if too many decimals (e.g. 1.12345678 -> 7 decimals -> 1.1234567)
                // Compute divisor as u128
                let divisor = 10u128.pow(frac_len - target_decimals);
                frac_parsed / divisor
            } else {
                // Pad if fewer decimals (e.g. 1.12 -> 7 decimals -> 1.1200000)
                // Compute padding multiplier as u128
                let padding_multiplier = 10u128.pow(target_decimals - frac_len);
                frac_parsed.checked_mul(padding_multiplier).ok_or_else(|| {
                    StellarDexServiceError::UnknownError(format!(
                        "Amount overflow: fraction padding overflow: {amount_str}"
                    ))
                })?
            };

            // Add fraction contribution to total (u128)
            stroops = stroops.checked_add(frac_stroops).ok_or_else(|| {
                StellarDexServiceError::UnknownError(format!(
                    "Amount overflow: total exceeds u128 maximum: {amount_str}"
                ))
            })?;
        }

        // Check if final total fits into u64 before casting
        if stroops > u64::MAX as u128 {
            return Err(StellarDexServiceError::UnknownError(format!(
                "Amount overflow: value {} exceeds u64 maximum ({}): {amount_str}",
                stroops,
                u64::MAX
            )));
        }

        Ok(stroops as u64)
    }

    /// Fetch strict-send paths from Horizon
    /// strict-send = "I am sending exactly X source asset, how much Y dest can I get?"
    async fn fetch_strict_send_paths(
        &self,
        source_asset: &AssetSpec,
        source_amount_stroops: u64,
        source_decimals: u8,
        destination_asset: &AssetSpec,
        _destination_account: &str,
    ) -> Result<Vec<PathRecord>, StellarDexServiceError> {
        let mut url = format!("{}/paths/strict-send", self.horizon_base_url);

        // Convert stroops to decimal string for the API using source asset decimals
        let amount_decimal = self.to_decimal_string(source_amount_stroops, source_decimals);

        let mut params = vec![format!("source_amount={}", amount_decimal)];

        // Add Source Asset Params
        match source_asset {
            AssetSpec::Native => {
                params.push("source_asset_type=native".to_string());
            }
            AssetSpec::Credit4 { code, issuer } => {
                params.push("source_asset_type=credit_alphanum4".to_string());
                params.push(format!("source_asset_code={code}"));
                params.push(format!("source_asset_issuer={issuer}"));
            }
            AssetSpec::Credit12 { code, issuer } => {
                params.push("source_asset_type=credit_alphanum12".to_string());
                params.push(format!("source_asset_code={code}"));
                params.push(format!("source_asset_issuer={issuer}"));
            }
        }

        // Add Destination Asset Params
        // Note: Horizon API doesn't allow both destination_account and destination_assets
        // We use destination_account to check trustlines, but specify the asset separately
        match destination_asset {
            AssetSpec::Native => {
                params.push("destination_assets=native".to_string());
            }
            AssetSpec::Credit4 { code, issuer } => {
                params.push(format!("destination_assets={code}:{issuer}"));
            }
            AssetSpec::Credit12 { code, issuer } => {
                params.push(format!("destination_assets={code}:{issuer}"));
            }
        }

        // Add destination_account as a separate parameter (Horizon will filter by trustlines)
        // Actually, Horizon doesn't support both. We'll omit destination_account when we have a specific asset.
        // The path finder will still work, but won't check trustlines automatically.
        // For trustline checking, the caller should verify separately.

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
                message: format!("Horizon API returned error {status}: {error_text}"),
            });
        }

        let path_response: PathResponse = response.json().await.map_err(|e| {
            StellarDexServiceError::UnknownError(format!("Failed to deserialize paths: {e}"))
        })?;

        Ok(path_response.embedded.records)
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
    /// Supports both Token->XLM and XLM->Token swaps
    async fn get_swap_quote(
        &self,
        params: &SwapTransactionParams,
    ) -> Result<StellarQuoteResponse, StellarDexServiceError> {
        if params.destination_asset == "native" {
            // Token -> XLM: Sell token, receive XLM
            self.get_token_to_xlm_quote(
                &params.source_asset,
                params.amount,
                params.slippage_percent,
                params.source_asset_decimals,
            )
            .await
        } else if params.source_asset == "native" {
            // XLM -> Token: Sell XLM, receive token
            self.get_xlm_to_token_quote(
                &params.destination_asset,
                params.amount,
                params.slippage_percent,
                params.destination_asset_decimals,
            )
            .await
        } else {
            Err(StellarDexServiceError::UnknownError(
                "Only swaps involving native XLM are currently supported".to_string(),
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
            StellarDexServiceError::InvalidAssetIdentifier(format!("Invalid source account: {e}"))
        })?;

        // Parse source asset to Asset using shared helper
        let source_spec = self.parse_asset_to_spec(&params.source_asset)?;
        let send_asset = Asset::try_from(source_spec).map_err(|e| {
            StellarDexServiceError::InvalidAssetIdentifier(format!(
                "Failed to convert source asset: {e}",
            ))
        })?;

        // Parse destination asset dynamically (supports both native and tokens)
        let dest_spec = self.parse_asset_to_spec(&params.destination_asset)?;
        let dest_asset = Asset::try_from(dest_spec).map_err(|e| {
            StellarDexServiceError::InvalidAssetIdentifier(format!(
                "Failed to convert destination asset: {e}",
            ))
        })?;

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
            .ok_or_else(|| {
                StellarDexServiceError::UnknownError("Overflow calculating min amount".into())
            })?
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
                    let asset_spec =
                        if let (Some(code), Some(issuer)) = (&p.asset_code, &p.asset_issuer) {
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
                            "Failed to convert path step to XDR asset: {e}",
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };

        // Convert Vec<Asset> to VecM<Asset, 5> (Stellar supports up to 5 intermediate assets in a path)
        let path_vecm: VecM<Asset, 5> = path_assets.try_into().map_err(|_| {
            StellarDexServiceError::UnknownError(
                "Failed to convert path to VecM (path too long, max 5 assets)".to_string(),
            )
        })?;

        // Create PathPaymentStrictSend operation
        let path_payment_op = Operation {
            source_account: None,
            body: OperationBody::PathPaymentStrictSend(PathPaymentStrictSendOp {
                send_asset,
                send_amount, // We strictly send this amount
                destination: source_account.clone(),
                dest_asset,
                dest_min,        // We accept at least this much
                path: path_vecm, // Populate the path here!
            }),
        };

        // Set time bounds: valid immediately (min_time = 0) until configured validity window
        // Using min_time = 0 prevents TxTooEarly errors when transaction goes through pipeline
        let now = Utc::now();
        let valid_until = now + ChronoDuration::minutes(TRANSACTION_VALIDITY_MINUTES);
        let time_bounds = TimeBounds {
            min_time: TimePoint(0), // Valid immediately, prevents TxTooEarly errors
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
                "Failed to serialize transaction to XDR: {e}"
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
                StellarDexServiceError::UnknownError(format!("Failed to sign transaction: {e}"))
            })?;

        debug!("Transaction signed successfully, submitting to network");

        // Parse the signed XDR to get the envelope
        let signed_envelope =
            TransactionEnvelope::from_xdr_base64(&signed_response.signed_xdr, Limits::none())
                .map_err(|e| {
                    StellarDexServiceError::UnknownError(format!("Failed to parse signed XDR: {e}"))
                })?;

        // Send the transaction
        self.provider
            .send_transaction(&signed_envelope)
            .await
            .map_err(|e| {
                StellarDexServiceError::UnknownError(format!("Failed to send transaction: {e}"))
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
        asset_decimals: Option<u8>,
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
        let signer_address = match self.signer.address().await.map_err(|e| {
            StellarDexServiceError::UnknownError(format!("Failed to get signer address: {e}"))
        })? {
            Address::Stellar(addr) => addr,
            _ => {
                return Err(StellarDexServiceError::UnknownError(
                    "Signer address is not a Stellar address".to_string(),
                ))
            }
        };

        // Fetch paths using strict-send path finding
        // Use source asset decimals (default to 7 for tokens if not provided)
        let source_decimals = asset_decimals.unwrap_or(7);
        let paths = self
            .fetch_strict_send_paths(
                &source_spec,
                amount,
                source_decimals,
                &dest_spec,
                &signer_address,
            )
            .await?;

        if paths.is_empty() {
            return Err(StellarDexServiceError::NoPathFound);
        }

        // The paths are typically sorted by best price (highest destination_amount)
        let best_path = &paths[0];

        // SAFE PARSING: Use integer-based parsing to avoid IEEE 754 floating-point errors
        // Horizon returns strings like "123.4567890" - parsing as f64 can introduce precision errors
        let out_amount = self.parse_string_amount_to_stroops(&best_path.destination_amount, 7)?;

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
            slippage_bps: (slippage * SLIPPAGE_TO_BPS_MULTIPLIER) as u32,
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
        let signer_address = match self.signer.address().await.map_err(|e| {
            StellarDexServiceError::UnknownError(format!("Failed to get signer address: {e}"))
        })? {
            Address::Stellar(addr) => addr,
            _ => {
                return Err(StellarDexServiceError::UnknownError(
                    "Signer address is not a Stellar address".to_string(),
                ))
            }
        };

        // REFACTORED: Use strict-send (Sell XLM) instead of strict-receive
        // This unifies the transaction builder logic and guarantees input amounts
        // We are selling exactly `amount` XLM (which has 7 decimals)
        let paths = self
            .fetch_strict_send_paths(&source_spec, amount, 7, &dest_spec, &signer_address)
            .await?;

        if paths.is_empty() {
            return Err(StellarDexServiceError::NoPathFound);
        }

        // The paths are typically sorted by best price (highest destination_amount)
        let best_path = &paths[0];

        // SAFE PARSING: Use integer-based parsing to avoid IEEE 754 floating-point errors
        // Horizon returns strings like "123.4567890" - parsing as f64 can introduce precision errors
        // Parse destination amount (token) using its specific decimals
        let out_amount =
            self.parse_string_amount_to_stroops(&best_path.destination_amount, decimals)?;

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
            in_amount: amount, // We are selling exactly this amount of XLM
            out_amount,        // We receive this amount of tokens
            price_impact_pct,
            slippage_bps: (slippage * SLIPPAGE_TO_BPS_MULTIPLIER) as u32,
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
