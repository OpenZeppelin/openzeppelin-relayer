//! Gas abstraction implementation for Stellar relayers.
//!
//! This module implements the `GasAbstractionTrait` for Stellar relayers, providing
//! gas abstraction functionality including fee estimation and transaction preparation.

use async_trait::async_trait;
use chrono::Utc;
use soroban_rs::xdr::{Limits, Operation, TransactionEnvelope, WriteXdr};
use tracing::debug;

use crate::constants::{
    get_default_fee_forwarder, get_stellar_sponsored_transaction_validity_duration,
    STELLAR_DEFAULT_TRANSACTION_FEE,
};

/// Default slippage tolerance for max_fee_amount in basis points (500 = 5%)
/// This allows fee fluctuation between quote and execution time
const DEFAULT_MAX_FEE_SLIPPAGE_BPS: u64 = 500;
use crate::domain::relayer::{
    stellar::xdr_utils::{extract_source_account, parse_transaction_xdr},
    GasAbstractionTrait, RelayerError, StellarRelayer,
};
use crate::domain::transaction::stellar::{
    utils::{
        add_operation_to_envelope, convert_xlm_fee_to_token, create_fee_payment_operation,
        estimate_fee, set_time_bounds, FeeQuote,
    },
    StellarTransactionValidator,
};
use crate::domain::xdr_needs_simulation;
use crate::jobs::JobProducerTrait;
use crate::models::{
    transaction::stellar::OperationSpec, SponsoredTransactionBuildRequest,
    SponsoredTransactionBuildResponse, SponsoredTransactionQuoteRequest,
    SponsoredTransactionQuoteResponse, StellarFeeEstimateResult, StellarPrepareTransactionResult,
    StellarTransactionData, TransactionInput,
};
use crate::models::{NetworkRepoModel, RelayerRepoModel, TransactionRepoModel};
use crate::repositories::{
    NetworkRepository, RelayerRepository, Repository, TransactionRepository,
};
use crate::services::provider::StellarProviderTrait;
use crate::services::signer::StellarSignTrait;
use crate::services::stellar_dex::StellarDexServiceTrait;
use crate::services::stellar_fee_forwarder::{
    FeeForwarderParams, FeeForwarderService, LEDGER_TIME_SECONDS,
};
use crate::services::TransactionCounterServiceTrait;
use soroban_rs::xdr::{HostFunction, OperationBody, ReadXdr, ScVal};

/// Information extracted from a Soroban InvokeHostFunction operation
#[derive(Debug, Clone)]
pub struct SorobanInvokeInfo {
    /// Target contract address (C... format)
    pub target_contract: String,
    /// Target function name
    pub target_fn: String,
    /// Target function arguments
    pub target_args: Vec<ScVal>,
}

/// Detect if a transaction XDR contains a Soroban InvokeHostFunction operation
/// and extract the contract call details.
///
/// Returns:
/// - `Ok(Some(info))` if XDR contains an InvokeHostFunction operation
/// - `Ok(None)` if XDR is a classic transaction (no InvokeHostFunction)
/// - `Err(...)` if XDR is invalid
fn detect_soroban_invoke_from_xdr(xdr: &str) -> Result<Option<SorobanInvokeInfo>, RelayerError> {
    use soroban_rs::xdr::TransactionEnvelope;

    let envelope = TransactionEnvelope::from_xdr_base64(xdr, Limits::none())
        .map_err(|e| RelayerError::ValidationError(format!("Invalid XDR: {e}")))?;

    // Extract operations from envelope
    let operations = match &envelope {
        TransactionEnvelope::TxV0(env) => env.tx.operations.to_vec(),
        TransactionEnvelope::Tx(env) => env.tx.operations.to_vec(),
        TransactionEnvelope::TxFeeBump(env) => match &env.tx.inner_tx {
            soroban_rs::xdr::FeeBumpTransactionInnerTx::Tx(inner) => inner.tx.operations.to_vec(),
        },
    };

    let mut invoke_index = None;
    let mut invoke_op = None;

    for (idx, op) in operations.iter().enumerate() {
        if let OperationBody::InvokeHostFunction(invoke) = &op.body {
            invoke_index = Some(idx);
            invoke_op = Some(invoke);
            break;
        }
    }

    if let Some(idx) = invoke_index {
        // Soroban transactions must contain exactly one operation
        if operations.len() != 1 {
            return Err(RelayerError::ValidationError(
                "Soroban transactions must contain exactly one operation".to_string(),
            ));
        }

        // Single-operation Soroban must be InvokeHostFunction
        let invoke_op = invoke_op.ok_or_else(|| {
            RelayerError::ValidationError("InvokeHostFunction operation missing".to_string())
        })?;

        if idx != 0 {
            return Err(RelayerError::ValidationError(
                "InvokeHostFunction must be the first operation".to_string(),
            ));
        }

        if let HostFunction::InvokeContract(invoke_args) = &invoke_op.host_function {
            // Extract contract address
            let target_contract = match &invoke_args.contract_address {
                soroban_rs::xdr::ScAddress::Contract(contract_id) => {
                    stellar_strkey::Contract(contract_id.0 .0).to_string()
                }
                _ => {
                    return Err(RelayerError::ValidationError(
                        "InvokeHostFunction must target a contract address".to_string(),
                    ));
                }
            };

            // Extract function name
            let target_fn = invoke_args.function_name.to_utf8_string_lossy();

            // Extract arguments
            let target_args: Vec<ScVal> = invoke_args.args.to_vec();

            return Ok(Some(SorobanInvokeInfo {
                target_contract,
                target_fn,
                target_args,
            }));
        }
    }

    // Not a Soroban InvokeHostFunction transaction
    Ok(None)
}

#[async_trait]
impl<P, RR, NR, TR, J, TCS, S, D> GasAbstractionTrait
    for StellarRelayer<P, RR, NR, TR, J, TCS, S, D>
where
    P: StellarProviderTrait + Send + Sync,
    D: StellarDexServiceTrait + Send + Sync + 'static,
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    TCS: TransactionCounterServiceTrait + Send + Sync + 'static,
    S: StellarSignTrait + Send + Sync + 'static,
{
    async fn quote_sponsored_transaction(
        &self,
        params: SponsoredTransactionQuoteRequest,
    ) -> Result<SponsoredTransactionQuoteResponse, RelayerError> {
        let params = match params {
            SponsoredTransactionQuoteRequest::Stellar(p) => p,
            _ => {
                return Err(RelayerError::ValidationError(
                    "Expected Stellar fee estimate request parameters".to_string(),
                ));
            }
        };

        // Check if this is a Soroban gas abstraction request by detecting InvokeHostFunction in XDR
        // Soroban mode is detected when transaction_xdr contains an InvokeHostFunction operation
        if let Some(xdr) = &params.transaction_xdr {
            if let Some(soroban_info) = detect_soroban_invoke_from_xdr(xdr)? {
                return self.quote_soroban_from_xdr(&params, &soroban_info).await;
            }
        }

        // Classic sponsored transaction flow
        self.quote_classic_sponsored(&params).await
    }

    async fn build_sponsored_transaction(
        &self,
        params: SponsoredTransactionBuildRequest,
    ) -> Result<SponsoredTransactionBuildResponse, RelayerError> {
        let params = match params {
            SponsoredTransactionBuildRequest::Stellar(p) => p,
            _ => {
                return Err(RelayerError::ValidationError(
                    "Expected Stellar prepare transaction request parameters".to_string(),
                ));
            }
        };

        let policy = self.relayer.policies.get_stellar_policy();

        // Validate allowed token
        StellarTransactionValidator::validate_allowed_token(&params.fee_token, &policy)
            .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Validate fee_payment_strategy is User
        if !policy.is_user_fee_payment() {
            return Err(RelayerError::ValidationError(
                "Gas abstraction requires fee_payment_strategy: User".to_string(),
            ));
        }

        // Check if this is a Soroban gas abstraction request by detecting InvokeHostFunction in XDR
        if let Some(xdr) = &params.transaction_xdr {
            if let Some(soroban_info) = detect_soroban_invoke_from_xdr(xdr)? {
                return self.build_soroban_sponsored(&params, &soroban_info).await;
            }
        }

        // Classic sponsored transaction flow
        self.build_classic_sponsored(&params).await
    }
}

// ============================================================================
// Classic Sponsored Transaction Handlers (Fee-bump Flow)
// ============================================================================

impl<P, RR, NR, TR, J, TCS, S, D> StellarRelayer<P, RR, NR, TR, J, TCS, S, D>
where
    P: StellarProviderTrait + Send + Sync,
    D: StellarDexServiceTrait + Send + Sync + 'static,
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    TCS: TransactionCounterServiceTrait + Send + Sync + 'static,
    S: StellarSignTrait + Send + Sync + 'static,
{
    /// Quote a classic sponsored transaction (fee-bump flow)
    ///
    /// Estimates the fee for a standard Stellar transaction where the relayer
    /// pays the network fee and user pays in a token.
    async fn quote_classic_sponsored(
        &self,
        params: &crate::models::StellarFeeEstimateRequestParams,
    ) -> Result<SponsoredTransactionQuoteResponse, RelayerError> {
        debug!(
            "Processing classic quote sponsored transaction for token: {}",
            params.fee_token
        );

        let policy = self.relayer.policies.get_stellar_policy();

        // Validate allowed token
        StellarTransactionValidator::validate_allowed_token(&params.fee_token, &policy)
            .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Validate that either transaction_xdr or operations is provided
        if params.transaction_xdr.is_none() && params.operations.is_none() {
            return Err(RelayerError::ValidationError(
                "Must provide either transaction_xdr or operations in the request".to_string(),
            ));
        }

        // Build envelope from XDR or operations
        let envelope = build_envelope_from_request(
            params.transaction_xdr.as_ref(),
            params.operations.as_ref(),
            params.source_account.as_ref(),
            &self.network.passphrase,
            &self.provider,
        )
        .await?;

        // Run comprehensive security validation
        StellarTransactionValidator::gasless_transaction_validation(
            &envelope,
            &self.relayer.address,
            &policy,
            &self.provider,
            None, // Duration validation not needed for quote
        )
        .await
        .map_err(|e| {
            RelayerError::ValidationError(format!("Failed to validate gasless transaction: {e}"))
        })?;

        // Estimate fee using estimate_fee utility which handles simulation if needed
        let inner_tx_fee = estimate_fee(&envelope, &self.provider, None)
            .await
            .map_err(crate::models::RelayerError::from)?;

        // Add fees for fee payment operation (100 stroops) and fee-bump transaction (100 stroops)
        let is_soroban = xdr_needs_simulation(&envelope).unwrap_or(false);
        let additional_fees = if is_soroban {
            0 // Soroban simulation already accounts for resource fees
        } else {
            2 * STELLAR_DEFAULT_TRANSACTION_FEE as u64 // 200 stroops total
        };
        let xlm_fee = inner_tx_fee + additional_fees;

        // Convert to token amount via DEX service
        let fee_quote = convert_xlm_fee_to_token(
            self.dex_service.as_ref(),
            &policy,
            xlm_fee,
            &params.fee_token,
        )
        .await
        .map_err(crate::models::RelayerError::from)?;

        // Validate max fee
        StellarTransactionValidator::validate_max_fee(fee_quote.fee_in_stroops, &policy)
            .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Validate token-specific max fee
        StellarTransactionValidator::validate_token_max_fee(
            &params.fee_token,
            fee_quote.fee_in_token,
            &policy,
        )
        .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Check user token balance to ensure they have enough to pay the fee
        StellarTransactionValidator::validate_user_token_balance(
            &envelope,
            &params.fee_token,
            fee_quote.fee_in_token,
            &self.provider,
        )
        .await
        .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        debug!("Classic fee estimate result: {:?}", fee_quote);

        Ok(SponsoredTransactionQuoteResponse::Stellar(
            StellarFeeEstimateResult {
                fee_in_token_ui: fee_quote.fee_in_token_ui,
                fee_in_token: fee_quote.fee_in_token.to_string(),
                conversion_rate: fee_quote.conversion_rate.to_string(),
            },
        ))
    }

    /// Build a classic sponsored transaction (fee-bump flow)
    ///
    /// Builds a complete transaction envelope with fee payment operation,
    /// ready for user signature. The relayer will later wrap this in a fee-bump.
    async fn build_classic_sponsored(
        &self,
        params: &crate::models::StellarPrepareTransactionRequestParams,
    ) -> Result<SponsoredTransactionBuildResponse, RelayerError> {
        debug!(
            "Processing classic build sponsored transaction for token: {}",
            params.fee_token
        );

        let policy = self.relayer.policies.get_stellar_policy();

        // Validate allowed token
        StellarTransactionValidator::validate_allowed_token(&params.fee_token, &policy)
            .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Validate that either transaction_xdr or operations is provided
        if params.transaction_xdr.is_none() && params.operations.is_none() {
            return Err(RelayerError::ValidationError(
                "Must provide either transaction_xdr or operations in the request".to_string(),
            ));
        }

        // Build envelope from XDR or operations
        let envelope = build_envelope_from_request(
            params.transaction_xdr.as_ref(),
            params.operations.as_ref(),
            params.source_account.as_ref(),
            &self.network.passphrase,
            &self.provider,
        )
        .await?;

        // Run comprehensive security validation
        StellarTransactionValidator::gasless_transaction_validation(
            &envelope,
            &self.relayer.address,
            &policy,
            &self.provider,
            None, // Duration validation not needed here as time bounds are set during build
        )
        .await
        .map_err(|e| {
            RelayerError::ValidationError(format!("Failed to validate gasless transaction: {e}"))
        })?;

        // Estimate fee using estimate_fee utility which handles simulation if needed
        let inner_tx_fee = estimate_fee(&envelope, &self.provider, None)
            .await
            .map_err(crate::models::RelayerError::from)?;

        // Add fees for fee payment operation and fee-bump transaction
        let is_soroban = xdr_needs_simulation(&envelope).unwrap_or(false);
        let additional_fees = if is_soroban {
            0
        } else {
            2 * STELLAR_DEFAULT_TRANSACTION_FEE as u64 // 200 stroops total
        };
        let xlm_fee = inner_tx_fee + additional_fees;

        debug!(
            inner_tx_fee = inner_tx_fee,
            additional_fees = additional_fees,
            total_fee = xlm_fee,
            "Fee estimated: inner transaction + fee payment op + fee-bump"
        );

        // Calculate fee quote to check user balance before modifying envelope
        let fee_quote = convert_xlm_fee_to_token(
            self.dex_service.as_ref(),
            &policy,
            xlm_fee,
            &params.fee_token,
        )
        .await
        .map_err(crate::models::RelayerError::from)?;

        // Validate max fee
        StellarTransactionValidator::validate_max_fee(fee_quote.fee_in_stroops, &policy)
            .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Validate token-specific max fee
        StellarTransactionValidator::validate_token_max_fee(
            &params.fee_token,
            fee_quote.fee_in_token,
            &policy,
        )
        .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Check user token balance to ensure they have enough to pay the fee
        StellarTransactionValidator::validate_user_token_balance(
            &envelope,
            &params.fee_token,
            fee_quote.fee_in_token,
            &self.provider,
        )
        .await
        .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Add payment operation using the validated fee quote
        let mut final_envelope = add_payment_operation_to_envelope(
            envelope,
            &fee_quote,
            &params.fee_token,
            &self.relayer.address,
        )?;

        debug!(
            estimated_fee = xlm_fee,
            final_fee_in_token = fee_quote.fee_in_token_ui,
            "Classic transaction prepared successfully"
        );

        // Set final time bounds just before returning to give user maximum time to sign
        let valid_until = Utc::now() + get_stellar_sponsored_transaction_validity_duration();
        set_time_bounds(&mut final_envelope, valid_until)
            .map_err(crate::models::RelayerError::from)?;

        // Serialize final transaction
        let extended_xdr = final_envelope
            .to_xdr_base64(Limits::none())
            .map_err(|e| RelayerError::Internal(format!("Failed to serialize XDR: {e}")))?;

        Ok(SponsoredTransactionBuildResponse::Stellar(
            StellarPrepareTransactionResult {
                transaction: extended_xdr,
                fee_in_token: fee_quote.fee_in_token.to_string(),
                fee_in_token_ui: fee_quote.fee_in_token_ui,
                fee_in_stroops: fee_quote.fee_in_stroops.to_string(),
                fee_token: params.fee_token.clone(),
                valid_until: valid_until.to_rfc3339(),
                // Classic mode: no Soroban-specific fields
                user_auth_entry: None,
            },
        ))
    }
}

