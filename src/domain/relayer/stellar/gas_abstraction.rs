//! Gas abstraction implementation for Stellar relayers.
//!
//! This module implements the `GasAbstractionTrait` for Stellar relayers, providing
//! gas abstraction functionality including fee estimation and transaction preparation.

use async_trait::async_trait;
use chrono::Utc;
use soroban_rs::xdr::{Limits, Operation, TransactionEnvelope, WriteXdr};
use tracing::debug;

use crate::constants::{
    get_stellar_sponsored_transaction_validity_duration, STELLAR_DEFAULT_TRANSACTION_FEE,
};
use crate::domain::relayer::{
    stellar::xdr_utils::parse_transaction_xdr, GasAbstractionTrait, RelayerError, StellarRelayer,
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
use crate::services::TransactionCounterServiceTrait;

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
        debug!(
            "Processing quote sponsored transaction request for token: {}",
            params.fee_token
        );

        // Validate allowed token
        let policy = self.relayer.policies.get_stellar_policy();
        StellarTransactionValidator::validate_allowed_token(&params.fee_token, &policy).map_err(
            |e| RelayerError::Internal(format!("Failed to validate allowed token: {}", e)),
        )?;

        // Validate that either transaction_xdr or operations is provided
        if params.transaction_xdr.is_none() && params.operations.is_none() {
            return Err(RelayerError::ValidationError(
                "Must provide either transaction_xdr or operations in the request".to_string(),
            ));
        }

        // Build envelope from XDR or operations (reusing logic from build method)
        let envelope = build_envelope_from_request(
            params.transaction_xdr.as_ref(),
            params.operations.as_ref(),
            params.source_account.as_ref(),
            &self.network.passphrase,
        )?;

        // Run comprehensive security validation (similar to build method)
        StellarTransactionValidator::gasless_transaction_validation(
            &envelope,
            &self.relayer.address,
            &policy,
            &self.provider,
            None, // Duration validation not needed for quote
        )
        .await
        .map_err(|e| {
            RelayerError::ValidationError(format!("Failed to validate gasless transaction: {}", e))
        })?;

        // Estimate fee using estimate_fee utility which handles simulation if needed
        let inner_tx_fee = estimate_fee(&envelope, &self.provider, None)
            .await
            .map_err(crate::models::RelayerError::from)?;

        // Add fees for fee payment operation (100 stroops) and fee-bump transaction (100 stroops)
        // For Soroban transactions, the simulation already accounts for resource fees,
        // we just need to add the inclusion fees for the additional operations
        let is_soroban = xdr_needs_simulation(&envelope).unwrap_or(false);
        let mut additional_fees = 0;
        if !is_soroban {
            additional_fees = 2 * STELLAR_DEFAULT_TRANSACTION_FEE as u64; // 200 stroops total
        }
        let xlm_fee = inner_tx_fee + additional_fees;

        // Convert to token amount via DEX service
        let fee_quote = convert_xlm_fee_to_token(
            self.dex_service.as_ref(),
            &policy,
            xlm_fee,
            &params.fee_token,
            policy.fee_margin_percentage,
        )
        .await
        .map_err(crate::models::RelayerError::from)?;

        // Validate max fee
        StellarTransactionValidator::validate_max_fee(fee_quote.fee_in_stroops, &policy)
            .map_err(|e| RelayerError::Internal(format!("Failed to validate max fee: {}", e)))?;

        // Validate token-specific max fee
        StellarTransactionValidator::validate_token_max_fee(
            &params.fee_token,
            fee_quote.fee_in_token,
            &policy,
        )
        .map_err(|e| {
            RelayerError::Internal(format!("Failed to validate token-specific max fee: {}", e))
        })?;

        // Check user token balance to ensure they have enough to pay the fee
        StellarTransactionValidator::validate_user_token_balance(
            &envelope,
            &params.fee_token,
            fee_quote.fee_in_token,
            &self.provider,
        )
        .await
        .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        debug!("Fee estimate result: {:?}", fee_quote);

        let result = StellarFeeEstimateResult {
            estimated_fee: fee_quote.fee_in_token_ui,
            conversion_rate: fee_quote.conversion_rate.to_string(),
        };
        Ok(SponsoredTransactionQuoteResponse::Stellar(result))
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
        debug!(
            "Processing prepare transaction request for token: {}",
            params.fee_token
        );

        // Validate allowed token
        let policy = self.relayer.policies.get_stellar_policy();
        StellarTransactionValidator::validate_allowed_token(&params.fee_token, &policy).map_err(
            |e| RelayerError::Internal(format!("Failed to validate allowed token: {}", e)),
        )?;

        // Validate that either transaction_xdr or operations is provided
        if params.transaction_xdr.is_none() && params.operations.is_none() {
            return Err(RelayerError::ValidationError(
                "Must provide either transaction_xdr or operations in the request".to_string(),
            ));
        }

        // Build envelope from XDR or operations (reusing shared helper)
        let envelope = build_envelope_from_request(
            params.transaction_xdr.as_ref(),
            params.operations.as_ref(),
            params.source_account.as_ref(),
            &self.network.passphrase,
        )?;

        StellarTransactionValidator::gasless_transaction_validation(
            &envelope,
            &self.relayer.address,
            &policy,
            &self.provider,
            None, // Duration validation not needed here as time bounds are set during build
        )
        .await
        .map_err(|e| {
            RelayerError::ValidationError(format!("Failed to validate gasless transaction: {}", e))
        })?;

        // Get fee estimate using estimate_fee utility which handles simulation if needed
        // For non-Soroban transactions, we'll add 200 stroops (100 for fee payment op + 100 for fee-bump)
        let inner_tx_fee = estimate_fee(&envelope, &self.provider, None)
            .await
            .map_err(crate::models::RelayerError::from)?;

        // Add fees for fee payment operation (100 stroops) and fee-bump transaction (100 stroops)
        // For Soroban transactions, the simulation already accounts for resource fees,
        // we just need to add the inclusion fees for the additional operations
        let is_soroban = xdr_needs_simulation(&envelope).unwrap_or(false);
        let mut additional_fees = 0;
        if !is_soroban {
            additional_fees = 2 * STELLAR_DEFAULT_TRANSACTION_FEE as u64; // 200 stroops total
        }
        let xlm_fee = inner_tx_fee + additional_fees;

        debug!(
            inner_tx_fee = inner_tx_fee,
            additional_fees = additional_fees,
            total_fee = xlm_fee,
            "Fee estimated: inner transaction + fee payment op + fee-bump transaction fee"
        );

        // Calculate fee quote first to check user balance before modifying envelope
        let preliminary_fee_quote = convert_xlm_fee_to_token(
            self.dex_service.as_ref(),
            &policy,
            xlm_fee,
            &params.fee_token,
            policy.fee_margin_percentage,
        )
        .await
        .map_err(crate::models::RelayerError::from)?;

        // Validate max fee
        StellarTransactionValidator::validate_max_fee(
            preliminary_fee_quote.fee_in_stroops,
            &policy,
        )
        .map_err(|e| RelayerError::Internal(format!("Failed to validate max fee: {}", e)))?;

        // Validate token-specific max fee
        StellarTransactionValidator::validate_token_max_fee(
            &params.fee_token,
            preliminary_fee_quote.fee_in_token,
            &policy,
        )
        .map_err(|e| {
            RelayerError::Internal(format!("Failed to validate token-specific max fee: {}", e))
        })?;

        // Check user token balance to ensure they have enough to pay the fee
        StellarTransactionValidator::validate_user_token_balance(
            &envelope,
            &params.fee_token,
            preliminary_fee_quote.fee_in_token,
            &self.provider,
        )
        .await
        .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Estimate fee, convert to token, and add payment operation
        let (fee_quote, mut final_envelope) = estimate_fee_and_add_payment_operation(
            self,
            envelope,
            xlm_fee,
            &params.fee_token,
            policy.fee_margin_percentage,
            &self.relayer.address,
        )
        .await?;

        debug!(
            estimated_fee = xlm_fee,
            final_fee_in_token = fee_quote.fee_in_token_ui,
            "Transaction prepared successfully"
        );

        // Set final time bounds just before returning to give user maximum time to review and submit
        let valid_until = Utc::now() + get_stellar_sponsored_transaction_validity_duration();
        set_time_bounds(&mut final_envelope, valid_until)
            .map_err(crate::models::RelayerError::from)?;

        // Serialize final transaction
        let extended_xdr = final_envelope
            .to_xdr_base64(Limits::none())
            .map_err(|e| RelayerError::Internal(format!("Failed to serialize XDR: {}", e)))?;

        Ok(SponsoredTransactionBuildResponse::Stellar(
            StellarPrepareTransactionResult {
                transaction: extended_xdr,
                fee_in_token: fee_quote.fee_in_token_ui,
                fee_in_stroops: fee_quote.fee_in_stroops.to_string(),
                fee_token: params.fee_token,
                valid_until: valid_until.to_rfc3339(),
            },
        ))
    }
}

