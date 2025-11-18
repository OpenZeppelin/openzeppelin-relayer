//! Gas abstraction implementation for Stellar relayers.
//!
//! This module implements the `GasAbstractionTrait` for Stellar relayers, providing
//! gas abstraction functionality including fee estimation and transaction preparation.

use async_trait::async_trait;
use chrono::{Duration, Utc};
use soroban_rs::xdr::{Limits, Operation, ReadXdr, TransactionEnvelope, WriteXdr};
use tracing::debug;

use crate::domain::relayer::{
    stellar::xdr_utils::parse_transaction_xdr, xdr_utils::extract_operations, GasAbstractionTrait,
    RelayerError, StellarRelayer,
};
use crate::domain::transaction::stellar::{
    utils::{
        add_operation_to_envelope, convert_xlm_fee_to_token, count_operations_from_xdr,
        create_fee_payment_operation, estimate_base_fee, estimate_fee, set_time_bounds, FeeQuote,
    },
    StellarTransactionValidator,
};
use crate::jobs::JobProducerTrait;
use crate::models::{
    GaslessTransactionBuildRequest, GaslessTransactionBuildResponse,
    GaslessTransactionQuoteRequest, GaslessTransactionQuoteResponse, StellarFeeEstimateResult,
    StellarPrepareTransactionResult, StellarTransactionData, TransactionInput,
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
    async fn get_gasless_transaction_quote(
        &self,
        params: GaslessTransactionQuoteRequest,
    ) -> Result<GaslessTransactionQuoteResponse, RelayerError> {
        let params = match params {
            GaslessTransactionQuoteRequest::Stellar(p) => p,
            _ => {
                return Err(RelayerError::ValidationError(
                    "Expected Stellar fee estimate request parameters".to_string(),
                ));
            }
        };
        debug!(
            "Processing fee estimate request for token: {}",
            params.fee_token
        );

        // Validate allowed token
        let policy = self.relayer.policies.get_stellar_policy();
        StellarTransactionValidator::validate_allowed_token(&params.fee_token, &policy).map_err(
            |e| RelayerError::Internal(format!("Failed to validate allowed token: {}", e)),
        )?;

        // Estimate fee from envelope or operations
        let xlm_fee: u64 = if let Some(ref xdr) = params.transaction_xdr {
            // Parse XDR and use estimate_fee utility which handles simulation if needed
            let envelope = TransactionEnvelope::from_xdr_base64(xdr, Limits::none())
                .map_err(|e| RelayerError::Internal(format!("Failed to parse XDR: {}", e)))?;

            // Count operations for override (+1 for fee payment operation)
            let operations = extract_operations(&envelope).map_err(|e| {
                RelayerError::Internal(format!("Failed to extract operations: {}", e))
            })?;
            let num_operations = operations.len();

            estimate_fee(
                &envelope,
                &self.provider,
                Some(num_operations + 1), // +1 for fee payment operation
            )
            .await
            .map_err(|e| RelayerError::Internal(format!("Failed to estimate fee: {}", e)))?
        } else if let Some(ref operations) = params.operations {
            // For operations-only requests, use base fee estimation
            // (estimate_fee requires an envelope, so we fall back to estimate_base_fee)
            let num_operations = operations.len();
            estimate_base_fee(num_operations + 1) // +1 for fee payment operation
        } else {
            return Err(RelayerError::ValidationError(
                "Must provide either transaction_xdr or operations in the request".to_string(),
            ));
        };

        // Convert to token amount via DEX service
        let (fee_quote, _) = convert_xlm_fee_to_token(
            self.dex_service.as_ref(),
            &policy,
            xlm_fee,
            &params.fee_token,
            policy.fee_margin_percentage,
        )
        .await
        .map_err(|e| {
            RelayerError::Internal(format!("Failed to estimate and convert fee: {}", e))
        })?;

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

        debug!("Fee estimate result: {:?}", fee_quote);

        let result = StellarFeeEstimateResult {
            estimated_fee: fee_quote.fee_in_token_ui,
            conversion_rate: fee_quote.conversion_rate.to_string(),
        };
        Ok(GaslessTransactionQuoteResponse::Stellar(result))
    }

    async fn build_gasless_transaction(
        &self,
        params: GaslessTransactionBuildRequest,
    ) -> Result<GaslessTransactionBuildResponse, RelayerError> {
        let params = match params {
            GaslessTransactionBuildRequest::Stellar(p) => p,
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

        // Build envelope from XDR or operations
        let (envelope, num_operations) = if let Some(ref xdr) = params.transaction_xdr {
            let envelope = parse_transaction_xdr(xdr, false)
                .map_err(|e| RelayerError::Internal(format!("Failed to parse XDR: {}", e)))?;
            let num_operations = count_operations_from_xdr(xdr)?;
            (envelope, num_operations)
        } else if let Some(ref operations) = params.operations {
            // Build envelope from operations
            let source_account = params.source_account.as_ref().ok_or_else(|| {
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
                network_passphrase: self.network.passphrase.clone(),
                signatures: vec![],
                hash: None,
                simulation_transaction_data: None,
                transaction_input: TransactionInput::Operations(operations.clone()),
                signed_envelope_xdr: None,
            };

            // Build unsigned envelope from operations
            let envelope = stellar_data.build_unsigned_envelope().map_err(|e| {
                RelayerError::Internal(format!("Failed to build envelope from operations: {}", e))
            })?;

            let num_operations = operations.len();
            (envelope, num_operations)
        } else {
            unreachable!("Validation above ensures one is set");
        };

        StellarTransactionValidator::gasless_transaction_validation(
            &envelope,
            &self.relayer.address,
            &policy,
            &self.provider,
        )
        .await
        .map_err(|e| {
            RelayerError::ValidationError(format!("Failed to validate gasless transaction: {}", e))
        })?;

        // Get fee estimate using estimate_fee utility which handles simulation if needed
        // Override operations count to include fee payment operation (+1)
        let xlm_fee = estimate_fee(
            &envelope,
            &self.provider,
            Some(num_operations + 1), // +1 for fee payment operation
        )
        .await
        .map_err(|e| RelayerError::Internal(format!("Failed to estimate fee: {}", e)))?;

        debug!(
            operations_count = num_operations,
            estimated_fee = xlm_fee,
            "Fee estimated using estimate_fee utility (simulation handled automatically if needed)"
        );

        // Estimate fee, convert to token, and add payment operation
        let (fee_quote, buffered_xlm_fee, mut final_envelope) =
            estimate_fee_and_add_payment_operation(
                self,
                envelope,
                xlm_fee,
                &params.fee_token,
                policy.fee_margin_percentage,
                &self.relayer.address,
            )
            .await?;

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

        debug!(
            operations_count = num_operations,
            estimated_fee = xlm_fee,
            final_fee_in_token = fee_quote.fee_in_token_ui,
            "Transaction prepared successfully"
        );

        // Set final time bounds just before returning to give user maximum time to review and submit
        // Using 1 minute to provide reasonable time while ensuring transaction doesn't expire too quickly
        let valid_until = Utc::now() + Duration::minutes(1);
        set_time_bounds(&mut final_envelope, valid_until).map_err(|e| {
            RelayerError::Internal(format!("Failed to set final time bounds: {}", e))
        })?;

        // Serialize final transaction
        let extended_xdr = final_envelope
            .to_xdr_base64(Limits::none())
            .map_err(|e| RelayerError::Internal(format!("Failed to serialize XDR: {}", e)))?;

        Ok(GaslessTransactionBuildResponse::Stellar(
            StellarPrepareTransactionResult {
                transaction: extended_xdr,
                fee_in_token: fee_quote.fee_in_token_ui,
                fee_in_stroops: buffered_xlm_fee.to_string(),
                fee_token: params.fee_token,
                valid_until: valid_until.to_rfc3339(),
            },
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
        .map_err(|e| {
            RelayerError::Internal(format!("Failed to create fee payment operation: {}", e))
        })?;

    // Convert OperationSpec to XDR Operation
    let payment_op = Operation::try_from(payment_op_spec).map_err(|e| {
        RelayerError::Internal(format!("Failed to convert payment operation: {}", e))
    })?;

    // Add payment operation to transaction
    add_operation_to_envelope(envelope, payment_op)
        .map_err(|e| RelayerError::Internal(format!("Failed to add operation: {}", e)))?;

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
) -> Result<(FeeQuote, u64, TransactionEnvelope), RelayerError>
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
    let (fee_quote, buffered_xlm_fee) = convert_xlm_fee_to_token(
        relayer.dex_service.as_ref(),
        &policy,
        xlm_fee,
        fee_token,
        fee_margin_percentage,
    )
    .await
    .map_err(|e| RelayerError::Internal(format!("Failed to estimate and convert fee: {}", e)))?;

    // Convert fee amount to i64 for payment operation
    let fee_amount = i64::try_from(fee_quote.fee_in_token).map_err(|_| {
        RelayerError::Internal(
            "Fee amount too large for payment operation (exceeds i64::MAX)".to_string(),
        )
    })?;

    // Add fee payment operation to envelope
    add_fee_payment_operation(&mut envelope, fee_token, fee_amount, relayer_address)?;

    Ok((fee_quote, buffered_xlm_fee, envelope))
}
