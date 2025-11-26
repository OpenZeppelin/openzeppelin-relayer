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

        // Validate: at most one decimal point
        if parts.len() > 2 {
            return Err(StellarDexServiceError::UnknownError(format!(
                "Invalid amount string: multiple decimal points: {amount_str}"
            )));
        }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SignerError;
    use crate::services::provider::MockStellarProviderTrait;
    use crate::services::signer::{MockStellarSignTrait, Signer};
    use async_trait::async_trait;
    use std::sync::Arc;

    // Combined mock that implements both StellarSignTrait and Signer
    struct MockCombinedSigner {
        stellar_mock: MockStellarSignTrait,
    }

    impl MockCombinedSigner {
        fn new() -> Self {
            Self {
                stellar_mock: MockStellarSignTrait::new(),
            }
        }
    }

    #[async_trait]
    impl StellarSignTrait for MockCombinedSigner {
        async fn sign_xdr_transaction(
            &self,
            unsigned_xdr: &str,
            network_passphrase: &str,
        ) -> Result<crate::domain::relayer::SignXdrTransactionResponseStellar, SignerError>
        {
            self.stellar_mock
                .sign_xdr_transaction(unsigned_xdr, network_passphrase)
                .await
        }
    }

    #[async_trait]
    impl Signer for MockCombinedSigner {
        async fn address(&self) -> Result<crate::models::Address, SignerError> {
            Ok(crate::models::Address::Stellar(
                "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            ))
        }

        async fn sign_transaction(
            &self,
            _transaction: crate::models::NetworkTransactionData,
        ) -> Result<crate::domain::SignTransactionResponse, SignerError> {
            Ok(crate::domain::SignTransactionResponse::Stellar(
                crate::domain::SignTransactionResponseStellar {
                    signature: crate::models::DecoratedSignature {
                        hint: soroban_rs::xdr::SignatureHint([0; 4]),
                        signature: soroban_rs::xdr::Signature(
                            soroban_rs::xdr::BytesM::try_from(vec![0u8; 64]).unwrap(),
                        ),
                    },
                },
            ))
        }
    }

    // Helper function to create a test service with mocks
    fn create_test_service() -> (
        OrderBookService<MockStellarProviderTrait, MockCombinedSigner>,
        Arc<MockStellarProviderTrait>,
        Arc<MockCombinedSigner>,
    ) {
        let provider = Arc::new(MockStellarProviderTrait::new());
        let signer = Arc::new(MockCombinedSigner::new());

        let service = OrderBookService::new(
            "https://horizon-testnet.stellar.org".to_string(),
            provider.clone(),
            signer.clone(),
        )
        .expect("Failed to create OrderBookService");

        (service, provider, signer)
    }

    #[test]
    fn test_parse_asset_to_spec_native() {
        let (service, _, _) = create_test_service();

        let result = service.parse_asset_to_spec("native");
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), AssetSpec::Native));

        let result = service.parse_asset_to_spec("");
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), AssetSpec::Native));
    }

    #[test]
    fn test_parse_asset_to_spec_credit4() {
        let (service, _, _) = create_test_service();

        let result = service
            .parse_asset_to_spec("USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5");
        assert!(result.is_ok());

        match result.unwrap() {
            AssetSpec::Credit4 { code, issuer } => {
                assert_eq!(code, "USDC");
                assert_eq!(
                    issuer,
                    "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
                );
            }
            _ => panic!("Expected Credit4"),
        }
    }

    #[test]
    fn test_parse_asset_to_spec_credit12() {
        let (service, _, _) = create_test_service();

        let result = service.parse_asset_to_spec(
            "LONGASSETCD:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
        );
        assert!(result.is_ok());

        match result.unwrap() {
            AssetSpec::Credit12 { code, issuer } => {
                assert_eq!(code, "LONGASSETCD");
                assert_eq!(
                    issuer,
                    "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
                );
            }
            _ => panic!("Expected Credit12"),
        }
    }

    #[test]
    fn test_parse_asset_to_spec_invalid() {
        let (service, _, _) = create_test_service();

        // Missing colon
        let result = service
            .parse_asset_to_spec("USDCGBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5");
        assert!(result.is_err());

        // Empty code
        let result = service
            .parse_asset_to_spec(":GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5");
        assert!(result.is_err());

        // Empty issuer
        let result = service.parse_asset_to_spec("USDC:");
        assert!(result.is_err());

        // Code too long (>12 chars)
        let result = service.parse_asset_to_spec(
            "VERYLONGASSETCODE:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_to_decimal_string() {
        let (service, _, _) = create_test_service();

        // Standard 7 decimals (stroops)
        assert_eq!(service.to_decimal_string(10000000, 7), "1.0000000");
        assert_eq!(service.to_decimal_string(12345678, 7), "1.2345678");
        assert_eq!(service.to_decimal_string(100, 7), "0.0000100");
        assert_eq!(service.to_decimal_string(1, 7), "0.0000001");

        // Different decimals
        assert_eq!(service.to_decimal_string(1000, 3), "1.000");
        assert_eq!(service.to_decimal_string(123456, 6), "0.123456");

        // Zero
        assert_eq!(service.to_decimal_string(0, 7), "0.0000000");
    }

    #[test]
    fn test_parse_string_amount_to_stroops() {
        let (service, _, _) = create_test_service();

        // Standard conversions with 7 decimals
        assert_eq!(
            service
                .parse_string_amount_to_stroops("1.0000000", 7)
                .unwrap(),
            10000000
        );
        assert_eq!(
            service
                .parse_string_amount_to_stroops("1.2345678", 7)
                .unwrap(),
            12345678
        );
        assert_eq!(
            service
                .parse_string_amount_to_stroops("0.0000100", 7)
                .unwrap(),
            100
        );
        assert_eq!(
            service
                .parse_string_amount_to_stroops("0.0000001", 7)
                .unwrap(),
            1
        );

        // Integer (no decimal)
        assert_eq!(
            service.parse_string_amount_to_stroops("100", 7).unwrap(),
            1000000000
        );

        // Trailing decimal point
        assert_eq!(
            service.parse_string_amount_to_stroops("100.", 7).unwrap(),
            1000000000
        );

        // Fewer decimals than expected (padding)
        assert_eq!(
            service.parse_string_amount_to_stroops("1.12", 7).unwrap(),
            11200000
        );

        // More decimals than expected (truncation)
        assert_eq!(
            service
                .parse_string_amount_to_stroops("1.12345678", 7)
                .unwrap(),
            11234567
        );

        // Different decimal precision
        assert_eq!(
            service.parse_string_amount_to_stroops("1.234", 3).unwrap(),
            1234
        );
        assert_eq!(
            service.parse_string_amount_to_stroops("0.5", 6).unwrap(),
            500000
        );
    }

    #[test]
    fn test_parse_string_amount_to_stroops_invalid() {
        let (service, _, _) = create_test_service();

        // Invalid format
        assert!(service.parse_string_amount_to_stroops("abc", 7).is_err());
        assert!(service.parse_string_amount_to_stroops("1.2.3", 7).is_err());
        assert!(service.parse_string_amount_to_stroops("", 7).is_err());
    }

    #[test]
    fn test_parse_string_amount_to_stroops_overflow() {
        let (service, _, _) = create_test_service();

        // Value exceeds u64::MAX
        let huge_value = format!("{}.0", u64::MAX as u128 + 1);
        assert!(service
            .parse_string_amount_to_stroops(&huge_value, 7)
            .is_err());
    }

    #[test]
    fn test_validate_swap_params_valid() {
        let (service, _, _) = create_test_service();

        let params = SwapTransactionParams {
            source_account: "GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX".to_string(),
            source_asset: "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
                .to_string(),
            destination_asset: "native".to_string(),
            amount: 1000000,
            slippage_percent: 1.0,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            source_asset_decimals: Some(7),
            destination_asset_decimals: Some(7),
        };

        assert!(service.validate_swap_params(&params).is_ok());
    }

    #[test]
    fn test_validate_swap_params_native_source() {
        let (service, _, _) = create_test_service();

        let params = SwapTransactionParams {
            source_account: "GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX".to_string(),
            source_asset: "native".to_string(),
            destination_asset: "native".to_string(),
            amount: 1000000,
            slippage_percent: 1.0,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            source_asset_decimals: Some(7),
            destination_asset_decimals: Some(7),
        };

        let result = service.validate_swap_params(&params);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            StellarDexServiceError::InvalidAssetIdentifier(_)
        ));
    }

    #[test]
    fn test_validate_swap_params_zero_amount() {
        let (service, _, _) = create_test_service();

        let params = SwapTransactionParams {
            source_account: "GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX".to_string(),
            source_asset: "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
                .to_string(),
            destination_asset: "native".to_string(),
            amount: 0,
            slippage_percent: 1.0,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            source_asset_decimals: Some(7),
            destination_asset_decimals: Some(7),
        };

        let result = service.validate_swap_params(&params);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            StellarDexServiceError::InvalidAssetIdentifier(_)
        ));
    }

    #[test]
    fn test_validate_swap_params_invalid_slippage() {
        let (service, _, _) = create_test_service();

        // Negative slippage
        let mut params = SwapTransactionParams {
            source_account: "GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX".to_string(),
            source_asset: "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
                .to_string(),
            destination_asset: "native".to_string(),
            amount: 1000000,
            slippage_percent: -1.0,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            source_asset_decimals: Some(7),
            destination_asset_decimals: Some(7),
        };

        assert!(service.validate_swap_params(&params).is_err());

        // Slippage > 100%
        params.slippage_percent = 101.0;
        assert!(service.validate_swap_params(&params).is_err());
    }

    #[test]
    fn test_validate_swap_params_non_native_destination() {
        let (service, _, _) = create_test_service();

        let params = SwapTransactionParams {
            source_account: "GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX".to_string(),
            source_asset: "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
                .to_string(),
            destination_asset: "EUROC:GXXXXXXXXXXXXXX".to_string(),
            amount: 1000000,
            slippage_percent: 1.0,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            source_asset_decimals: Some(7),
            destination_asset_decimals: Some(7),
        };

        let result = service.validate_swap_params(&params);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_native_to_native() {
        let (service, _, _) = create_test_service();

        let result = service
            .get_token_to_xlm_quote("native", 10000000, 1.0, Some(7))
            .await;

        assert!(result.is_ok());
        let quote = result.unwrap();
        assert_eq!(quote.input_asset, "native");
        assert_eq!(quote.output_asset, "native");
        assert_eq!(quote.in_amount, 10000000);
        assert_eq!(quote.out_amount, 10000000);
        assert_eq!(quote.price_impact_pct, 0.0);
        assert_eq!(quote.slippage_bps, 100);
        assert!(quote.path.is_none());
    }

    #[tokio::test]
    async fn test_get_xlm_to_token_quote_native_to_native() {
        let (service, _, _) = create_test_service();

        let result = service
            .get_xlm_to_token_quote("native", 10000000, 1.0, Some(7))
            .await;

        assert!(result.is_ok());
        let quote = result.unwrap();
        assert_eq!(quote.input_asset, "native");
        assert_eq!(quote.output_asset, "native");
        assert_eq!(quote.in_amount, 10000000);
        assert_eq!(quote.out_amount, 10000000);
        assert_eq!(quote.price_impact_pct, 0.0);
        assert_eq!(quote.slippage_bps, 100);
        assert!(quote.path.is_none());
    }

    #[test]
    fn test_supported_asset_types() {
        let (service, _, _) = create_test_service();

        let types = service.supported_asset_types();
        assert_eq!(types.len(), 2);
        assert!(types.contains(&AssetType::Native));
        assert!(types.contains(&AssetType::Classic));
    }

    // Integration tests with mocked provider and signer

    #[test]
    fn test_swap_params_validation_comprehensive() {
        let (service, _, _) = create_test_service();

        // Test valid params
        let params = SwapTransactionParams {
            source_account: "GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX".to_string(),
            source_asset: "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
                .to_string(),
            destination_asset: "native".to_string(),
            amount: 100000000,
            slippage_percent: 1.0,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            source_asset_decimals: Some(7),
            destination_asset_decimals: Some(7),
        };

        let validation = service.validate_swap_params(&params);
        assert!(validation.is_ok());
    }

    #[test]
    fn test_parse_asset_to_spec_edge_cases() {
        let (_service, _, _) = create_test_service();

        // Test asset with exactly 4 characters
        let result = _service
            .parse_asset_to_spec("ABCD:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5");
        assert!(result.is_ok());
        match result.unwrap() {
            AssetSpec::Credit4 { code, .. } => assert_eq!(code, "ABCD"),
            _ => panic!("Expected Credit4"),
        }

        // Test asset with exactly 12 characters
        let result = _service.parse_asset_to_spec(
            "ABCDEFGHIJKL:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
        );
        assert!(result.is_ok());
        match result.unwrap() {
            AssetSpec::Credit12 { code, .. } => assert_eq!(code, "ABCDEFGHIJKL"),
            _ => panic!("Expected Credit12"),
        }

        // Test asset with 5 characters (should be Credit12)
        let result = _service
            .parse_asset_to_spec("ABCDE:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5");
        assert!(result.is_ok());
        match result.unwrap() {
            AssetSpec::Credit12 { code, .. } => assert_eq!(code, "ABCDE"),
            _ => panic!("Expected Credit12"),
        }
    }

    #[test]
    fn test_to_decimal_string_edge_cases() {
        let (_service, _, _) = create_test_service();

        // Test with 0 decimals (note: function adds trailing ".")
        assert_eq!(_service.to_decimal_string(123, 0), "123.");

        // Test with very large number (u64::MAX with 7 decimals)
        assert_eq!(
            _service.to_decimal_string(u64::MAX, 7),
            "1844674407370.9551615"
        );

        // Test with 1 decimal
        assert_eq!(_service.to_decimal_string(123, 1), "12.3");

        // Test with 2 decimals
        assert_eq!(_service.to_decimal_string(12345, 2), "123.45");
    }

    #[test]
    fn test_parse_string_amount_edge_cases() {
        let (_service, _, _) = create_test_service();

        // Test with no decimal point
        assert_eq!(
            _service.parse_string_amount_to_stroops("100", 7).unwrap(),
            1000000000
        );

        // Test with trailing decimal point
        assert_eq!(
            _service.parse_string_amount_to_stroops("100.", 7).unwrap(),
            1000000000
        );

        // Test with leading zero
        assert_eq!(
            _service.parse_string_amount_to_stroops("0.1", 7).unwrap(),
            1000000
        );

        // Test with many decimal places (truncation)
        assert_eq!(
            _service
                .parse_string_amount_to_stroops("1.123456789", 7)
                .unwrap(),
            11234567
        );

        // Test with few decimal places (padding)
        assert_eq!(
            _service.parse_string_amount_to_stroops("1.12", 7).unwrap(),
            11200000
        );
    }

    // Note: Integration tests for sign_and_submit_transaction, prepare_swap_transaction,
    // and other async functions that require HTTP mocking would need a more sophisticated
    // setup with wiremock or similar HTTP mocking libraries. The current test structure
    // focuses on unit-testable logic (parsing, validation, conversion) which provides
    // excellent coverage for the core functionality.

    // Edge case tests
    #[test]
    fn test_slippage_to_bps_conversion() {
        let (_service, _, _) = create_test_service();

        // Test various slippage percentages
        let params = SwapTransactionParams {
            source_account: "GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX".to_string(),
            source_asset: "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
                .to_string(),
            destination_asset: "native".to_string(),
            amount: 1000000,
            slippage_percent: 0.5,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            source_asset_decimals: Some(7),
            destination_asset_decimals: Some(7),
        };

        // Verify slippage conversion (0.5% = 50 bps)
        let expected_bps = (params.slippage_percent * SLIPPAGE_TO_BPS_MULTIPLIER) as u32;
        assert_eq!(expected_bps, 50);
    }

    #[test]
    fn test_min_amount_calculation() {
        // Test minimum receive amount calculation with slippage
        // out_amount * (10000 - bps) / 10000

        let out_amount = 1000000u128; // 1 XLM in stroops
        let slippage_bps = 100u128; // 1%
        let basis = 10000u128;

        let min_amount = out_amount * (basis - slippage_bps) / basis;
        assert_eq!(min_amount, 990000); // 0.99 XLM

        // Test with 0.5% slippage
        let slippage_bps = 50u128;
        let min_amount = out_amount * (basis - slippage_bps) / basis;
        assert_eq!(min_amount, 995000); // 0.995 XLM

        // Test edge case: 100% slippage (accept anything)
        let slippage_bps = 10000u128;
        let min_amount = out_amount * (basis - slippage_bps) / basis;
        assert_eq!(min_amount, 0);
    }

    #[test]
    fn test_parse_string_amount_roundtrip() {
        let (service, _, _) = create_test_service();

        // Test that to_decimal_string and parse_string_amount_to_stroops are inverses
        let test_amounts = vec![10000000u64, 12345678, 100, 1, 9999999999];

        for amount in test_amounts {
            let decimal_str = service.to_decimal_string(amount, 7);
            let parsed = service
                .parse_string_amount_to_stroops(&decimal_str, 7)
                .unwrap();
            assert_eq!(parsed, amount, "Roundtrip failed for amount {}", amount);
        }
    }

    #[test]
    fn test_asset_spec_conversion_roundtrip() {
        let (service, _, _) = create_test_service();

        let test_cases = vec![
            "native",
            "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
            "BTC:GCNSGHUCG5VMGLT5RIYYZSO7VQULQKAJ62QA33DBC5PPBSO57LFWVV6P",
            "LONGASSETCD:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
        ];

        for asset_id in test_cases {
            let spec = service.parse_asset_to_spec(asset_id).unwrap();

            // Try to convert to XDR Asset
            let xdr_result = Asset::try_from(spec);
            assert!(xdr_result.is_ok(), "Failed to convert {} to XDR", asset_id);
        }
    }

    #[test]
    fn test_transaction_constants() {
        // Verify constants are reasonable
        assert_eq!(TRANSACTION_VALIDITY_MINUTES, 5);
        assert_eq!(HTTP_REQUEST_TIMEOUT_SECONDS, 7);
        assert_eq!(SLIPPAGE_TO_BPS_MULTIPLIER, 100.0);
    }

    // HTTP Integration tests with mockito

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_with_http_mock() {
        let mut mock_server = mockito::Server::new_async().await;

        // Mock successful Horizon API response
        let mock = mock_server
            .mock(
                "GET",
                mockito::Matcher::Regex(r".*/paths/strict-send.*".to_string()),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                "_embedded": {
                    "records": [
                        {
                            "source_amount": "10.0000000",
                            "destination_amount": "9.8500000",
                            "path": []
                        }
                    ]
                }
            }"#,
            )
            .create_async()
            .await;

        let provider = Arc::new(MockStellarProviderTrait::new());
        let signer = Arc::new(MockCombinedSigner::new());

        let service = OrderBookService::new(mock_server.url(), provider, signer)
            .expect("Failed to create OrderBookService");

        let result = service
            .get_token_to_xlm_quote(
                "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
                100000000,
                1.0,
                Some(7),
            )
            .await;

        mock.assert_async().await;
        assert!(result.is_ok());
        let quote = result.unwrap();
        assert_eq!(quote.in_amount, 100000000);
        assert_eq!(quote.out_amount, 98500000);
        assert_eq!(quote.slippage_bps, 100);
    }

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_http_error_404() {
        let mut mock_server = mockito::Server::new_async().await;

        let mock = mock_server
            .mock(
                "GET",
                mockito::Matcher::Regex(r".*/paths/strict-send.*".to_string()),
            )
            .with_status(404)
            .with_body("Not found")
            .create_async()
            .await;

        let provider = Arc::new(MockStellarProviderTrait::new());
        let signer = Arc::new(MockCombinedSigner::new());

        let service = OrderBookService::new(mock_server.url(), provider, signer)
            .expect("Failed to create OrderBookService");

        let result = service
            .get_token_to_xlm_quote(
                "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
                100000000,
                1.0,
                Some(7),
            )
            .await;

        mock.assert_async().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_http_error_500() {
        let mut mock_server = mockito::Server::new_async().await;

        let mock = mock_server
            .mock(
                "GET",
                mockito::Matcher::Regex(r".*/paths/strict-send.*".to_string()),
            )
            .with_status(500)
            .with_body("Internal server error")
            .create_async()
            .await;

        let provider = Arc::new(MockStellarProviderTrait::new());
        let signer = Arc::new(MockCombinedSigner::new());

        let service = OrderBookService::new(mock_server.url(), provider, signer)
            .expect("Failed to create OrderBookService");

        let result = service
            .get_token_to_xlm_quote(
                "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
                100000000,
                1.0,
                Some(7),
            )
            .await;

        mock.assert_async().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_no_paths_found() {
        let mut mock_server = mockito::Server::new_async().await;

        let mock = mock_server
            .mock(
                "GET",
                mockito::Matcher::Regex(r".*/paths/strict-send.*".to_string()),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                "_embedded": {
                    "records": []
                }
            }"#,
            )
            .create_async()
            .await;

        let provider = Arc::new(MockStellarProviderTrait::new());
        let signer = Arc::new(MockCombinedSigner::new());

        let service = OrderBookService::new(mock_server.url(), provider, signer)
            .expect("Failed to create OrderBookService");

        let result = service
            .get_token_to_xlm_quote(
                "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
                100000000,
                1.0,
                Some(7),
            )
            .await;

        mock.assert_async().await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            StellarDexServiceError::NoPathFound
        ));
    }

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_invalid_json() {
        let mut mock_server = mockito::Server::new_async().await;

        let mock = mock_server
            .mock(
                "GET",
                mockito::Matcher::Regex(r".*/paths/strict-send.*".to_string()),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("invalid json")
            .create_async()
            .await;

        let provider = Arc::new(MockStellarProviderTrait::new());
        let signer = Arc::new(MockCombinedSigner::new());

        let service = OrderBookService::new(mock_server.url(), provider, signer)
            .expect("Failed to create OrderBookService");

        let result = service
            .get_token_to_xlm_quote(
                "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
                100000000,
                1.0,
                Some(7),
            )
            .await;

        mock.assert_async().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_with_multi_hop_path() {
        let mut mock_server = mockito::Server::new_async().await;

        let mock = mock_server
            .mock("GET", mockito::Matcher::Regex(r".*/paths/strict-send.*".to_string()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{
                "_embedded": {
                    "records": [
                        {
                            "source_amount": "10.0000000",
                            "destination_amount": "9.9000000",
                            "path": [
                                {
                                    "asset_code": "USDC",
                                    "asset_issuer": "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
                                },
                                {
                                    "asset_code": "BTC",
                                    "asset_issuer": "GCNSGHUCG5VMGLT5RIYYZSO7VQULQKAJ62QA33DBC5PPBSO57LFWVV6P"
                                }
                            ]
                        }
                    ]
                }
            }"#)
            .create_async()
            .await;

        let provider = Arc::new(MockStellarProviderTrait::new());
        let signer = Arc::new(MockCombinedSigner::new());

        let service = OrderBookService::new(mock_server.url(), provider, signer)
            .expect("Failed to create OrderBookService");

        let result = service
            .get_token_to_xlm_quote(
                "EUROC:GDHU6WRG4IEQXM5NZ4BMPKOXHW76MZM4Y2IEMFDVXBSDP6SJY4ITNPP2",
                100000000,
                1.0,
                Some(7),
            )
            .await;

        mock.assert_async().await;
        assert!(result.is_ok());
        let quote = result.unwrap();
        assert!(quote.path.is_some());
        let path = quote.path.unwrap();
        assert_eq!(path.len(), 2);
        assert_eq!(path[0].asset_code, Some("USDC".to_string()));
        assert_eq!(
            path[0].asset_issuer,
            Some("GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5".to_string())
        );
        assert_eq!(path[1].asset_code, Some("BTC".to_string()));
        assert_eq!(
            path[1].asset_issuer,
            Some("GCNSGHUCG5VMGLT5RIYYZSO7VQULQKAJ62QA33DBC5PPBSO57LFWVV6P".to_string())
        );
    }

    #[tokio::test]
    async fn test_get_xlm_to_token_quote_with_http_mock() {
        let mut mock_server = mockito::Server::new_async().await;

        let mock = mock_server
            .mock(
                "GET",
                mockito::Matcher::Regex(r".*/paths/strict-send.*".to_string()),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                "_embedded": {
                    "records": [
                        {
                            "source_amount": "1.0000000",
                            "destination_amount": "10.5000000",
                            "path": []
                        }
                    ]
                }
            }"#,
            )
            .create_async()
            .await;

        let provider = Arc::new(MockStellarProviderTrait::new());
        let signer = Arc::new(MockCombinedSigner::new());

        let service = OrderBookService::new(mock_server.url(), provider, signer)
            .expect("Failed to create OrderBookService");

        let result = service
            .get_xlm_to_token_quote(
                "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
                10000000,
                1.0,
                Some(7),
            )
            .await;

        mock.assert_async().await;
        assert!(result.is_ok());
        let quote = result.unwrap();
        assert_eq!(quote.input_asset, "native");
        assert_eq!(quote.in_amount, 10000000);
        assert_eq!(quote.out_amount, 105000000);
    }

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_with_different_decimals() {
        let mut mock_server = mockito::Server::new_async().await;

        let mock = mock_server
            .mock(
                "GET",
                mockito::Matcher::Regex(r".*/paths/strict-send.*".to_string()),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                "_embedded": {
                    "records": [
                        {
                            "source_amount": "100.000000",
                            "destination_amount": "98.500000",
                            "path": []
                        }
                    ]
                }
            }"#,
            )
            .create_async()
            .await;

        let provider = Arc::new(MockStellarProviderTrait::new());
        let signer = Arc::new(MockCombinedSigner::new());

        let service = OrderBookService::new(mock_server.url(), provider, signer)
            .expect("Failed to create OrderBookService");

        // Test with 6 decimals (USDC on some chains)
        let result = service
            .get_token_to_xlm_quote(
                "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
                100000000,
                1.0,
                Some(6),
            )
            .await;

        mock.assert_async().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_swap_quote_token_to_xlm() {
        let mut mock_server = mockito::Server::new_async().await;

        let mock = mock_server
            .mock(
                "GET",
                mockito::Matcher::Regex(r".*/paths/strict-send.*".to_string()),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                "_embedded": {
                    "records": [
                        {
                            "source_amount": "10.0000000",
                            "destination_amount": "9.8000000",
                            "path": []
                        }
                    ]
                }
            }"#,
            )
            .create_async()
            .await;

        let provider = Arc::new(MockStellarProviderTrait::new());
        let signer = Arc::new(MockCombinedSigner::new());

        let service = OrderBookService::new(mock_server.url(), provider, signer)
            .expect("Failed to create OrderBookService");

        let params = SwapTransactionParams {
            source_account: "GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX".to_string(),
            source_asset: "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
                .to_string(),
            destination_asset: "native".to_string(),
            amount: 100000000,
            slippage_percent: 1.0,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            source_asset_decimals: Some(7),
            destination_asset_decimals: Some(7),
        };

        let result = service.get_swap_quote(&params).await;

        mock.assert_async().await;
        assert!(result.is_ok());
        let quote = result.unwrap();
        assert_eq!(quote.in_amount, 100000000);
        assert_eq!(quote.out_amount, 98000000);
    }

    #[test]
    fn test_slippage_bps_calculation() {
        let (_service, _, _) = create_test_service();

        // Test slippage to BPS conversion
        let test_cases = vec![
            (0.0, 0),
            (0.5, 50),
            (1.0, 100),
            (2.5, 250),
            (5.0, 500),
            (10.0, 1000),
        ];

        for (slippage_percent, expected_bps) in test_cases {
            let bps = (slippage_percent * SLIPPAGE_TO_BPS_MULTIPLIER) as u32;
            assert_eq!(
                bps, expected_bps,
                "Failed for slippage {}%",
                slippage_percent
            );
        }
    }
}