/// Build a transaction envelope from either XDR or operations
///
/// This helper function is used by both quote and build methods to construct
/// a transaction envelope from either a pre-built XDR transaction or from
/// operations with a source account.
fn build_envelope_from_request(
    transaction_xdr: Option<&String>,
    operations: Option<&Vec<OperationSpec>>,
    source_account: Option<&String>,
    network_passphrase: &str,
) -> Result<TransactionEnvelope, RelayerError> {
    if let Some(xdr) = transaction_xdr {
        parse_transaction_xdr(xdr, false)
            .map_err(|e| RelayerError::Internal(format!("Failed to parse XDR: {}", e)))
    } else if let Some(ops) = operations {
        // Build envelope from operations
        let source_account = source_account.ok_or_else(|| {
            RelayerError::ValidationError(
                "source_account is required when providing operations".to_string(),
            )
        })?;

        // Create StellarTransactionData from operations
        let stellar_data = StellarTransactionData {
            source_account: source_account.clone(),
            fee: None,
            sequence_number: None,
            memo: None,
            valid_until: None,
            network_passphrase: network_passphrase.to_string(),
            signatures: vec![],
            hash: None,
            simulation_transaction_data: None,
            transaction_input: TransactionInput::Operations(ops.clone()),
            signed_envelope_xdr: None,
        };

        // Build unsigned envelope from operations
        stellar_data.build_unsigned_envelope().map_err(|e| {
            RelayerError::Internal(format!("Failed to build envelope from operations: {}", e))
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
    let payment_op = Operation::try_from(payment_op_spec).map_err(|e| {
        RelayerError::Internal(format!("Failed to convert payment operation: {}", e))
    })?;

    // Add payment operation to transaction
    add_operation_to_envelope(envelope, payment_op).map_err(crate::models::RelayerError::from)?;

    Ok(())
}

/// Estimate fee, convert to token amount, and add payment operation to envelope
///
/// This utility function combines fee estimation, conversion, and payment operation
/// creation into a single operation. It:
/// 1. Estimates and converts XLM fee to token amount
/// 2. Adds fee payment operation to the envelope
///
/// Note: Time bounds should be set separately just before returning the transaction
/// to give the user maximum time to review and submit.
///
/// Returns the fee quote, buffered XLM fee, and the updated envelope.
async fn estimate_fee_and_add_payment_operation<P, RR, NR, TR, J, TCS, S, D>(
    relayer: &StellarRelayer<P, RR, NR, TR, J, TCS, S, D>,
    mut envelope: TransactionEnvelope,
    xlm_fee: u64,
    fee_token: &str,
    fee_margin_percentage: Option<f32>,
    relayer_address: &str,
) -> Result<(FeeQuote, TransactionEnvelope), RelayerError>
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
    // Convert XLM fee to token amount
    let policy = relayer.relayer.policies.get_stellar_policy();
    let fee_quote = convert_xlm_fee_to_token(
        relayer.dex_service.as_ref(),
        &policy,
        xlm_fee,
        fee_token,
        fee_margin_percentage,
    )
    .await
    .map_err(crate::models::RelayerError::from)?;

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

    Ok((fee_quote, envelope))
}