// ============================================================================
// Soroban Gas Abstraction Handlers (FeeForwarder Flow with XDR-based detection)
// ============================================================================

impl<P, RR, NR, TR, J, TCS, S, D> StellarRelayer<P, RR, NR, TR, J, TCS, S, D>
where
    P: StellarProviderTrait + Send + Sync,
    D: StellarDexServiceTrait + Send + Sync + 'static,
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    TCS: TransactionCounterServiceTrait + Send + Sync + 'static,
    S: StellarSignTrait + Send + Sync + 'static,
{
    /// Quote a Soroban sponsored transaction using FeeForwarder (XDR-based detection)
    ///
    /// Called when transaction_xdr contains an InvokeHostFunction operation.
    /// Extracts contract call details from the XDR and estimates fee.
    async fn quote_soroban_from_xdr(
        &self,
        params: &crate::models::StellarFeeEstimateRequestParams,
        soroban_info: &SorobanInvokeInfo,
    ) -> Result<SponsoredTransactionQuoteResponse, RelayerError> {
        debug!(
            "Processing Soroban quote request for token: {}, target: {}::{}",
            params.fee_token, soroban_info.target_contract, soroban_info.target_fn
        );

        let policy = self.relayer.policies.get_stellar_policy();

        // Validate fee_payment_strategy is User
        if !policy.is_user_fee_payment() {
            return Err(RelayerError::ValidationError(
                "Gas abstraction requires fee_payment_strategy: User".to_string(),
            ));
        }

        // Validate allowed token (same as classic flow)
        StellarTransactionValidator::validate_allowed_token(&params.fee_token, &policy)
            .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Get fee_forwarder address: env var override takes precedence, otherwise use network default
        let fee_forwarder = crate::config::ServerConfig::get_stellar_fee_forwarder_address()
            .or_else(|| {
                let default = get_default_fee_forwarder(self.network.is_testnet());
                if default.is_empty() {
                    None
                } else {
                    Some(default.to_string())
                }
            })
            .ok_or_else(|| {
                RelayerError::ValidationError(
                    "FeeForwarder address not configured. Set STELLAR_FEE_FORWARDER_ADDRESS env var or wait for default deployment.".to_string(),
                )
            })?;

        // Validate fee_token is a valid Soroban contract address (C...)
        if stellar_strkey::Contract::from_string(&params.fee_token).is_err() {
            return Err(RelayerError::ValidationError(format!(
                "fee_token must be a valid Soroban contract address (C...), got '{}'",
                params.fee_token
            )));
        }

        // Extract user_address from transaction_xdr source account (or use source_account if provided)
        // For quote, we don't need the actual user_address, just validation that XDR is valid
        // The user_address will be extracted in build phase when we have the XDR

        let xdr = params.transaction_xdr.as_ref().ok_or_else(|| {
            RelayerError::ValidationError(
                "Soroban gas abstraction requires transaction_xdr".to_string(),
            )
        })?;

        let source_envelope = TransactionEnvelope::from_xdr_base64(xdr, Limits::none())
            .map_err(|e| RelayerError::ValidationError(format!("Invalid XDR: {e}")))?;
        let user_address = extract_source_account(&source_envelope).map_err(|e| {
            RelayerError::ValidationError(format!("Failed to extract source account: {e}"))
        })?;

        // Build FeeForwarder params with a placeholder fee (will simulate to get accurate fee)
        let base_fee_stroops: u64 = STELLAR_DEFAULT_TRANSACTION_FEE as u64;
        let base_fee_quote = convert_xlm_fee_to_token(
            self.dex_service.as_ref(),
            &policy,
            base_fee_stroops,
            &params.fee_token,
        )
        .await
        .map_err(crate::models::RelayerError::from)?;

        let validity_duration = get_stellar_sponsored_transaction_validity_duration();
        let validity_seconds = validity_duration.num_seconds() as u64;
        let expiration_ledger = get_expiration_ledger(&self.provider, validity_seconds)
            .await
            .map_err(|e| RelayerError::Internal(format!("Failed to get expiration ledger: {e}")))?;

        let fee_params = FeeForwarderParams {
            fee_token: params.fee_token.clone(),
            fee_amount: base_fee_quote.fee_in_token as i128,
            max_fee_amount: apply_max_fee_slippage(base_fee_quote.fee_in_token),
            expiration_ledger,
            target_contract: soroban_info.target_contract.clone(),
            target_fn: soroban_info.target_fn.clone(),
            target_args: soroban_info.target_args.clone(),
            user: user_address,
            relayer: self.relayer.address.clone(),
        };

        // For quote/simulation, we don't include auth entries because the FeeForwarder
        // contract has custom auth verification that fails on empty signatures.
        // Unlike standard Soroban "recording mode", this contract explicitly checks
        // for valid signatures and returns Error when none are found.
        // The build flow will include proper auth entries for accurate resource estimation.
        let invoke_op = FeeForwarderService::<P>::build_invoke_operation_standalone(
            &fee_forwarder,
            &fee_params,
            vec![],
        )
        .map_err(|e| RelayerError::Internal(format!("Failed to build invoke operation: {e}")))?;

        let envelope = build_soroban_transaction_envelope(
            &self.relayer.address,
            invoke_op,
            base_fee_stroops as u32,
        )?;

        let sim_response = self
            .provider
            .simulate_transaction_envelope(&envelope)
            .await
            .map_err(|e| RelayerError::Internal(format!("Failed to simulate transaction: {e}")))?;

        let total_fee = calculate_total_soroban_fee(&sim_response, 1)?;

        let fee_quote = convert_xlm_fee_to_token(
            self.dex_service.as_ref(),
            &policy,
            total_fee as u64,
            &params.fee_token,
        )
        .await
        .map_err(crate::models::RelayerError::from)?;

        debug!(
            "Soroban fee estimate: {} stroops, {} token",
            fee_quote.fee_in_stroops, fee_quote.fee_in_token
        );

        // Validate max fee
        StellarTransactionValidator::validate_max_fee(fee_quote.fee_in_stroops, &policy)
            .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Validate token-specific max fee
        StellarTransactionValidator::validate_token_max_fee(
            &params.fee_token,
            fee_quote.fee_in_token,
            &policy,
        )
        .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Check user token balance using the original source envelope (user as source)
        StellarTransactionValidator::validate_user_token_balance(
            &source_envelope,
            &params.fee_token,
            fee_quote.fee_in_token,
            &self.provider,
        )
        .await
        .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Return using consolidated result struct
        let result = StellarFeeEstimateResult {
            fee_in_token_ui: fee_quote.fee_in_token_ui,
            fee_in_token: fee_quote.fee_in_token.to_string(),
            conversion_rate: fee_quote.conversion_rate.to_string(),
        };

        Ok(SponsoredTransactionQuoteResponse::Stellar(result))
    }

    /// Build a Soroban sponsored transaction using FeeForwarder (XDR-based detection)
    ///
    /// Called when transaction_xdr contains an InvokeHostFunction operation.
    /// Builds the FeeForwarder transaction wrapping the original contract call.
    async fn build_soroban_sponsored(
        &self,
        params: &crate::models::StellarPrepareTransactionRequestParams,
        soroban_info: &SorobanInvokeInfo,
    ) -> Result<SponsoredTransactionBuildResponse, RelayerError> {
        debug!(
            "Processing Soroban build request for token: {}, target: {}::{}",
            params.fee_token, soroban_info.target_contract, soroban_info.target_fn
        );

        let policy = self.relayer.policies.get_stellar_policy();

        // Note: validate_allowed_token is already called in build_sponsored_transaction

        // Get fee_forwarder address: env var override takes precedence, otherwise use network default
        let fee_forwarder = crate::config::ServerConfig::get_stellar_fee_forwarder_address()
            .or_else(|| {
                let default = get_default_fee_forwarder(self.network.is_testnet());
                if default.is_empty() {
                    None
                } else {
                    Some(default.to_string())
                }
            })
            .ok_or_else(|| {
                RelayerError::ValidationError(
                    "FeeForwarder address not configured. Set STELLAR_FEE_FORWARDER_ADDRESS env var or wait for default deployment.".to_string(),
                )
            })?;

        // Validate fee_token is a valid Soroban contract address (C...)
        if stellar_strkey::Contract::from_string(&params.fee_token).is_err() {
            return Err(RelayerError::ValidationError(format!(
                "fee_token must be a valid Soroban contract address (C...), got '{}'",
                params.fee_token
            )));
        }

        // Extract user_address from transaction_xdr source account
        // Soroban gas abstraction requires transaction_xdr, so we can unwrap here
        let xdr = params.transaction_xdr.as_ref().ok_or_else(|| {
            RelayerError::ValidationError(
                "Soroban gas abstraction requires transaction_xdr".to_string(),
            )
        })?;

        let source_envelope = TransactionEnvelope::from_xdr_base64(xdr, Limits::none())
            .map_err(|e| RelayerError::ValidationError(format!("Invalid XDR: {e}")))?;
        let user_address = extract_source_account(&source_envelope).map_err(|e| {
            RelayerError::ValidationError(format!("Failed to extract source account: {e}"))
        })?;

        // Use default validity duration (same as classic sponsored transactions)
        let validity_duration = get_stellar_sponsored_transaction_validity_duration();
        let validity_seconds = validity_duration.num_seconds() as u64;
        let expiration_ledger = get_expiration_ledger(&self.provider, validity_seconds)
            .await
            .map_err(|e| RelayerError::Internal(format!("Failed to get expiration ledger: {e}")))?;

        // Build initial fee quote based on base fee, then simulate to get accurate Soroban fee
        let base_fee_stroops: u64 = STELLAR_DEFAULT_TRANSACTION_FEE as u64;
        let base_fee_quote = convert_xlm_fee_to_token(
            self.dex_service.as_ref(),
            &policy,
            base_fee_stroops,
            &params.fee_token,
        )
        .await
        .map_err(crate::models::RelayerError::from)?;

        // Build the FeeForwarder parameters using extracted Soroban info
        let mut fee_params = FeeForwarderParams {
            fee_token: params.fee_token.clone(),
            fee_amount: base_fee_quote.fee_in_token as i128,
            max_fee_amount: apply_max_fee_slippage(base_fee_quote.fee_in_token),
            expiration_ledger,
            target_contract: soroban_info.target_contract.clone(),
            target_fn: soroban_info.target_fn.clone(),
            target_args: soroban_info.target_args.clone(),
            user: user_address.clone(),
            relayer: self.relayer.address.clone(),
        };

        // For simulation, we don't include auth entries because the FeeForwarder
        // contract has custom auth verification that fails on empty signatures.
        // Unlike standard Soroban "recording mode", this contract explicitly checks
        // for valid signatures and returns Error when none are found.
        let invoke_op = FeeForwarderService::<P>::build_invoke_operation_standalone(
            &fee_forwarder,
            &fee_params,
            vec![], // Empty auth entries for simulation
        )
        .map_err(|e| RelayerError::Internal(format!("Failed to build invoke operation: {e}")))?;

        let envelope = build_soroban_transaction_envelope(
            &self.relayer.address,
            invoke_op,
            base_fee_stroops as u32,
        )?;

        let sim_response = self
            .provider
            .simulate_transaction_envelope(&envelope)
            .await
            .map_err(|e| RelayerError::Internal(format!("Failed to simulate transaction: {e}")))?;

        let total_fee = calculate_total_soroban_fee(&sim_response, 1)?;

        let fee_quote = convert_xlm_fee_to_token(
            self.dex_service.as_ref(),
            &policy,
            total_fee as u64,
            &params.fee_token,
        )
        .await
        .map_err(crate::models::RelayerError::from)?;

        // Validate max fee
        StellarTransactionValidator::validate_max_fee(fee_quote.fee_in_stroops, &policy)
            .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Validate token-specific max fee
        StellarTransactionValidator::validate_token_max_fee(
            &params.fee_token,
            fee_quote.fee_in_token,
            &policy,
        )
        .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Check user token balance using the original source envelope (user as source)
        StellarTransactionValidator::validate_user_token_balance(
            &source_envelope,
            &params.fee_token,
            fee_quote.fee_in_token,
            &self.provider,
        )
        .await
        .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Rebuild params with the final fee amounts
        // Apply slippage buffer to max_fee_amount to allow for fee fluctuation
        fee_params.fee_amount = fee_quote.fee_in_token as i128;
        fee_params.max_fee_amount = apply_max_fee_slippage(fee_quote.fee_in_token);

        // Build the user authorization entry for the user to sign
        let user_auth_entry = FeeForwarderService::<P>::build_user_auth_entry_standalone(
            &fee_forwarder,
            &fee_params,
            true,
        )
        .map_err(|e| RelayerError::Internal(format!("Failed to build user auth entry: {e}")))?;

        let user_auth_xdr = FeeForwarderService::<P>::serialize_auth_entry(&user_auth_entry)
            .map_err(|e| RelayerError::Internal(format!("Failed to serialize auth entry: {e}")))?;

        // Build relayer auth entry - required by FeeForwarder contract
        let relayer_auth_entry = FeeForwarderService::<P>::build_relayer_auth_entry_standalone(
            &fee_forwarder,
            &fee_params,
        )
        .map_err(|e| RelayerError::Internal(format!("Failed to build relayer auth entry: {e}")))?;

        // Build the final invoke operation WITH auth entries
        // Note: We don't simulate again because the contract's custom auth verification
        // would fail on empty signatures. We use the simulation data from the first
        // simulation (without auth entries) to set the fee and resources.
        let invoke_op = FeeForwarderService::<P>::build_invoke_operation_standalone(
            &fee_forwarder,
            &fee_params,
            vec![user_auth_entry, relayer_auth_entry],
        )
        .map_err(|e| RelayerError::Internal(format!("Failed to build invoke operation: {e}")))?;

        let mut envelope = build_soroban_transaction_envelope(
            &self.relayer.address,
            invoke_op,
            base_fee_stroops as u32,
        )?;

        // Apply simulation data from the first simulation (without auth entries)
        // This sets the fee and Soroban resource data on the final envelope
        // Also extends the footprint to include the relayer's account for require_auth
        apply_simulation_to_soroban_envelope(&mut envelope, &sim_response, 1)?;

        let transaction_xdr = envelope
            .to_xdr_base64(Limits::none())
            .map_err(|e| RelayerError::Internal(format!("Failed to serialize transaction: {e}")))?;

        // Derive valid_until from expiration_ledger to ensure consistency
        // Get current ledger to calculate time until expiration
        let current_ledger =
            self.provider.get_latest_ledger().await.map_err(|e| {
                RelayerError::Internal(format!("Failed to get current ledger: {e}"))
            })?;
        let ledgers_until_expiration = expiration_ledger.saturating_sub(current_ledger.sequence);
        let seconds_until_expiration = ledgers_until_expiration as u64 * LEDGER_TIME_SECONDS;
        let valid_until = Utc::now() + chrono::Duration::seconds(seconds_until_expiration as i64);

        debug!(
            "Soroban build complete: transaction_xdr length={}, auth_xdr length={}, expiration_ledger={}, valid_until={}",
            transaction_xdr.len(),
            user_auth_xdr.len(),
            expiration_ledger,
            valid_until.to_rfc3339()
        );

        // Return using consolidated result struct with Soroban-specific fields populated
        let result = StellarPrepareTransactionResult {
            transaction: transaction_xdr,
            fee_in_token: fee_quote.fee_in_token.to_string(),
            fee_in_token_ui: fee_quote.fee_in_token_ui,
            fee_in_stroops: fee_quote.fee_in_stroops.to_string(),
            fee_token: params.fee_token.clone(),
            valid_until: valid_until.to_rfc3339(),
            // Soroban-specific fields
            user_auth_entry: Some(user_auth_xdr),
        };

        Ok(SponsoredTransactionBuildResponse::Stellar(result))
    }
}

/// Build a Soroban transaction envelope with the given operation
fn build_soroban_transaction_envelope(
    source_address: &str,
    operation: Operation,
    fee: u32,
) -> Result<TransactionEnvelope, RelayerError> {
    use soroban_rs::xdr::{
        Memo, MuxedAccount, Preconditions, SequenceNumber, Transaction, TransactionExt,
        TransactionV1Envelope, Uint256, VecM,
    };

    // Parse source address
    let pk = stellar_strkey::ed25519::PublicKey::from_string(source_address)
        .map_err(|e| RelayerError::ValidationError(format!("Invalid source address: {e}")))?;
    let source = MuxedAccount::Ed25519(Uint256(pk.0));

    // Build transaction with placeholder sequence (0) - will be updated at submit time
    let tx = Transaction {
        source_account: source,
        fee,
        seq_num: SequenceNumber(0),
        cond: Preconditions::None,
        memo: Memo::None,
        operations: vec![operation].try_into().map_err(|_| {
            RelayerError::Internal("Failed to create operations vector".to_string())
        })?,
        ext: TransactionExt::V0,
    };

    Ok(TransactionEnvelope::Tx(TransactionV1Envelope {
        tx,
        signatures: VecM::default(),
    }))
}

/// Calculate total fee for a Soroban transaction from simulation response.
fn calculate_total_soroban_fee(
    sim_response: &soroban_rs::stellar_rpc_client::SimulateTransactionResponse,
    operations_count: u64,
) -> Result<u32, RelayerError> {
    if let Some(err) = sim_response.error.clone() {
        return Err(RelayerError::ValidationError(format!(
            "Simulation failed: {err}"
        )));
    }

    let inclusion_fee = operations_count * STELLAR_DEFAULT_TRANSACTION_FEE as u64;
    let resource_fee = sim_response.min_resource_fee;
    let total_fee = inclusion_fee + resource_fee;
    let total_fee_u32 = u32::try_from(total_fee)
        .map_err(|_| RelayerError::Internal("Soroban fee exceeds u32::MAX".to_string()))?;

    Ok(total_fee_u32.max(STELLAR_DEFAULT_TRANSACTION_FEE))
}

/// Apply Soroban simulation data to a transaction envelope (fee + extension data).
fn apply_simulation_to_soroban_envelope(
    envelope: &mut TransactionEnvelope,
    sim_response: &soroban_rs::stellar_rpc_client::SimulateTransactionResponse,
    operations_count: u64,
) -> Result<(), RelayerError> {
    use soroban_rs::xdr::SorobanTransactionData;

    let total_fee = calculate_total_soroban_fee(sim_response, operations_count)?;

    let tx_data = SorobanTransactionData::from_xdr_base64(
        sim_response.transaction_data.as_str(),
        Limits::none(),
    )
    .map_err(|e| RelayerError::Internal(format!("Invalid transaction_data XDR: {e}")))?;

    match envelope {
        TransactionEnvelope::Tx(ref mut env) => {
            env.tx.fee = total_fee;
            env.tx.ext = soroban_rs::xdr::TransactionExt::V1(tx_data);
        }
        TransactionEnvelope::TxV0(_) | TransactionEnvelope::TxFeeBump(_) => {
            return Err(RelayerError::Internal(
                "Soroban transaction must be a V1 envelope".to_string(),
            ));
        }
    }

    Ok(())
}

/// Apply slippage tolerance to max_fee_amount for FeeForwarder
///
/// The FeeForwarder contract has separate `fee_amount` (what relayer charges at execution)
/// and `max_fee_amount` (user's authorized ceiling). Setting them equal means no room for
/// fee fluctuation between quote and execution. This function applies a slippage buffer
/// to allow for price movement.
///
/// # Arguments
/// * `fee_in_token` - The calculated fee amount in token units
///
/// # Returns
/// The max_fee_amount with slippage buffer applied as i128
fn apply_max_fee_slippage(fee_in_token: u64) -> i128 {
    // Apply slippage: max_fee = fee * (10000 + slippage_bps) / 10000
    let fee_with_slippage =
        (fee_in_token as u128) * (10000 + DEFAULT_MAX_FEE_SLIPPAGE_BPS as u128) / 10000;
    fee_with_slippage as i128
}

/// Calculate the expiration ledger for authorization
///
/// Uses the provider to get the current ledger sequence and adds the
/// specified validity duration (in seconds) converted to ledger count.
async fn get_expiration_ledger<P>(provider: &P, validity_seconds: u64) -> Result<u32, RelayerError>
where
    P: StellarProviderTrait + Send + Sync,
{
    let current_ledger = provider
        .get_latest_ledger()
        .await
        .map_err(|e| RelayerError::Internal(format!("Failed to get latest ledger: {e}")))?;

    let mut ledgers_to_add = validity_seconds.div_ceil(LEDGER_TIME_SECONDS);
    if ledgers_to_add == 0 {
        ledgers_to_add = 1;
    }
    Ok(current_ledger
        .sequence
        .saturating_add(ledgers_to_add as u32))
}

/// Add payment operation to envelope using a pre-computed fee quote
///
/// This function adds a fee payment operation to the transaction envelope using
/// a pre-computed FeeQuote. This avoids duplicate DEX calls and ensures the
/// validated fee quote matches the fee amount in the payment operation.
///
/// Note: Time bounds should be set separately just before returning the transaction
/// to give the user maximum time to review and submit.
///
/// # Arguments
/// * `envelope` - The transaction envelope to add the payment operation to
/// * `fee_quote` - Pre-computed fee quote containing the token amount to charge
/// * `fee_token` - Asset identifier for the fee token
/// * `relayer_address` - Address of the relayer receiving the fee payment
///
/// # Returns
/// The updated envelope with the payment operation added (if not Soroban)
fn add_payment_operation_to_envelope(
    mut envelope: TransactionEnvelope,
    fee_quote: &FeeQuote,
    fee_token: &str,
    relayer_address: &str,
) -> Result<TransactionEnvelope, RelayerError> {
    // Convert fee amount to i64 for payment operation
    let fee_amount = i64::try_from(fee_quote.fee_in_token).map_err(|_| {
        RelayerError::Internal(
            "Fee amount too large for payment operation (exceeds i64::MAX)".to_string(),
        )
    })?;

    let is_soroban = xdr_needs_simulation(&envelope).unwrap_or(false);
    // For Soroban we don't add the fee payment operation because of Soroban limitation to allow just single operation in the transaction
    if !is_soroban {
        // Add fee payment operation to envelope
        add_fee_payment_operation(&mut envelope, fee_token, fee_amount, relayer_address)?;
    }

    Ok(envelope)
}

/// Build a transaction envelope from either XDR or operations
///
/// This helper function is used by both quote and build methods to construct
/// a transaction envelope from either a pre-built XDR transaction or from
/// operations with a source account.
///
/// When building from operations, this function fetches the user's current
/// sequence number from the network to ensure the transaction can be properly
/// signed and submitted by the user.
async fn build_envelope_from_request<P>(
    transaction_xdr: Option<&String>,
    operations: Option<&Vec<OperationSpec>>,
    source_account: Option<&String>,
    network_passphrase: &str,
    provider: &P,
) -> Result<TransactionEnvelope, RelayerError>
where
    P: StellarProviderTrait + Send + Sync,
{
    if let Some(xdr) = transaction_xdr {
        parse_transaction_xdr(xdr, false)
            .map_err(|e| RelayerError::Internal(format!("Failed to parse XDR: {e}")))
    } else if let Some(ops) = operations {
        // Build envelope from operations
        let source_account = source_account.ok_or_else(|| {
            RelayerError::ValidationError(
                "source_account is required when providing operations".to_string(),
            )
        })?;

        // Create StellarTransactionData from operations
        // Fetch the user's current sequence number from the network
        // This is required because the user will sign the transaction with their account
        let account_entry = provider.get_account(source_account).await.map_err(|e| {
            RelayerError::Internal(format!(
                "Failed to fetch account sequence number for {source_account}: {e}",
            ))
        })?;

        // Use the next sequence number (current + 1)
        let next_sequence = account_entry.seq_num.0 + 1;

        let stellar_data = StellarTransactionData {
            source_account: source_account.clone(),
            fee: None,
            sequence_number: Some(next_sequence as i64),
            memo: None,
            valid_until: None,
            network_passphrase: network_passphrase.to_string(),
            signatures: vec![],
            hash: None,
            simulation_transaction_data: None,
            transaction_input: TransactionInput::Operations(ops.clone()),
            signed_envelope_xdr: None,
            transaction_result_xdr: None,
        };

        // Build unsigned envelope from operations
        stellar_data.build_unsigned_envelope().map_err(|e| {
            RelayerError::Internal(format!("Failed to build envelope from operations: {e}"))
        })
    } else {
        Err(RelayerError::ValidationError(
            "Must provide either transaction_xdr or operations in the request".to_string(),
        ))
    }
}

/// Add a fee payment operation to the transaction envelope
fn add_fee_payment_operation(
    envelope: &mut TransactionEnvelope,
    fee_token: &str,
    fee_amount: i64,
    relayer_address: &str,
) -> Result<(), RelayerError> {
    let payment_op_spec = create_fee_payment_operation(relayer_address, fee_token, fee_amount)
        .map_err(crate::models::RelayerError::from)?;

    // Convert OperationSpec to XDR Operation
    let payment_op = Operation::try_from(payment_op_spec)
        .map_err(|e| RelayerError::Internal(format!("Failed to convert payment operation: {e}")))?;

    // Add payment operation to transaction
    add_operation_to_envelope(envelope, payment_op).map_err(crate::models::RelayerError::from)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::transaction::stellar::utils::parse_account_id;
    use crate::services::stellar_dex::AssetType;
    use crate::{
        config::{NetworkConfigCommon, StellarNetworkConfig},
        jobs::MockJobProducerTrait,
        models::{
            transaction::stellar::OperationSpec, AssetSpec, NetworkConfigData, NetworkRepoModel,
            NetworkType, RelayerNetworkPolicy, RelayerRepoModel, RelayerStellarPolicy, RpcConfig,
            SponsoredTransactionBuildRequest, SponsoredTransactionQuoteRequest,
        },
        repositories::{
            InMemoryNetworkRepository, MockRelayerRepository, MockTransactionRepository,
        },
        services::{
            provider::MockStellarProviderTrait, signer::MockStellarSignTrait,
            stellar_dex::MockStellarDexServiceTrait, MockTransactionCounterServiceTrait,
        },
    };
    use mockall::predicate::*;
    use serial_test::serial;
    use soroban_rs::stellar_rpc_client::GetLedgerEntriesResponse;
    use soroban_rs::stellar_rpc_client::LedgerEntryResult;
    use soroban_rs::xdr::{
        AccountEntry, AccountEntryExt, AccountId, AlphaNum4, AssetCode4, LedgerEntry,
        LedgerEntryData, LedgerEntryExt, LedgerKey, Limits, MuxedAccount, Operation, OperationBody,
        PaymentOp, Preconditions, PublicKey, SequenceNumber, String32, Thresholds, Transaction,
        TransactionEnvelope, TransactionExt, TransactionV1Envelope, TrustLineEntry,
        TrustLineEntryExt, Uint256, VecM, WriteXdr,
    };
    use std::future::ready;
    use std::sync::Arc;
    use stellar_strkey::ed25519::PublicKey as Ed25519PublicKey;

    const TEST_PK: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
    const TEST_NETWORK_PASSPHRASE: &str = "Test SDF Network ; September 2015";
    const USDC_ASSET: &str = "USDC:GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN";

    /// Helper function to create a test transaction XDR
    fn create_test_transaction_xdr() -> String {
        // Use a different account than TEST_PK (relayer address) to avoid validation error
        let source_pk = Ed25519PublicKey::from_string(
            "GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2",
        )
        .unwrap();
        let dest_pk = Ed25519PublicKey::from_string(
            "GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ",
        )
        .unwrap();

        let payment_op = PaymentOp {
            destination: MuxedAccount::Ed25519(Uint256(dest_pk.0)),
            asset: soroban_rs::xdr::Asset::Native,
            amount: 1000000,
        };

        let operation = Operation {
            source_account: None,
            body: OperationBody::Payment(payment_op),
        };

        let operations: VecM<Operation, 100> = vec![operation].try_into().unwrap();

        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
            fee: 100,
            seq_num: SequenceNumber(2), // Must be > account sequence (1)
            cond: Preconditions::None,
            memo: soroban_rs::xdr::Memo::None,
            operations,
            ext: TransactionExt::V0,
        };

        let envelope = TransactionV1Envelope {
            tx,
            signatures: vec![].try_into().unwrap(),
        };

        let tx_envelope = TransactionEnvelope::Tx(envelope);
        tx_envelope.to_xdr_base64(Limits::none()).unwrap()
    }

    /// Helper function to create a test relayer with user fee payment strategy
    fn create_test_relayer_with_user_fee_strategy() -> RelayerRepoModel {
        let mut policy = RelayerStellarPolicy::default();
        policy.fee_payment_strategy = Some(crate::models::StellarFeePaymentStrategy::User);
        policy.allowed_tokens = Some(vec![crate::models::StellarAllowedTokensPolicy {
            asset: USDC_ASSET.to_string(),
            metadata: None,
            max_allowed_fee: None,
            swap_config: None,
        }]);

        RelayerRepoModel {
            id: "test-relayer-id".to_string(),
            name: "Test Relayer".to_string(),
            network: "testnet".to_string(),
            paused: false,
            network_type: NetworkType::Stellar,
            signer_id: "signer-id".to_string(),
            policies: RelayerNetworkPolicy::Stellar(policy),
            address: TEST_PK.to_string(),
            notification_id: Some("notification-id".to_string()),
            system_disabled: false,
            custom_rpc_urls: None,
            ..Default::default()
        }
    }

    /// Helper function to create a mock DEX service
    fn create_mock_dex_service() -> Arc<MockStellarDexServiceTrait> {
        let mut mock_dex = MockStellarDexServiceTrait::new();
        mock_dex
            .expect_supported_asset_types()
            .returning(|| std::collections::HashSet::from([AssetType::Native, AssetType::Classic]));
        Arc::new(mock_dex)
    }

    /// Helper function to create a test network
    fn create_test_network() -> NetworkRepoModel {
        NetworkRepoModel {
            id: "stellar:testnet".to_string(),
            name: "testnet".to_string(),
            network_type: NetworkType::Stellar,
            config: NetworkConfigData::Stellar(StellarNetworkConfig {
                common: NetworkConfigCommon {
                    network: "testnet".to_string(),
                    from: None,
                    rpc_urls: Some(vec![RpcConfig::new(
                        "https://horizon-testnet.stellar.org".to_string(),
                    )]),
                    explorer_urls: None,
                    average_blocktime_ms: Some(5000),
                    is_testnet: Some(true),
                    tags: None,
                },
                passphrase: Some(TEST_NETWORK_PASSPHRASE.to_string()),
                horizon_url: Some("https://horizon-testnet.stellar.org".to_string()),
            }),
        }
    }

    /// Helper function to create a Stellar relayer instance for testing
    async fn create_test_relayer_instance(
        relayer_model: RelayerRepoModel,
        provider: MockStellarProviderTrait,
        dex_service: Arc<MockStellarDexServiceTrait>,
    ) -> crate::domain::relayer::stellar::StellarRelayer<
        MockStellarProviderTrait,
        MockRelayerRepository,
        InMemoryNetworkRepository,
        MockTransactionRepository,
        MockJobProducerTrait,
        MockTransactionCounterServiceTrait,
        MockStellarSignTrait,
        MockStellarDexServiceTrait,
    > {
        let network_repository = Arc::new(InMemoryNetworkRepository::new());
        let test_network = create_test_network();
        network_repository.create(test_network).await.unwrap();

        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());
        let counter = Arc::new(MockTransactionCounterServiceTrait::new());
        let signer = Arc::new(MockStellarSignTrait::new());

        crate::domain::relayer::stellar::StellarRelayer::new(
            relayer_model,
            signer,
            provider,
            crate::domain::relayer::stellar::StellarRelayerDependencies::new(
                relayer_repo,
                network_repository,
                tx_repo,
                counter,
                job_producer,
            ),
            dex_service,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn test_quote_sponsored_transaction_with_xdr() {
        let relayer_model = create_test_relayer_with_user_fee_strategy();
        let mut provider = MockStellarProviderTrait::new();

        // Mock account for validation
        provider.expect_get_account().returning(|_| {
            Box::pin(ready(Ok(AccountEntry {
                account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                balance: 1000000000,
                seq_num: SequenceNumber(1),
                num_sub_entries: 0,
                inflation_dest: None,
                flags: 0,
                home_domain: String32::default(),
                thresholds: Thresholds([0; 4]),
                signers: VecM::default(),
                ext: AccountEntryExt::V0,
            })))
        });

        // Mock get_ledger_entries for token balance validation
        // This mock extracts the account ID from the ledger key and returns a trustline with sufficient balance
        provider.expect_get_ledger_entries().returning(|keys| {
            // Extract account ID from the first ledger key (should be a Trustline key)
            let account_id = if let Some(LedgerKey::Trustline(trustline_key)) = keys.first() {
                trustline_key.account_id.clone()
            } else {
                // Fallback: try to parse TEST_PK
                parse_account_id(TEST_PK).unwrap_or_else(|_| {
                    AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32])))
                })
            };

            let issuer_id =
                parse_account_id("GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN")
                    .unwrap_or_else(|_| {
                        AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32])))
                    });

            // Create a trustline entry with sufficient balance
            let trustline_entry = TrustLineEntry {
                account_id,
                asset: soroban_rs::xdr::TrustLineAsset::CreditAlphanum4(AlphaNum4 {
                    asset_code: AssetCode4(*b"USDC"),
                    issuer: issuer_id,
                }),
                balance: 10_000_000i64,
                limit: i64::MAX,
                flags: 0,
                ext: TrustLineEntryExt::V0,
            };

            let ledger_entry = LedgerEntry {
                last_modified_ledger_seq: 0,
                data: LedgerEntryData::Trustline(trustline_entry),
                ext: LedgerEntryExt::V0,
            };

            // Encode LedgerEntryData to XDR base64 (not the full LedgerEntry)
            let xdr = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .expect("Failed to encode trustline entry data to XDR");

            Box::pin(ready(Ok(GetLedgerEntriesResponse {
                entries: Some(vec![LedgerEntryResult {
                    key: "test_key".to_string(),
                    xdr,
                    last_modified_ledger: 0u32,
                    live_until_ledger_seq_ledger_seq: None,
                }]),
                latest_ledger: 0,
            })))
        });

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service
            .expect_supported_asset_types()
            .returning(|| std::collections::HashSet::from([AssetType::Native, AssetType::Classic]));

        // Mock get_xlm_to_token_quote for fee conversion (XLM -> token)
        dex_service
            .expect_get_xlm_to_token_quote()
            .returning(|_, _, _, _| {
                Box::pin(ready(Ok(
                    crate::services::stellar_dex::StellarQuoteResponse {
                        input_asset: "native".to_string(),
                        output_asset: USDC_ASSET.to_string(),
                        in_amount: 100000,
                        out_amount: 1500000,
                        price_impact_pct: 0.0,
                        slippage_bps: 100,
                        path: None,
                    },
                )))
            });

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let transaction_xdr = create_test_transaction_xdr();
        let request = SponsoredTransactionQuoteRequest::Stellar(
            crate::models::StellarFeeEstimateRequestParams {
                transaction_xdr: Some(transaction_xdr),
                operations: None,
                source_account: None,
                fee_token: USDC_ASSET.to_string(),
            },
        );

        let result = relayer.quote_sponsored_transaction(request).await;
        if let Err(e) = &result {
            eprintln!("Quote error: {:?}", e);
        }
        assert!(result.is_ok());

        if let SponsoredTransactionQuoteResponse::Stellar(quote) = result.unwrap() {
            assert_eq!(quote.fee_in_token, "1500000");
            assert!(!quote.fee_in_token_ui.is_empty());
            assert!(!quote.conversion_rate.is_empty());
        } else {
            panic!("Expected Stellar quote response");
        }
    }

    #[tokio::test]
    async fn test_quote_sponsored_transaction_with_operations() {
        let relayer_model = create_test_relayer_with_user_fee_strategy();
        let mut provider = MockStellarProviderTrait::new();

        provider.expect_get_account().returning(|_| {
            Box::pin(ready(Ok(AccountEntry {
                account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                balance: 1000000000,
                seq_num: SequenceNumber(-1),
                num_sub_entries: 0,
                inflation_dest: None,
                flags: 0,
                home_domain: String32::default(),
                thresholds: Thresholds([0; 4]),
                signers: VecM::default(),
                ext: AccountEntryExt::V0,
            })))
        });

        // Mock get_ledger_entries for token balance validation
        // This mock extracts the account ID from the ledger key and returns a trustline with sufficient balance
        provider.expect_get_ledger_entries().returning(|keys| {
            // Extract account ID from the first ledger key (should be a Trustline key)
            let account_id = if let Some(LedgerKey::Trustline(trustline_key)) = keys.first() {
                trustline_key.account_id.clone()
            } else {
                // Fallback: use the source account from the test
                parse_account_id("GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2")
                    .unwrap_or_else(|_| {
                        AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32])))
                    })
            };

            let issuer_id =
                parse_account_id("GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN")
                    .unwrap_or_else(|_| {
                        AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32])))
                    });

            // Create a trustline entry with sufficient balance
            let trustline_entry = TrustLineEntry {
                account_id,
                asset: soroban_rs::xdr::TrustLineAsset::CreditAlphanum4(AlphaNum4 {
                    asset_code: AssetCode4(*b"USDC"),
                    issuer: issuer_id,
                }),
                balance: 10_000_000i64,
                limit: i64::MAX,
                flags: 0,
                ext: TrustLineEntryExt::V0,
            };

            let ledger_entry = LedgerEntry {
                last_modified_ledger_seq: 0,
                data: LedgerEntryData::Trustline(trustline_entry),
                ext: LedgerEntryExt::V0,
            };

            // Encode LedgerEntryData to XDR base64 (not the full LedgerEntry)
            let xdr = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .expect("Failed to encode trustline entry data to XDR");

            Box::pin(ready(Ok(
                soroban_rs::stellar_rpc_client::GetLedgerEntriesResponse {
                    entries: Some(vec![LedgerEntryResult {
                        key: "test_key".to_string(),
                        xdr,
                        last_modified_ledger: 0u32,
                        live_until_ledger_seq_ledger_seq: None,
                    }]),
                    latest_ledger: 0,
                },
            )))
        });

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service
            .expect_supported_asset_types()
            .returning(|| std::collections::HashSet::from([AssetType::Native, AssetType::Classic]));

        // Mock get_xlm_to_token_quote for fee conversion (XLM -> token)
        dex_service
            .expect_get_xlm_to_token_quote()
            .returning(|_, _, _, _| {
                Box::pin(ready(Ok(
                    crate::services::stellar_dex::StellarQuoteResponse {
                        input_asset: "native".to_string(),
                        output_asset: USDC_ASSET.to_string(),
                        in_amount: 100000,
                        out_amount: 1500000,
                        price_impact_pct: 0.0,
                        slippage_bps: 100,
                        path: None,
                    },
                )))
            });

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let operations = vec![OperationSpec::Payment {
            destination: TEST_PK.to_string(),
            amount: 1000000,
            asset: AssetSpec::Native,
        }];

        let request = SponsoredTransactionQuoteRequest::Stellar(
            crate::models::StellarFeeEstimateRequestParams {
                transaction_xdr: None,
                operations: Some(operations),
                source_account: Some(
                    "GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2".to_string(),
                ),
                fee_token: USDC_ASSET.to_string(),
            },
        );

        let result = relayer.quote_sponsored_transaction(request).await;
        if let Err(e) = &result {
            eprintln!("Quote error: {:?}", e);
        }
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_quote_sponsored_transaction_invalid_token() {
        let relayer_model = create_test_relayer_with_user_fee_strategy();
        let provider = MockStellarProviderTrait::new();
        let dex_service = create_mock_dex_service();
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let transaction_xdr = create_test_transaction_xdr();
        let request = SponsoredTransactionQuoteRequest::Stellar(
            crate::models::StellarFeeEstimateRequestParams {
                transaction_xdr: Some(transaction_xdr),
                operations: None,
                source_account: None,
                fee_token: "INVALID:TOKEN".to_string(),
            },
        );

        let result = relayer.quote_sponsored_transaction(request).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RelayerError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_quote_sponsored_transaction_missing_xdr_and_operations() {
        let relayer_model = create_test_relayer_with_user_fee_strategy();
        let provider = MockStellarProviderTrait::new();
        let dex_service = create_mock_dex_service();
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let request = SponsoredTransactionQuoteRequest::Stellar(
            crate::models::StellarFeeEstimateRequestParams {
                transaction_xdr: None,
                operations: None,
                source_account: None,
                fee_token: USDC_ASSET.to_string(),
            },
        );

        let result = relayer.quote_sponsored_transaction(request).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RelayerError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_build_sponsored_transaction_with_xdr() {
        let relayer_model = create_test_relayer_with_user_fee_strategy();
        let mut provider = MockStellarProviderTrait::new();

        provider.expect_get_account().returning(|_| {
            Box::pin(ready(Ok(AccountEntry {
                account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                balance: 1000000000,
                seq_num: SequenceNumber(-1),
                num_sub_entries: 0,
                inflation_dest: None,
                flags: 0,
                home_domain: String32::default(),
                thresholds: Thresholds([0; 4]),
                signers: VecM::default(),
                ext: AccountEntryExt::V0,
            })))
        });

        // Mock get_ledger_entries for token balance validation
        // This mock extracts the account ID from the ledger key and returns a trustline with sufficient balance
        provider.expect_get_ledger_entries().returning(|keys| {
            // Extract account ID from the first ledger key (should be a Trustline key)
            let account_id = if let Some(LedgerKey::Trustline(trustline_key)) = keys.first() {
                trustline_key.account_id.clone()
            } else {
                // Fallback: try to parse TEST_PK
                parse_account_id(TEST_PK).unwrap_or_else(|_| {
                    AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32])))
                })
            };

            let issuer_id =
                parse_account_id("GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN")
                    .unwrap_or_else(|_| {
                        AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32])))
                    });

            // Create a trustline entry with sufficient balance (10 USDC = 10000000 with 6 decimals)
            let trustline_entry = TrustLineEntry {
                account_id,
                asset: soroban_rs::xdr::TrustLineAsset::CreditAlphanum4(AlphaNum4 {
                    asset_code: AssetCode4(*b"USDC"),
                    issuer: issuer_id,
                }),
                balance: 10_000_000i64, // 10 USDC (with 6 decimals) - sufficient for fee
                limit: i64::MAX,
                flags: 0,
                ext: TrustLineEntryExt::V0, // V0 has no liabilities
            };

            let ledger_entry = LedgerEntry {
                last_modified_ledger_seq: 0,
                data: LedgerEntryData::Trustline(trustline_entry),
                ext: LedgerEntryExt::V0,
            };

            // Encode LedgerEntryData to XDR base64 (not the full LedgerEntry)
            let xdr = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .expect("Failed to encode trustline entry data to XDR");

            Box::pin(ready(Ok(
                soroban_rs::stellar_rpc_client::GetLedgerEntriesResponse {
                    entries: Some(vec![LedgerEntryResult {
                        key: "test_key".to_string(),
                        xdr,
                        last_modified_ledger: 0u32,
                        live_until_ledger_seq_ledger_seq: None,
                    }]),
                    latest_ledger: 0,
                },
            )))
        });

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service
            .expect_supported_asset_types()
            .returning(|| std::collections::HashSet::from([AssetType::Native, AssetType::Classic]));

        // Mock get_xlm_to_token_quote for build (converting XLM fee to token)
        dex_service
            .expect_get_xlm_to_token_quote()
            .returning(|_, _, _, _| {
                Box::pin(ready(Ok(
                    crate::services::stellar_dex::StellarQuoteResponse {
                        input_asset: "native".to_string(),
                        output_asset: USDC_ASSET.to_string(),
                        in_amount: 1000000,
                        out_amount: 1500000,
                        price_impact_pct: 0.0,
                        slippage_bps: 100,
                        path: None,
                    },
                )))
            });

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let transaction_xdr = create_test_transaction_xdr();
        let request = SponsoredTransactionBuildRequest::Stellar(
            crate::models::StellarPrepareTransactionRequestParams {
                transaction_xdr: Some(transaction_xdr),
                operations: None,
                source_account: None,
                fee_token: USDC_ASSET.to_string(),
            },
        );

        let result = relayer.build_sponsored_transaction(request).await;
        assert!(result.is_ok());

        if let SponsoredTransactionBuildResponse::Stellar(build) = result.unwrap() {
            assert!(!build.transaction.is_empty());
            assert_eq!(build.fee_in_token, "1500000");
            assert!(!build.fee_in_token_ui.is_empty());
            assert_eq!(build.fee_token, USDC_ASSET);
            assert!(!build.valid_until.is_empty());
        } else {
            panic!("Expected Stellar build response");
        }
    }

    #[tokio::test]
    async fn test_build_sponsored_transaction_with_operations() {
        let relayer_model = create_test_relayer_with_user_fee_strategy();
        let mut provider = MockStellarProviderTrait::new();

        provider.expect_get_account().returning(|_| {
            Box::pin(ready(Ok(AccountEntry {
                account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                balance: 1000000000,
                seq_num: SequenceNumber(-1),
                num_sub_entries: 0,
                inflation_dest: None,
                flags: 0,
                home_domain: String32::default(),
                thresholds: Thresholds([0; 4]),
                signers: VecM::default(),
                ext: AccountEntryExt::V0,
            })))
        });

        provider.expect_get_ledger_entries().returning(|_| {
            use crate::domain::transaction::stellar::utils::parse_account_id;
            use soroban_rs::stellar_rpc_client::LedgerEntryResult;
            use soroban_rs::xdr::{
                AccountId, AlphaNum4, AssetCode4, LedgerEntry, LedgerEntryData, LedgerEntryExt,
                PublicKey, TrustLineEntry, TrustLineEntryExt, Uint256, WriteXdr,
            };

            // Parse account IDs - use the source account from the test
            let account_id =
                parse_account_id("GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2")
                    .unwrap_or_else(|_| {
                        AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32])))
                    });
            let issuer_id =
                parse_account_id("GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN")
                    .unwrap_or_else(|_| {
                        AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32])))
                    });

            // Create a trustline entry with sufficient balance (10 USDC = 10000000 with 6 decimals)
            // The fee is 1500000 (from the quote), so 10 USDC is more than enough
            let trustline_entry = TrustLineEntry {
                account_id,
                asset: soroban_rs::xdr::TrustLineAsset::CreditAlphanum4(AlphaNum4 {
                    asset_code: AssetCode4(*b"USDC"),
                    issuer: issuer_id,
                }),
                balance: 10_000_000i64,
                limit: i64::MAX,
                flags: 0,
                ext: TrustLineEntryExt::V0,
            };

            let ledger_entry = LedgerEntry {
                last_modified_ledger_seq: 0,
                data: LedgerEntryData::Trustline(trustline_entry),
                ext: LedgerEntryExt::V0,
            };

            // Encode LedgerEntryData to XDR base64 (not the full LedgerEntry)
            // The parse_ledger_entry_from_xdr function expects just the data portion
            let xdr = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .expect("Failed to encode trustline entry data to XDR");

            Box::pin(ready(Ok(
                soroban_rs::stellar_rpc_client::GetLedgerEntriesResponse {
                    entries: Some(vec![LedgerEntryResult {
                        key: "test_key".to_string(),
                        xdr,
                        last_modified_ledger: 0u32,
                        live_until_ledger_seq_ledger_seq: None,
                    }]),
                    latest_ledger: 0,
                },
            )))
        });

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service
            .expect_supported_asset_types()
            .returning(|| std::collections::HashSet::from([AssetType::Native, AssetType::Classic]));

        dex_service
            .expect_get_xlm_to_token_quote()
            .returning(|_, _, _, _| {
                Box::pin(ready(Ok(
                    crate::services::stellar_dex::StellarQuoteResponse {
                        input_asset: "native".to_string(),
                        output_asset: USDC_ASSET.to_string(),
                        in_amount: 1000000,
                        out_amount: 1500000,
                        price_impact_pct: 0.0,
                        slippage_bps: 100,
                        path: None,
                    },
                )))
            });

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let operations = vec![OperationSpec::Payment {
            destination: TEST_PK.to_string(),
            amount: 1000000,
            asset: AssetSpec::Native,
        }];

        let request = SponsoredTransactionBuildRequest::Stellar(
            crate::models::StellarPrepareTransactionRequestParams {
                transaction_xdr: None,
                operations: Some(operations),
                source_account: Some(
                    "GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2".to_string(),
                ),
                fee_token: USDC_ASSET.to_string(),
            },
        );

        let result = relayer.build_sponsored_transaction(request).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_build_sponsored_transaction_missing_source_account() {
        let relayer_model = create_test_relayer_with_user_fee_strategy();
        let provider = MockStellarProviderTrait::new();
        let dex_service = create_mock_dex_service();
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let operations = vec![OperationSpec::Payment {
            destination: TEST_PK.to_string(),
            amount: 1000000,
            asset: AssetSpec::Native,
        }];

        let request = SponsoredTransactionBuildRequest::Stellar(
            crate::models::StellarPrepareTransactionRequestParams {
                transaction_xdr: None,
                operations: Some(operations),
                source_account: None,
                fee_token: USDC_ASSET.to_string(),
            },
        );

        let result = relayer.build_sponsored_transaction(request).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RelayerError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_build_envelope_from_request_with_xdr() {
        let provider = MockStellarProviderTrait::new();
        let transaction_xdr = create_test_transaction_xdr();
        let result = build_envelope_from_request(
            Some(&transaction_xdr),
            None,
            None,
            TEST_NETWORK_PASSPHRASE,
            &provider,
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_build_envelope_from_request_with_operations() {
        let mut provider = MockStellarProviderTrait::new();

        // Mock get_account to return a valid account with sequence number
        provider.expect_get_account().returning(|_| {
            Box::pin(ready(Ok(AccountEntry {
                account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                balance: 1000000000,
                seq_num: SequenceNumber(100),
                num_sub_entries: 0,
                inflation_dest: None,
                flags: 0,
                home_domain: String32::default(),
                thresholds: Thresholds([0; 4]),
                signers: VecM::default(),
                ext: AccountEntryExt::V0,
            })))
        });

        let operations = vec![OperationSpec::Payment {
            destination: TEST_PK.to_string(),
            amount: 1000000,
            asset: AssetSpec::Native,
        }];

        let result = build_envelope_from_request(
            None,
            Some(&operations),
            Some(&TEST_PK.to_string()),
            TEST_NETWORK_PASSPHRASE,
            &provider,
        )
        .await;
        assert!(result.is_ok());

        // Verify the sequence number is set correctly (current + 1 = 101)
        if let Ok(envelope) = result {
            if let TransactionEnvelope::Tx(tx_env) = envelope {
                assert_eq!(tx_env.tx.seq_num.0, 101);
            }
        }
    }

    #[tokio::test]
    async fn test_build_envelope_from_request_missing_source_account() {
        let provider = MockStellarProviderTrait::new();
        let operations = vec![OperationSpec::Payment {
            destination: TEST_PK.to_string(),
            amount: 1000000,
            asset: AssetSpec::Native,
        }];

        let result = build_envelope_from_request(
            None,
            Some(&operations),
            None,
            TEST_NETWORK_PASSPHRASE,
            &provider,
        )
        .await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RelayerError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_build_envelope_from_request_missing_both() {
        let provider = MockStellarProviderTrait::new();
        let result =
            build_envelope_from_request(None, None, None, TEST_NETWORK_PASSPHRASE, &provider).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RelayerError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_build_envelope_from_request_invalid_xdr() {
        let provider = MockStellarProviderTrait::new();
        let result = build_envelope_from_request(
            Some(&"INVALID_XDR".to_string()),
            None,
            None,
            TEST_NETWORK_PASSPHRASE,
            &provider,
        )
        .await;
        assert!(result.is_err());
    }

    // ============================================================================
    // Tests for detect_soroban_invoke_from_xdr
    // ============================================================================

    #[test]
    fn test_detect_soroban_invoke_from_xdr_classic_transaction() {
        // Classic payment transaction should return None
        let xdr = create_test_transaction_xdr();
        let result = detect_soroban_invoke_from_xdr(&xdr);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_detect_soroban_invoke_from_xdr_invalid_xdr() {
        let result = detect_soroban_invoke_from_xdr("INVALID_XDR");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RelayerError::ValidationError(_)
        ));
    }

    #[test]
    fn test_detect_soroban_invoke_from_xdr_with_soroban_transaction() {
        use soroban_rs::xdr::{
            ContractId, Hash, HostFunction, InvokeContractArgs, InvokeHostFunctionOp, Memo,
            MuxedAccount, Operation, OperationBody, Preconditions, ScAddress, ScSymbol, ScVal,
            SequenceNumber, Transaction, TransactionEnvelope, TransactionExt,
            TransactionV1Envelope, Uint256, VecM,
        };

        // Create a Soroban InvokeHostFunction transaction
        let contract_id = ContractId(Hash([1u8; 32]));
        let invoke_args = InvokeContractArgs {
            contract_address: ScAddress::Contract(contract_id),
            function_name: ScSymbol("test_function".try_into().unwrap()),
            args: vec![ScVal::Bool(true)].try_into().unwrap(),
        };

        let invoke_op = InvokeHostFunctionOp {
            host_function: HostFunction::InvokeContract(invoke_args),
            auth: VecM::default(),
        };

        let operation = Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(invoke_op),
        };

        let source_pk = Ed25519PublicKey::from_string(
            "GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2",
        )
        .unwrap();

        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
            fee: 100,
            seq_num: SequenceNumber(1),
            cond: Preconditions::None,
            memo: Memo::None,
            operations: vec![operation].try_into().unwrap(),
            ext: TransactionExt::V0,
        };

        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        });

        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();
        let result = detect_soroban_invoke_from_xdr(&xdr);
        assert!(result.is_ok());

        let soroban_info = result.unwrap();
        assert!(soroban_info.is_some());

        let info = soroban_info.unwrap();
        assert_eq!(info.target_fn, "test_function");
        assert_eq!(info.target_args.len(), 1);
        // Verify contract address format (C...)
        assert!(info.target_contract.starts_with('C'));
    }

    #[test]
    fn test_detect_soroban_invoke_from_xdr_multiple_operations_error() {
        use soroban_rs::xdr::{
            ContractId, Hash, HostFunction, InvokeContractArgs, InvokeHostFunctionOp, Memo,
            MuxedAccount, Operation, OperationBody, PaymentOp, Preconditions, ScAddress, ScSymbol,
            SequenceNumber, Transaction, TransactionEnvelope, TransactionExt,
            TransactionV1Envelope, Uint256, VecM,
        };

        // Create a transaction with InvokeHostFunction AND another operation (invalid for Soroban)
        let contract_id = ContractId(Hash([1u8; 32]));
        let invoke_args = InvokeContractArgs {
            contract_address: ScAddress::Contract(contract_id),
            function_name: ScSymbol("test".try_into().unwrap()),
            args: VecM::default(),
        };

        let invoke_op = InvokeHostFunctionOp {
            host_function: HostFunction::InvokeContract(invoke_args),
            auth: VecM::default(),
        };

        let source_pk = Ed25519PublicKey::from_string(
            "GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2",
        )
        .unwrap();
        let dest_pk = Ed25519PublicKey::from_string(
            "GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ",
        )
        .unwrap();

        let payment_op = PaymentOp {
            destination: MuxedAccount::Ed25519(Uint256(dest_pk.0)),
            asset: soroban_rs::xdr::Asset::Native,
            amount: 1000000,
        };

        let operations: VecM<Operation, 100> = vec![
            Operation {
                source_account: None,
                body: OperationBody::InvokeHostFunction(invoke_op),
            },
            Operation {
                source_account: None,
                body: OperationBody::Payment(payment_op),
            },
        ]
        .try_into()
        .unwrap();

        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
            fee: 100,
            seq_num: SequenceNumber(1),
            cond: Preconditions::None,
            memo: Memo::None,
            operations,
            ext: TransactionExt::V0,
        };

        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        });

        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();
        let result = detect_soroban_invoke_from_xdr(&xdr);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, RelayerError::ValidationError(_)));
        if let RelayerError::ValidationError(msg) = err {
            assert!(msg.contains("exactly one operation"));
        }
    }

    #[test]
    fn test_detect_soroban_invoke_from_xdr_v0_envelope() {
        use soroban_rs::xdr::{
            Memo, Operation, OperationBody, PaymentOp, SequenceNumber, TransactionEnvelope,
            TransactionV0, TransactionV0Envelope, TransactionV0Ext, Uint256, VecM,
        };

        // Create a V0 envelope (legacy format)
        let source_pk = Ed25519PublicKey::from_string(
            "GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2",
        )
        .unwrap();
        let dest_pk = Ed25519PublicKey::from_string(
            "GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ",
        )
        .unwrap();

        let payment_op = PaymentOp {
            destination: MuxedAccount::Ed25519(Uint256(dest_pk.0)),
            asset: soroban_rs::xdr::Asset::Native,
            amount: 1000000,
        };

        let tx = TransactionV0 {
            source_account_ed25519: Uint256(source_pk.0),
            fee: 100,
            seq_num: SequenceNumber(1),
            time_bounds: None,
            memo: Memo::None,
            operations: vec![Operation {
                source_account: None,
                body: OperationBody::Payment(payment_op),
            }]
            .try_into()
            .unwrap(),
            ext: TransactionV0Ext::V0,
        };

        let envelope = TransactionEnvelope::TxV0(TransactionV0Envelope {
            tx,
            signatures: VecM::default(),
        });

        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();
        let result = detect_soroban_invoke_from_xdr(&xdr);

        // V0 envelope with classic operation should return None
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_detect_soroban_invoke_from_xdr_fee_bump_envelope() {
        use soroban_rs::xdr::{
            FeeBumpTransaction, FeeBumpTransactionEnvelope, FeeBumpTransactionExt,
            FeeBumpTransactionInnerTx, Memo, MuxedAccount, Operation, OperationBody, PaymentOp,
            Preconditions, SequenceNumber, Transaction, TransactionEnvelope, TransactionExt,
            TransactionV1Envelope, Uint256, VecM,
        };

        let source_pk = Ed25519PublicKey::from_string(
            "GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2",
        )
        .unwrap();
        let dest_pk = Ed25519PublicKey::from_string(
            "GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ",
        )
        .unwrap();

        let payment_op = PaymentOp {
            destination: MuxedAccount::Ed25519(Uint256(dest_pk.0)),
            asset: soroban_rs::xdr::Asset::Native,
            amount: 1000000,
        };

        let inner_tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
            fee: 100,
            seq_num: SequenceNumber(1),
            cond: Preconditions::None,
            memo: Memo::None,
            operations: vec![Operation {
                source_account: None,
                body: OperationBody::Payment(payment_op),
            }]
            .try_into()
            .unwrap(),
            ext: TransactionExt::V0,
        };

        let inner_envelope = TransactionV1Envelope {
            tx: inner_tx,
            signatures: VecM::default(),
        };

        let fee_bump_tx = FeeBumpTransaction {
            fee_source: MuxedAccount::Ed25519(Uint256(source_pk.0)),
            fee: 200,
            inner_tx: FeeBumpTransactionInnerTx::Tx(inner_envelope),
            ext: FeeBumpTransactionExt::V0,
        };

        let envelope = TransactionEnvelope::TxFeeBump(FeeBumpTransactionEnvelope {
            tx: fee_bump_tx,
            signatures: VecM::default(),
        });

        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();
        let result = detect_soroban_invoke_from_xdr(&xdr);

        // Fee bump with classic operation should return None
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_detect_soroban_invoke_non_contract_address_error() {
        use soroban_rs::xdr::{
            HostFunction, InvokeContractArgs, InvokeHostFunctionOp, Memo, MuxedAccount, Operation,
            OperationBody, Preconditions, ScAddress, ScSymbol, SequenceNumber, Transaction,
            TransactionEnvelope, TransactionExt, TransactionV1Envelope, Uint256, VecM,
        };

        // Create a Soroban transaction with account address instead of contract address
        let source_pk = Ed25519PublicKey::from_string(
            "GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2",
        )
        .unwrap();

        let invoke_args = InvokeContractArgs {
            contract_address: ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(
                Uint256(source_pk.0),
            ))),
            function_name: ScSymbol("test".try_into().unwrap()),
            args: VecM::default(),
        };

        let invoke_op = InvokeHostFunctionOp {
            host_function: HostFunction::InvokeContract(invoke_args),
            auth: VecM::default(),
        };

        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
            fee: 100,
            seq_num: SequenceNumber(1),
            cond: Preconditions::None,
            memo: Memo::None,
            operations: vec![Operation {
                source_account: None,
                body: OperationBody::InvokeHostFunction(invoke_op),
            }]
            .try_into()
            .unwrap(),
            ext: TransactionExt::V0,
        };

        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        });

        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();
        let result = detect_soroban_invoke_from_xdr(&xdr);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, RelayerError::ValidationError(_)));
        if let RelayerError::ValidationError(msg) = err {
            assert!(msg.contains("contract address"));
        }
    }

    // ============================================================================
    // Tests for apply_max_fee_slippage
    // ============================================================================

    #[test]
    fn test_apply_max_fee_slippage_basic() {
        // 5% slippage on 10000 should give 10500
        let result = apply_max_fee_slippage(10000);
        assert_eq!(result, 10500);
    }

    #[test]
    fn test_apply_max_fee_slippage_zero() {
        let result = apply_max_fee_slippage(0);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_apply_max_fee_slippage_large_value() {
        // Test with a large value to ensure no overflow
        let large_fee: u64 = 1_000_000_000_000;
        let result = apply_max_fee_slippage(large_fee);
        // 5% of 1 trillion = 50 billion, so result = 1.05 trillion
        assert_eq!(result, 1_050_000_000_000i128);
    }

    #[test]
    fn test_apply_max_fee_slippage_small_value() {
        // Small value: 100 * 10500 / 10000 = 105
        let result = apply_max_fee_slippage(100);
        assert_eq!(result, 105);
    }

    // ============================================================================
    // Tests for calculate_total_soroban_fee
    // ============================================================================

    #[test]
    fn test_calculate_total_soroban_fee_success() {
        let sim_response = soroban_rs::stellar_rpc_client::SimulateTransactionResponse {
            error: None,
            transaction_data: "".to_string(),
            min_resource_fee: 50000,
            ..Default::default()
        };

        let result = calculate_total_soroban_fee(&sim_response, 1);
        assert!(result.is_ok());
        // inclusion_fee (100) + resource_fee (50000) = 50100
        let fee = result.unwrap();
        assert_eq!(fee, 50100);
    }

    #[test]
    fn test_calculate_total_soroban_fee_with_multiple_operations() {
        let sim_response = soroban_rs::stellar_rpc_client::SimulateTransactionResponse {
            error: None,
            transaction_data: "".to_string(),
            min_resource_fee: 50000,
            ..Default::default()
        };

        let result = calculate_total_soroban_fee(&sim_response, 3);
        assert!(result.is_ok());
        // inclusion_fee (100 * 3) + resource_fee (50000) = 50300
        let fee = result.unwrap();
        assert_eq!(fee, 50300);
    }

    #[test]
    fn test_calculate_total_soroban_fee_simulation_error() {
        let sim_response = soroban_rs::stellar_rpc_client::SimulateTransactionResponse {
            error: Some("Simulation failed: insufficient funds".to_string()),
            transaction_data: "".to_string(),
            min_resource_fee: 0,
            ..Default::default()
        };

        let result = calculate_total_soroban_fee(&sim_response, 1);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, RelayerError::ValidationError(_)));
        if let RelayerError::ValidationError(msg) = err {
            assert!(msg.contains("Simulation failed"));
        }
    }

    #[test]
    fn test_calculate_total_soroban_fee_minimum_fee() {
        // When calculated fee is less than minimum, should return minimum
        let sim_response = soroban_rs::stellar_rpc_client::SimulateTransactionResponse {
            error: None,
            transaction_data: "".to_string(),
            min_resource_fee: 0, // Very low resource fee
            ..Default::default()
        };

        let result = calculate_total_soroban_fee(&sim_response, 1);
        assert!(result.is_ok());
        // Should be at least STELLAR_DEFAULT_TRANSACTION_FEE (100)
        let fee = result.unwrap();
        assert!(fee >= STELLAR_DEFAULT_TRANSACTION_FEE);
    }

    // ============================================================================
    // Tests for build_soroban_transaction_envelope
    // ============================================================================

    #[test]
    fn test_build_soroban_transaction_envelope_success() {
        use soroban_rs::xdr::{
            ContractId, Hash, HostFunction, InvokeContractArgs, InvokeHostFunctionOp, Operation,
            OperationBody, ScAddress, ScSymbol, VecM,
        };

        let contract_id = ContractId(Hash([1u8; 32]));
        let invoke_args = InvokeContractArgs {
            contract_address: ScAddress::Contract(contract_id),
            function_name: ScSymbol("test".try_into().unwrap()),
            args: VecM::default(),
        };

        let invoke_op = InvokeHostFunctionOp {
            host_function: HostFunction::InvokeContract(invoke_args),
            auth: VecM::default(),
        };

        let operation = Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(invoke_op),
        };

        let result = build_soroban_transaction_envelope(TEST_PK, operation.clone(), 100);
        assert!(result.is_ok());

        let envelope = result.unwrap();
        if let TransactionEnvelope::Tx(tx_env) = envelope {
            assert_eq!(tx_env.tx.fee, 100);
            assert_eq!(tx_env.tx.seq_num.0, 0); // Placeholder sequence
            assert_eq!(tx_env.tx.operations.len(), 1);
        } else {
            panic!("Expected Tx envelope");
        }
    }

    #[test]
    fn test_build_soroban_transaction_envelope_invalid_source() {
        use soroban_rs::xdr::{
            ContractId, Hash, HostFunction, InvokeContractArgs, InvokeHostFunctionOp, Operation,
            OperationBody, ScAddress, ScSymbol, VecM,
        };

        let contract_id = ContractId(Hash([1u8; 32]));
        let invoke_args = InvokeContractArgs {
            contract_address: ScAddress::Contract(contract_id),
            function_name: ScSymbol("test".try_into().unwrap()),
            args: VecM::default(),
        };

        let invoke_op = InvokeHostFunctionOp {
            host_function: HostFunction::InvokeContract(invoke_args),
            auth: VecM::default(),
        };

        let operation = Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(invoke_op),
        };

        let result = build_soroban_transaction_envelope("INVALID_ADDRESS", operation, 100);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RelayerError::ValidationError(_)
        ));
    }

    // ============================================================================
    // Tests for get_expiration_ledger
    // ============================================================================

    #[tokio::test]
    async fn test_get_expiration_ledger_success() {
        let mut provider = MockStellarProviderTrait::new();
        provider.expect_get_latest_ledger().returning(|| {
            Box::pin(ready(Ok(
                soroban_rs::stellar_rpc_client::GetLatestLedgerResponse {
                    id: "test".to_string(),
                    protocol_version: 20,
                    sequence: 1000,
                },
            )))
        });

        // 300 seconds / 5 seconds per ledger = 60 ledgers
        let result = get_expiration_ledger(&provider, 300).await;
        assert!(result.is_ok());
        let expiration = result.unwrap();
        assert_eq!(expiration, 1060); // 1000 + 60
    }

    #[tokio::test]
    async fn test_get_expiration_ledger_zero_seconds() {
        let mut provider = MockStellarProviderTrait::new();
        provider.expect_get_latest_ledger().returning(|| {
            Box::pin(ready(Ok(
                soroban_rs::stellar_rpc_client::GetLatestLedgerResponse {
                    id: "test".to_string(),
                    protocol_version: 20,
                    sequence: 1000,
                },
            )))
        });

        // 0 seconds should still add at least 1 ledger
        let result = get_expiration_ledger(&provider, 0).await;
        assert!(result.is_ok());
        let expiration = result.unwrap();
        assert_eq!(expiration, 1001); // 1000 + 1 (minimum)
    }

    // ============================================================================
    // Tests for add_payment_operation_to_envelope
    // ============================================================================

    #[test]
    fn test_add_payment_operation_to_envelope_classic() {
        let envelope = create_test_envelope_for_payment();
        let fee_quote = FeeQuote {
            fee_in_token: 1000000,
            fee_in_token_ui: "1.0".to_string(),
            fee_in_stroops: 10000,
            conversion_rate: 100.0,
        };

        let result = add_payment_operation_to_envelope(envelope, &fee_quote, USDC_ASSET, TEST_PK);
        assert!(result.is_ok());

        let updated_envelope = result.unwrap();
        // Classic transaction should have 2 operations now (original + payment)
        if let TransactionEnvelope::Tx(tx_env) = updated_envelope {
            assert_eq!(tx_env.tx.operations.len(), 2);
        }
    }

    #[test]
    fn test_add_payment_operation_to_envelope_soroban_no_op_added() {
        use soroban_rs::xdr::{
            ContractId, Hash, HostFunction, InvokeContractArgs, InvokeHostFunctionOp, Memo,
            Operation, OperationBody, Preconditions, ScAddress, ScSymbol, SequenceNumber,
            Transaction, TransactionEnvelope, TransactionExt, TransactionV1Envelope, Uint256, VecM,
        };

        // Create a Soroban transaction (InvokeHostFunction)
        let source_pk = Ed25519PublicKey::from_string(
            "GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2",
        )
        .unwrap();

        let contract_id = ContractId(Hash([1u8; 32]));
        let invoke_args = InvokeContractArgs {
            contract_address: ScAddress::Contract(contract_id),
            function_name: ScSymbol("test".try_into().unwrap()),
            args: VecM::default(),
        };

        let invoke_op = InvokeHostFunctionOp {
            host_function: HostFunction::InvokeContract(invoke_args),
            auth: VecM::default(),
        };

        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
            fee: 100,
            seq_num: SequenceNumber(1),
            cond: Preconditions::None,
            memo: Memo::None,
            operations: vec![Operation {
                source_account: None,
                body: OperationBody::InvokeHostFunction(invoke_op),
            }]
            .try_into()
            .unwrap(),
            ext: TransactionExt::V0,
        };

        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        });

        let fee_quote = FeeQuote {
            fee_in_token: 1000000,
            fee_in_token_ui: "1.0".to_string(),
            fee_in_stroops: 10000,
            conversion_rate: 100.0,
        };

        let result = add_payment_operation_to_envelope(envelope, &fee_quote, USDC_ASSET, TEST_PK);
        assert!(result.is_ok());

        // Soroban transactions should NOT have payment operation added
        let updated_envelope = result.unwrap();
        if let TransactionEnvelope::Tx(tx_env) = updated_envelope {
            assert_eq!(tx_env.tx.operations.len(), 1); // Still only 1 operation
        }
    }

    /// Helper to create a test envelope for payment tests
    fn create_test_envelope_for_payment() -> TransactionEnvelope {
        let source_pk = Ed25519PublicKey::from_string(
            "GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2",
        )
        .unwrap();
        let dest_pk = Ed25519PublicKey::from_string(
            "GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ",
        )
        .unwrap();

        let payment_op = PaymentOp {
            destination: MuxedAccount::Ed25519(Uint256(dest_pk.0)),
            asset: soroban_rs::xdr::Asset::Native,
            amount: 1000000,
        };

        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
            fee: 100,
            seq_num: SequenceNumber(1),
            cond: Preconditions::None,
            memo: soroban_rs::xdr::Memo::None,
            operations: vec![Operation {
                source_account: None,
                body: OperationBody::Payment(payment_op),
            }]
            .try_into()
            .unwrap(),
            ext: TransactionExt::V0,
        };

        TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        })
    }

    // ============================================================================
    // Tests for add_fee_payment_operation
    // ============================================================================

    #[test]
    fn test_add_fee_payment_operation_success() {
        let mut envelope = create_test_envelope_for_payment();
        let result = add_fee_payment_operation(&mut envelope, USDC_ASSET, 1000000, TEST_PK);
        assert!(result.is_ok());

        // Verify operation was added
        if let TransactionEnvelope::Tx(tx_env) = envelope {
            assert_eq!(tx_env.tx.operations.len(), 2);
        }
    }

    #[test]
    fn test_add_fee_payment_operation_native_asset() {
        let mut envelope = create_test_envelope_for_payment();
        let result = add_fee_payment_operation(&mut envelope, "native", 1000000, TEST_PK);
        assert!(result.is_ok());
    }

    // ============================================================================
    // Tests for SorobanInvokeInfo
    // ============================================================================

    #[test]
    fn test_soroban_invoke_info_debug_clone() {
        use soroban_rs::xdr::ScVal;

        let info = SorobanInvokeInfo {
            target_contract: "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC".to_string(),
            target_fn: "transfer".to_string(),
            target_args: vec![ScVal::Bool(true)],
        };

        // Test Debug trait
        let debug_str = format!("{:?}", info);
        assert!(debug_str.contains("SorobanInvokeInfo"));
        assert!(debug_str.contains("transfer"));

        // Test Clone trait
        let cloned = info.clone();
        assert_eq!(cloned.target_contract, info.target_contract);
        assert_eq!(cloned.target_fn, info.target_fn);
        assert_eq!(cloned.target_args.len(), info.target_args.len());
    }

    // ============================================================================
    // Tests for fee payment strategy validation
    // ============================================================================

    #[tokio::test]
    async fn test_build_sponsored_transaction_non_user_fee_strategy() {
        // Create relayer with Relayer fee payment strategy (not User)
        let mut policy = RelayerStellarPolicy::default();
        policy.fee_payment_strategy = Some(crate::models::StellarFeePaymentStrategy::Relayer);
        policy.allowed_tokens = Some(vec![crate::models::StellarAllowedTokensPolicy {
            asset: USDC_ASSET.to_string(),
            metadata: None,
            max_allowed_fee: None,
            swap_config: None,
        }]);

        let relayer_model = RelayerRepoModel {
            id: "test-relayer-id".to_string(),
            name: "Test Relayer".to_string(),
            network: "testnet".to_string(),
            paused: false,
            network_type: NetworkType::Stellar,
            signer_id: "signer-id".to_string(),
            policies: RelayerNetworkPolicy::Stellar(policy),
            address: TEST_PK.to_string(),
            notification_id: Some("notification-id".to_string()),
            system_disabled: false,
            custom_rpc_urls: None,
            ..Default::default()
        };

        let provider = MockStellarProviderTrait::new();
        let dex_service = create_mock_dex_service();
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let transaction_xdr = create_test_transaction_xdr();
        let request = SponsoredTransactionBuildRequest::Stellar(
            crate::models::StellarPrepareTransactionRequestParams {
                transaction_xdr: Some(transaction_xdr),
                operations: None,
                source_account: None,
                fee_token: USDC_ASSET.to_string(),
            },
        );

        let result = relayer.build_sponsored_transaction(request).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, RelayerError::ValidationError(_)));
        if let RelayerError::ValidationError(msg) = err {
            assert!(msg.contains("fee_payment_strategy: User"));
        }
    }

    // ============================================================================
    // Tests for quote_soroban_from_xdr (via quote_sponsored_transaction)
    // ============================================================================

    /// Helper function to create a valid SorobanTransactionData XDR for mocking simulation responses
    fn create_valid_soroban_transaction_data_xdr() -> String {
        use soroban_rs::xdr::{
            LedgerFootprint, SorobanResources, SorobanTransactionData, SorobanTransactionDataExt,
        };

        let soroban_data = SorobanTransactionData {
            ext: SorobanTransactionDataExt::V0,
            resources: SorobanResources {
                footprint: LedgerFootprint {
                    read_only: VecM::default(),
                    read_write: VecM::default(),
                },
                instructions: 1000000,
                disk_read_bytes: 10000,
                write_bytes: 1000,
            },
            resource_fee: 50000,
        };

        soroban_data.to_xdr_base64(Limits::none()).unwrap()
    }

    /// Helper function to create a Soroban InvokeHostFunction transaction XDR
    fn create_test_soroban_transaction_xdr() -> String {
        use soroban_rs::xdr::{
            ContractId, Hash, HostFunction, InvokeContractArgs, InvokeHostFunctionOp, Memo,
            ScAddress, ScSymbol, ScVal,
        };

        let source_pk = Ed25519PublicKey::from_string(
            "GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2",
        )
        .unwrap();

        // Create a Soroban contract call operation
        let contract_id = ContractId(Hash([1u8; 32]));
        let invoke_args = InvokeContractArgs {
            contract_address: ScAddress::Contract(contract_id),
            function_name: ScSymbol("transfer".try_into().unwrap()),
            args: vec![ScVal::Bool(true)].try_into().unwrap(),
        };

        let invoke_op = InvokeHostFunctionOp {
            host_function: HostFunction::InvokeContract(invoke_args),
            auth: VecM::default(),
        };

        let operation = Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(invoke_op),
        };

        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
            fee: 100,
            seq_num: SequenceNumber(1),
            cond: Preconditions::None,
            memo: Memo::None,
            operations: vec![operation].try_into().unwrap(),
            ext: TransactionExt::V0,
        };

        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        });

        envelope.to_xdr_base64(Limits::none()).unwrap()
    }

    /// Helper function to create a relayer with Soroban token support
    fn create_test_relayer_with_soroban_token() -> RelayerRepoModel {
        let mut policy = RelayerStellarPolicy::default();
        policy.fee_payment_strategy = Some(crate::models::StellarFeePaymentStrategy::User);
        // Use a Soroban contract address (C...) as the allowed token
        policy.allowed_tokens = Some(vec![crate::models::StellarAllowedTokensPolicy {
            asset: "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC".to_string(),
            metadata: None,
            max_allowed_fee: None,
            swap_config: None,
        }]);

        RelayerRepoModel {
            id: "test-relayer-id".to_string(),
            name: "Test Relayer".to_string(),
            network: "testnet".to_string(),
            paused: false,
            network_type: NetworkType::Stellar,
            signer_id: "signer-id".to_string(),
            policies: RelayerNetworkPolicy::Stellar(policy),
            address: TEST_PK.to_string(),
            notification_id: Some("notification-id".to_string()),
            system_disabled: false,
            custom_rpc_urls: None,
            ..Default::default()
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_quote_soroban_from_xdr_success() {
        // Set required env var for FeeForwarder
        std::env::set_var(
            "STELLAR_FEE_FORWARDER_ADDRESS",
            "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
        );

        let relayer_model = create_test_relayer_with_soroban_token();
        let mut provider = MockStellarProviderTrait::new();

        // Mock get_latest_ledger for expiration calculation
        provider.expect_get_latest_ledger().returning(|| {
            Box::pin(ready(Ok(
                soroban_rs::stellar_rpc_client::GetLatestLedgerResponse {
                    id: "test".to_string(),
                    protocol_version: 20,
                    sequence: 1000,
                },
            )))
        });

        // Mock simulate_transaction_envelope for Soroban fee estimation
        provider
            .expect_simulate_transaction_envelope()
            .returning(|_| {
                Box::pin(ready(Ok(
                    soroban_rs::stellar_rpc_client::SimulateTransactionResponse {
                        min_resource_fee: 50000,
                        transaction_data: "AAAAAQAAAAAAAAACAAAAAAAAAAAAAAAAAAAABgAAAAEAAAAGAAAAAG0JZTO9fU6p3NeJp5w3TpKhZmx6p1pR7mq9wFwCnEIuAAAAFAAAAAEAAAAAAAAAB8NVb2IAAAH0AAAAAQAAAAAAABfAAAAAAAAAAPUAAAAAAAAENgAAAAA=".to_string(),
                        ..Default::default()
                    },
                )))
            });

        // Mock call_contract for Soroban token balance check (balance function)
        provider.expect_call_contract().returning(|_, _, _| {
            use soroban_rs::xdr::Int128Parts;
            // Return a balance of 10_000_000 (10 tokens with 6 decimals)
            Box::pin(ready(Ok(ScVal::I128(Int128Parts {
                hi: 0,
                lo: 10_000_000,
            }))))
        });

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service.expect_supported_asset_types().returning(|| {
            std::collections::HashSet::from([AssetType::Native, AssetType::Contract])
        });

        // Mock get_xlm_to_token_quote for fee conversion
        dex_service
            .expect_get_xlm_to_token_quote()
            .returning(|_, _, _, _| {
                Box::pin(ready(Ok(
                    crate::services::stellar_dex::StellarQuoteResponse {
                        input_asset: "native".to_string(),
                        output_asset: "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC"
                            .to_string(),
                        in_amount: 50100,    // fee in stroops
                        out_amount: 1500000, // fee in token
                        price_impact_pct: 0.0,
                        slippage_bps: 100,
                        path: None,
                    },
                )))
            });

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let transaction_xdr = create_test_soroban_transaction_xdr();
        let request = SponsoredTransactionQuoteRequest::Stellar(
            crate::models::StellarFeeEstimateRequestParams {
                transaction_xdr: Some(transaction_xdr),
                operations: None,
                source_account: None,
                fee_token: "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC".to_string(),
            },
        );

        let result = relayer.quote_sponsored_transaction(request).await;
        if let Err(e) = &result {
            eprintln!("Soroban quote error: {:?}", e);
        }
        assert!(result.is_ok());

        if let SponsoredTransactionQuoteResponse::Stellar(quote) = result.unwrap() {
            assert_eq!(quote.fee_in_token, "1500000");
            assert!(!quote.fee_in_token_ui.is_empty());
            assert!(!quote.conversion_rate.is_empty());
        } else {
            panic!("Expected Stellar quote response");
        }

        // Clean up env var
        std::env::remove_var("STELLAR_FEE_FORWARDER_ADDRESS");
    }

    #[tokio::test]
    #[serial]
    async fn test_quote_soroban_from_xdr_missing_fee_forwarder() {
        // Ensure env var is NOT set
        std::env::remove_var("STELLAR_FEE_FORWARDER_ADDRESS");

        let relayer_model = create_test_relayer_with_soroban_token();
        let provider = MockStellarProviderTrait::new();

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service.expect_supported_asset_types().returning(|| {
            std::collections::HashSet::from([AssetType::Native, AssetType::Contract])
        });

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let transaction_xdr = create_test_soroban_transaction_xdr();
        let request = SponsoredTransactionQuoteRequest::Stellar(
            crate::models::StellarFeeEstimateRequestParams {
                transaction_xdr: Some(transaction_xdr),
                operations: None,
                source_account: None,
                fee_token: "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC".to_string(),
            },
        );

        let result = relayer.quote_sponsored_transaction(request).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, RelayerError::ValidationError(_)));
        if let RelayerError::ValidationError(msg) = err {
            assert!(msg.contains("STELLAR_FEE_FORWARDER_ADDRESS"));
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_quote_soroban_from_xdr_invalid_fee_token_format() {
        // Set required env var for FeeForwarder
        std::env::set_var(
            "STELLAR_FEE_FORWARDER_ADDRESS",
            "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
        );

        // Create relayer that allows both classic and Soroban tokens
        let mut policy = RelayerStellarPolicy::default();
        policy.fee_payment_strategy = Some(crate::models::StellarFeePaymentStrategy::User);
        policy.allowed_tokens = Some(vec![
            crate::models::StellarAllowedTokensPolicy {
                asset: USDC_ASSET.to_string(), // Classic asset
                metadata: None,
                max_allowed_fee: None,
                swap_config: None,
            },
            crate::models::StellarAllowedTokensPolicy {
                asset: "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC".to_string(),
                metadata: None,
                max_allowed_fee: None,
                swap_config: None,
            },
        ]);

        let relayer_model = RelayerRepoModel {
            id: "test-relayer-id".to_string(),
            name: "Test Relayer".to_string(),
            network: "testnet".to_string(),
            paused: false,
            network_type: NetworkType::Stellar,
            signer_id: "signer-id".to_string(),
            policies: RelayerNetworkPolicy::Stellar(policy),
            address: TEST_PK.to_string(),
            notification_id: Some("notification-id".to_string()),
            system_disabled: false,
            custom_rpc_urls: None,
            ..Default::default()
        };

        let provider = MockStellarProviderTrait::new();

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service
            .expect_supported_asset_types()
            .returning(|| std::collections::HashSet::from([AssetType::Native, AssetType::Classic]));

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        // Use Soroban XDR but with classic asset as fee_token (invalid for Soroban path)
        let transaction_xdr = create_test_soroban_transaction_xdr();
        let request = SponsoredTransactionQuoteRequest::Stellar(
            crate::models::StellarFeeEstimateRequestParams {
                transaction_xdr: Some(transaction_xdr),
                operations: None,
                source_account: None,
                fee_token: USDC_ASSET.to_string(), // Classic asset, not valid C... format
            },
        );

        let result = relayer.quote_sponsored_transaction(request).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, RelayerError::ValidationError(_)));
        if let RelayerError::ValidationError(msg) = err {
            assert!(msg.contains("Soroban contract address"));
        }

        // Clean up env var
        std::env::remove_var("STELLAR_FEE_FORWARDER_ADDRESS");
    }

    // ============================================================================
    // Tests for build_soroban_sponsored (via build_sponsored_transaction)
    // ============================================================================

    #[tokio::test]
    #[serial]
    async fn test_build_soroban_sponsored_success() {
        // Set required env var for FeeForwarder
        std::env::set_var(
            "STELLAR_FEE_FORWARDER_ADDRESS",
            "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
        );

        let relayer_model = create_test_relayer_with_soroban_token();
        let mut provider = MockStellarProviderTrait::new();

        // Mock get_latest_ledger for expiration calculation (called twice - for simulation and for valid_until)
        provider.expect_get_latest_ledger().returning(|| {
            Box::pin(ready(Ok(
                soroban_rs::stellar_rpc_client::GetLatestLedgerResponse {
                    id: "test".to_string(),
                    protocol_version: 20,
                    sequence: 1000,
                },
            )))
        });

        // Mock simulate_transaction_envelope for Soroban fee estimation
        let valid_tx_data = create_valid_soroban_transaction_data_xdr();
        provider
            .expect_simulate_transaction_envelope()
            .returning(move |_| {
                let tx_data = valid_tx_data.clone();
                Box::pin(ready(Ok(
                    soroban_rs::stellar_rpc_client::SimulateTransactionResponse {
                        min_resource_fee: 50000,
                        transaction_data: tx_data,
                        ..Default::default()
                    },
                )))
            });

        // Mock call_contract for Soroban token balance check
        provider.expect_call_contract().returning(|_, _, _| {
            use soroban_rs::xdr::Int128Parts;
            // Return a balance of 10_000_000 (sufficient for fee)
            Box::pin(ready(Ok(ScVal::I128(Int128Parts {
                hi: 0,
                lo: 10_000_000,
            }))))
        });

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service.expect_supported_asset_types().returning(|| {
            std::collections::HashSet::from([AssetType::Native, AssetType::Contract])
        });

        // Mock get_xlm_to_token_quote for fee conversion
        dex_service
            .expect_get_xlm_to_token_quote()
            .returning(|_, _, _, _| {
                Box::pin(ready(Ok(
                    crate::services::stellar_dex::StellarQuoteResponse {
                        input_asset: "native".to_string(),
                        output_asset: "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC"
                            .to_string(),
                        in_amount: 50100,
                        out_amount: 1500000,
                        price_impact_pct: 0.0,
                        slippage_bps: 100,
                        path: None,
                    },
                )))
            });

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let transaction_xdr = create_test_soroban_transaction_xdr();
        let request = SponsoredTransactionBuildRequest::Stellar(
            crate::models::StellarPrepareTransactionRequestParams {
                transaction_xdr: Some(transaction_xdr),
                operations: None,
                source_account: None,
                fee_token: "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC".to_string(),
            },
        );

        let result = relayer.build_sponsored_transaction(request).await;
        if let Err(e) = &result {
            eprintln!("Soroban build error: {:?}", e);
        }
        assert!(result.is_ok());

        if let SponsoredTransactionBuildResponse::Stellar(build) = result.unwrap() {
            assert!(!build.transaction.is_empty());
            assert_eq!(build.fee_in_token, "1500000");
            assert!(!build.fee_in_token_ui.is_empty());
            assert_eq!(
                build.fee_token,
                "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC"
            );
            assert!(!build.valid_until.is_empty());
            // Soroban transactions should have user_auth_entry
            assert!(build.user_auth_entry.is_some());
            assert!(!build.user_auth_entry.unwrap().is_empty());
        } else {
            panic!("Expected Stellar build response");
        }

        // Clean up env var
        std::env::remove_var("STELLAR_FEE_FORWARDER_ADDRESS");
    }

    #[tokio::test]
    #[serial]
    async fn test_build_soroban_sponsored_missing_fee_forwarder() {
        // Ensure env var is NOT set
        std::env::remove_var("STELLAR_FEE_FORWARDER_ADDRESS");

        let relayer_model = create_test_relayer_with_soroban_token();
        let provider = MockStellarProviderTrait::new();

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service.expect_supported_asset_types().returning(|| {
            std::collections::HashSet::from([AssetType::Native, AssetType::Contract])
        });

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let transaction_xdr = create_test_soroban_transaction_xdr();
        let request = SponsoredTransactionBuildRequest::Stellar(
            crate::models::StellarPrepareTransactionRequestParams {
                transaction_xdr: Some(transaction_xdr),
                operations: None,
                source_account: None,
                fee_token: "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC".to_string(),
            },
        );

        let result = relayer.build_sponsored_transaction(request).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, RelayerError::ValidationError(_)));
        if let RelayerError::ValidationError(msg) = err {
            assert!(msg.contains("STELLAR_FEE_FORWARDER_ADDRESS"));
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_build_soroban_sponsored_insufficient_balance() {
        // Set required env var for FeeForwarder
        std::env::set_var(
            "STELLAR_FEE_FORWARDER_ADDRESS",
            "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
        );

        let relayer_model = create_test_relayer_with_soroban_token();
        let mut provider = MockStellarProviderTrait::new();

        // Mock get_latest_ledger
        provider.expect_get_latest_ledger().returning(|| {
            Box::pin(ready(Ok(
                soroban_rs::stellar_rpc_client::GetLatestLedgerResponse {
                    id: "test".to_string(),
                    protocol_version: 20,
                    sequence: 1000,
                },
            )))
        });

        // Mock simulate_transaction_envelope
        provider
            .expect_simulate_transaction_envelope()
            .returning(|_| {
                Box::pin(ready(Ok(
                    soroban_rs::stellar_rpc_client::SimulateTransactionResponse {
                        min_resource_fee: 50000,
                        transaction_data: "AAAAAQAAAAAAAAACAAAAAAAAAAAAAAAAAAAABgAAAAEAAAAGAAAAAG0JZTO9fU6p3NeJp5w3TpKhZmx6p1pR7mq9wFwCnEIuAAAAFAAAAAEAAAAAAAAAB8NVb2IAAAH0AAAAAQAAAAAAABfAAAAAAAAAAPUAAAAAAAAENgAAAAA=".to_string(),
                        ..Default::default()
                    },
                )))
            });

        // Mock call_contract with INSUFFICIENT balance
        provider.expect_call_contract().returning(|_, _, _| {
            use soroban_rs::xdr::Int128Parts;
            // Return a very low balance (100, much less than required 1500000)
            Box::pin(ready(Ok(ScVal::I128(Int128Parts { hi: 0, lo: 100 }))))
        });

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service.expect_supported_asset_types().returning(|| {
            std::collections::HashSet::from([AssetType::Native, AssetType::Contract])
        });

        // Mock get_xlm_to_token_quote
        dex_service
            .expect_get_xlm_to_token_quote()
            .returning(|_, _, _, _| {
                Box::pin(ready(Ok(
                    crate::services::stellar_dex::StellarQuoteResponse {
                        input_asset: "native".to_string(),
                        output_asset: "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC"
                            .to_string(),
                        in_amount: 50100,
                        out_amount: 1500000, // Fee required
                        price_impact_pct: 0.0,
                        slippage_bps: 100,
                        path: None,
                    },
                )))
            });

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let transaction_xdr = create_test_soroban_transaction_xdr();
        let request = SponsoredTransactionBuildRequest::Stellar(
            crate::models::StellarPrepareTransactionRequestParams {
                transaction_xdr: Some(transaction_xdr),
                operations: None,
                source_account: None,
                fee_token: "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC".to_string(),
            },
        );

        let result = relayer.build_sponsored_transaction(request).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, RelayerError::ValidationError(_)));
        if let RelayerError::ValidationError(msg) = err {
            assert!(msg.contains("Insufficient balance"));
        }

        // Clean up env var
        std::env::remove_var("STELLAR_FEE_FORWARDER_ADDRESS");
    }

    #[tokio::test]
    #[serial]
    async fn test_build_soroban_sponsored_simulation_error() {
        // Set required env var for FeeForwarder
        std::env::set_var(
            "STELLAR_FEE_FORWARDER_ADDRESS",
            "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
        );

        let relayer_model = create_test_relayer_with_soroban_token();
        let mut provider = MockStellarProviderTrait::new();

        // Mock get_latest_ledger
        provider.expect_get_latest_ledger().returning(|| {
            Box::pin(ready(Ok(
                soroban_rs::stellar_rpc_client::GetLatestLedgerResponse {
                    id: "test".to_string(),
                    protocol_version: 20,
                    sequence: 1000,
                },
            )))
        });

        // Mock simulate_transaction_envelope to return error
        provider
            .expect_simulate_transaction_envelope()
            .returning(|_| {
                Box::pin(ready(Ok(
                    soroban_rs::stellar_rpc_client::SimulateTransactionResponse {
                        error: Some(
                            "Contract execution failed: insufficient resources".to_string(),
                        ),
                        min_resource_fee: 0,
                        transaction_data: "".to_string(),
                        ..Default::default()
                    },
                )))
            });

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service.expect_supported_asset_types().returning(|| {
            std::collections::HashSet::from([AssetType::Native, AssetType::Contract])
        });

        // Mock get_xlm_to_token_quote for initial fee estimation
        dex_service
            .expect_get_xlm_to_token_quote()
            .returning(|_, _, _, _| {
                Box::pin(ready(Ok(
                    crate::services::stellar_dex::StellarQuoteResponse {
                        input_asset: "native".to_string(),
                        output_asset: "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC"
                            .to_string(),
                        in_amount: 100,
                        out_amount: 1500,
                        price_impact_pct: 0.0,
                        slippage_bps: 100,
                        path: None,
                    },
                )))
            });

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let transaction_xdr = create_test_soroban_transaction_xdr();
        let request = SponsoredTransactionBuildRequest::Stellar(
            crate::models::StellarPrepareTransactionRequestParams {
                transaction_xdr: Some(transaction_xdr),
                operations: None,
                source_account: None,
                fee_token: "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC".to_string(),
            },
        );

        let result = relayer.build_sponsored_transaction(request).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        // Simulation errors are wrapped in ValidationError via calculate_total_soroban_fee
        assert!(matches!(err, RelayerError::ValidationError(_)));
        if let RelayerError::ValidationError(msg) = err {
            assert!(msg.contains("Simulation failed"));
        }

        // Clean up env var
        std::env::remove_var("STELLAR_FEE_FORWARDER_ADDRESS");
    }
}
