//! This module contains the status handling functionality for Stellar transactions.
//! It includes methods for checking transaction status with robust error handling,
//! ensuring proper transaction state management and lane cleanup.

use chrono::{DateTime, Utc};
use soroban_rs::xdr::{
    Error, Hash, InnerTransactionResultResult, InvokeHostFunctionResult, Limits, OperationResult,
    OperationResultTr, TransactionEnvelope, TransactionResultResult, WriteXdr,
};
use tracing::{debug, info, warn};

use super::{is_final_state, StellarRelayerTransaction};
use crate::constants::{get_stellar_max_stuck_transaction_lifetime, get_stellar_resend_timeout};
use crate::domain::transaction::stellar::prepare::common::send_submit_transaction_job;
use crate::domain::transaction::stellar::utils::extract_return_value_from_meta;
use crate::domain::transaction::stellar::utils::extract_time_bounds;
use crate::domain::transaction::util::{get_age_since_created, get_age_since_sent_or_created};
use crate::domain::xdr_utils::parse_transaction_xdr;
use crate::{
    constants::STELLAR_PENDING_RECOVERY_TRIGGER_SECONDS,
    jobs::{JobProducerTrait, StatusCheckContext, TransactionRequest},
    models::{
        NetworkTransactionData, RelayerRepoModel, TransactionError, TransactionRepoModel,
        TransactionStatus, TransactionUpdateRequest,
    },
    repositories::{Repository, TransactionCounterTrait, TransactionRepository},
    services::{
        provider::StellarProviderTrait,
        signer::{Signer, StellarSignTrait},
    },
};

impl<R, T, J, S, P, C, D> StellarRelayerTransaction<R, T, J, S, P, C, D>
where
    R: Repository<RelayerRepoModel, String> + Send + Sync,
    T: TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    S: Signer + StellarSignTrait + Send + Sync,
    P: StellarProviderTrait + Send + Sync,
    C: TransactionCounterTrait + Send + Sync,
    D: crate::services::stellar_dex::StellarDexServiceTrait + Send + Sync + 'static,
{
    /// Main status handling method with robust error handling.
    /// This method checks transaction status and handles lane cleanup for finalized transactions.
    ///
    /// # Arguments
    ///
    /// * `tx` - The transaction to check status for
    /// * `context` - Optional circuit breaker context with failure tracking information
    pub async fn handle_transaction_status_impl(
        &self,
        tx: TransactionRepoModel,
        context: Option<StatusCheckContext>,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(
            tx_id = %tx.id,
            relayer_id = %tx.relayer_id,
            status = ?tx.status,
            "handling transaction status"
        );

        // Early exit for final states - no need to check
        if is_final_state(&tx.status) {
            debug!(
                tx_id = %tx.id,
                relayer_id = %tx.relayer_id,
                status = ?tx.status,
                "transaction in final state, skipping status check"
            );
            return Ok(tx);
        }

        // Check if circuit breaker should force finalization
        if let Some(ref ctx) = context {
            if ctx.should_force_finalize() {
                let reason = format!(
                    "Transaction status monitoring failed after {} consecutive errors (total: {}). \
                     Last status: {:?}. Unable to determine final on-chain state.",
                    ctx.consecutive_failures, ctx.total_failures, tx.status
                );
                warn!(
                    tx_id = %tx.id,
                    consecutive_failures = ctx.consecutive_failures,
                    total_failures = ctx.total_failures,
                    max_consecutive = ctx.max_consecutive_failures,
                    "circuit breaker triggered, forcing transaction to failed state"
                );
                // Note: Expiry checks are already performed in the normal flow for Pending/Sent
                // states (before any RPC calls). If we've hit consecutive failures, it's a strong
                // signal that status monitoring is fundamentally broken for this transaction.
                return self.mark_as_failed(tx, reason).await;
            }
        }

        match self.status_core(tx.clone()).await {
            Ok(updated_tx) => {
                debug!(
                    tx_id = %updated_tx.id,
                    status = ?updated_tx.status,
                    "status check completed successfully"
                );
                Ok(updated_tx)
            }
            Err(error) => {
                debug!(
                    tx_id = %tx.id,
                    error = ?error,
                    "status check encountered error"
                );

                // Handle different error types appropriately
                match error {
                    TransactionError::ValidationError(ref msg) => {
                        // Validation errors (like missing hash) indicate a fundamental problem
                        // that won't be fixed by retrying. Mark the transaction as Failed.
                        warn!(
                            tx_id = %tx.id,
                            error = %msg,
                            "validation error detected - marking transaction as failed"
                        );

                        self.mark_as_failed(tx, format!("Validation error: {msg}"))
                            .await
                    }
                    _ => {
                        // For other errors (like provider errors), log and propagate
                        // The job system will retry based on the job configuration
                        warn!(
                            tx_id = %tx.id,
                            error = ?error,
                            "status check failed with retriable error, will retry"
                        );
                        Err(error)
                    }
                }
            }
        }
    }

    /// Core status checking logic - pure business logic without error handling concerns.
    async fn status_core(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        // Early exits for unsubmitted transactions - they don't have on-chain hashes yet
        // The submit handler will schedule status checks after submission
        if tx.status == TransactionStatus::Pending {
            return self.handle_pending_state(tx).await;
        }

        if tx.status == TransactionStatus::Sent {
            return self.handle_sent_state(tx).await;
        }

        let stellar_hash = match self.parse_and_validate_hash(&tx) {
            Ok(hash) => hash,
            Err(e) => {
                // Transaction should be in Submitted or later state
                // If hash is missing, this is a database inconsistency that won't fix itself
                warn!(
                    tx_id = %tx.id,
                    status = ?tx.status,
                    error = ?e,
                    "failed to parse and validate hash for submitted transaction"
                );
                return self
                    .mark_as_failed(tx, format!("Failed to parse and validate hash: {e}"))
                    .await;
            }
        };

        let provider_response = match self.provider().get_transaction(&stellar_hash).await {
            Ok(response) => response,
            Err(e) => {
                warn!(error = ?e, "provider get_transaction failed");
                return Err(TransactionError::from(e));
            }
        };

        match provider_response.status.as_str().to_uppercase().as_str() {
            "SUCCESS" => self.handle_stellar_success(tx, provider_response).await,
            "FAILED" => self.handle_stellar_failed(tx, provider_response).await,
            _ => {
                self.handle_stellar_pending(tx, provider_response.status)
                    .await
            }
        }
    }

    /// Parses the transaction hash from the network data and validates it.
    /// Returns a `TransactionError::ValidationError` if the hash is missing, empty, or invalid.
    pub fn parse_and_validate_hash(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<Hash, TransactionError> {
        let stellar_network_data = tx.network_data.get_stellar_transaction_data()?;

        let tx_hash_str = stellar_network_data.hash.as_deref().filter(|s| !s.is_empty()).ok_or_else(|| {
            TransactionError::ValidationError(format!(
                "Stellar transaction {} is missing or has an empty on-chain hash in network_data. Cannot check status.",
                tx.id
            ))
        })?;

        let stellar_hash: Hash = tx_hash_str.parse().map_err(|e: Error| {
            TransactionError::UnexpectedError(format!(
                "Failed to parse transaction hash '{}' for tx {}: {:?}. This hash may be corrupted or not a valid Stellar hash.",
                tx_hash_str, tx.id, e
            ))
        })?;

        Ok(stellar_hash)
    }

    /// Mark a transaction as failed with a reason
    pub(super) async fn mark_as_failed(
        &self,
        tx: TransactionRepoModel,
        reason: String,
    ) -> Result<TransactionRepoModel, TransactionError> {
        warn!(tx_id = %tx.id, reason = %reason, "marking transaction as failed");

        let update_request = TransactionUpdateRequest {
            status: Some(TransactionStatus::Failed),
            status_reason: Some(reason),
            ..Default::default()
        };

        let failed_tx = self
            .finalize_transaction_state(tx.id.clone(), update_request)
            .await?;

        // Try to enqueue next transaction
        if let Err(e) = self.enqueue_next_pending_transaction(&tx.id).await {
            warn!(error = %e, "failed to enqueue next pending transaction after failure");
        }

        Ok(failed_tx)
    }

    /// Mark a transaction as expired with a reason
    pub(super) async fn mark_as_expired(
        &self,
        tx: TransactionRepoModel,
        reason: String,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!(tx_id = %tx.id, reason = %reason, "marking transaction as expired");

        let update_request = TransactionUpdateRequest {
            status: Some(TransactionStatus::Expired),
            status_reason: Some(reason),
            ..Default::default()
        };

        let expired_tx = self
            .finalize_transaction_state(tx.id.clone(), update_request)
            .await?;

        // Try to enqueue next transaction
        if let Err(e) = self.enqueue_next_pending_transaction(&tx.id).await {
            warn!(tx_id = %tx.id, relayer_id = %tx.relayer_id, error = %e, "failed to enqueue next pending transaction after expiration");
        }

        Ok(expired_tx)
    }

    /// Check if expired: valid_until > XDR time_bounds > false
    pub(super) fn is_transaction_expired(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<bool, TransactionError> {
        if let Some(valid_until_str) = &tx.valid_until {
            return Ok(Self::is_valid_until_string_expired(valid_until_str));
        }

        // Fallback: parse signed_envelope_xdr for legacy rows
        let stellar_data = tx.network_data.get_stellar_transaction_data()?;
        if let Some(signed_xdr) = &stellar_data.signed_envelope_xdr {
            if let Ok(envelope) = parse_transaction_xdr(signed_xdr, true) {
                if let Some(tb) = extract_time_bounds(&envelope) {
                    if tb.max_time.0 == 0 {
                        return Ok(false); // unbounded
                    }
                    return Ok(Utc::now().timestamp() as u64 > tb.max_time.0);
                }
            }
        }

        Ok(false)
    }

    /// Check if a valid_until string has expired (RFC3339 or numeric timestamp).
    fn is_valid_until_string_expired(valid_until: &str) -> bool {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(valid_until) {
            return Utc::now() > dt.with_timezone(&Utc);
        }
        match valid_until.parse::<i64>() {
            Ok(0) => false,
            Ok(ts) => Utc::now().timestamp() > ts,
            Err(_) => false,
        }
    }

    /// Handles the logic when a Stellar transaction is confirmed successfully.
    pub async fn handle_stellar_success(
        &self,
        tx: TransactionRepoModel,
        provider_response: soroban_rs::stellar_rpc_client::GetTransactionResponse,
    ) -> Result<TransactionRepoModel, TransactionError> {
        // Extract the actual fee charged and transaction result from the transaction response
        let updated_network_data =
            tx.network_data
                .get_stellar_transaction_data()
                .ok()
                .map(|mut stellar_data| {
                    // Update fee if available
                    if let Some(tx_result) = provider_response.result.as_ref() {
                        stellar_data = stellar_data.with_fee(tx_result.fee_charged as u32);
                    }

                    // Extract transaction result XDR from result_meta if available
                    if let Some(result_meta) = provider_response.result_meta.as_ref() {
                        if let Some(return_value) = extract_return_value_from_meta(result_meta) {
                            let xdr_base64 = return_value.to_xdr_base64(Limits::none());
                            if let Ok(xdr_base64) = xdr_base64 {
                                stellar_data = stellar_data.with_transaction_result_xdr(xdr_base64);
                            } else {
                                warn!("Failed to serialize return value to XDR base64");
                            }
                        }
                    }

                    NetworkTransactionData::Stellar(stellar_data)
                });

        let update_request = TransactionUpdateRequest {
            status: Some(TransactionStatus::Confirmed),
            confirmed_at: Some(Utc::now().to_rfc3339()),
            network_data: updated_network_data,
            ..Default::default()
        };

        let confirmed_tx = self
            .finalize_transaction_state(tx.id.clone(), update_request)
            .await?;

        self.enqueue_next_pending_transaction(&tx.id).await?;

        Ok(confirmed_tx)
    }

    /// Handles the logic when a Stellar transaction has failed.
    pub async fn handle_stellar_failed(
        &self,
        tx: TransactionRepoModel,
        provider_response: soroban_rs::stellar_rpc_client::GetTransactionResponse,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let result_code = provider_response
            .result
            .as_ref()
            .map(|r| r.result.name())
            .unwrap_or("unknown");

        // Extract inner failure fields for fee-bump and op-level detail
        let (inner_result_code, op_result_code, inner_tx_hash, inner_fee_charged) =
            match provider_response.result.as_ref().map(|r| &r.result) {
                Some(TransactionResultResult::TxFeeBumpInnerFailed(pair)) => {
                    let inner = &pair.result.result;
                    let op = match inner {
                        InnerTransactionResultResult::TxFailed(ops) => {
                            first_failing_op(ops.as_slice())
                        }
                        _ => None,
                    };
                    (
                        Some(inner.name()),
                        op,
                        Some(hex::encode(pair.transaction_hash.0)),
                        pair.result.fee_charged,
                    )
                }
                Some(TransactionResultResult::TxFailed(ops)) => {
                    (None, first_failing_op(ops.as_slice()), None, 0)
                }
                _ => (None, None, None, 0),
            };

        let fee_charged = provider_response.result.as_ref().map(|r| r.fee_charged);
        let fee_bid = provider_response.envelope.as_ref().map(extract_fee_bid);

        warn!(
            tx_id = %tx.id,
            result_code,
            inner_result_code = inner_result_code.unwrap_or("n/a"),
            op_result_code = op_result_code.unwrap_or("n/a"),
            inner_tx_hash = inner_tx_hash.as_deref().unwrap_or("n/a"),
            inner_fee_charged,
            fee_charged = ?fee_charged,
            fee_bid = ?fee_bid,
            "stellar transaction failed"
        );

        let status_reason = format!(
            "Transaction failed on-chain. Provider status: FAILED. Specific XDR reason: {result_code}."
        );

        let update_request = TransactionUpdateRequest {
            status: Some(TransactionStatus::Failed),
            status_reason: Some(status_reason),
            ..Default::default()
        };

        let updated_tx = self
            .finalize_transaction_state(tx.id.clone(), update_request)
            .await?;

        self.enqueue_next_pending_transaction(&tx.id).await?;

        Ok(updated_tx)
    }

    /// Checks if transaction has expired or exceeded max lifetime.
    /// Returns Some(Result) if transaction was handled (expired or failed), None if checks passed.
    async fn check_expiration_and_max_lifetime(
        &self,
        tx: TransactionRepoModel,
        failed_reason: String,
    ) -> Option<Result<TransactionRepoModel, TransactionError>> {
        let age = match get_age_since_created(&tx) {
            Ok(age) => age,
            Err(e) => return Some(Err(e)),
        };

        // Check if transaction has expired
        if let Ok(true) = self.is_transaction_expired(&tx) {
            info!(tx_id = %tx.id, valid_until = ?tx.valid_until, "Transaction has expired");
            return Some(
                self.mark_as_expired(tx, "Transaction time_bounds expired".to_string())
                    .await,
            );
        }

        // Check if transaction exceeded max lifetime
        if age > get_stellar_max_stuck_transaction_lifetime() {
            warn!(tx_id = %tx.id, age_minutes = age.num_minutes(),
                "Transaction exceeded max lifetime, marking as Failed");
            return Some(self.mark_as_failed(tx, failed_reason).await);
        }

        None
    }

    /// Handles Sent transactions that failed hash parsing.
    /// Checks for expiration, max lifetime, and re-enqueues submit job if needed.
    async fn handle_sent_state(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        // Check expiration and max lifetime
        if let Some(result) = self
            .check_expiration_and_max_lifetime(
                tx.clone(),
                "Transaction stuck in Sent status for too long".to_string(),
            )
            .await
        {
            return result;
        }

        // Re-enqueue submit job if transaction exceeded resend timeout
        let age = get_age_since_sent_or_created(&tx)?;
        if age > get_stellar_resend_timeout() {
            info!(tx_id = %tx.id, age_seconds = age.num_seconds(),
                "re-enqueueing submit job for stuck Sent transaction");
            send_submit_transaction_job(self.job_producer(), &tx, None).await?;
        }

        Ok(tx)
    }

    /// Handles pending transactions without a hash (e.g., reset after bad sequence error).
    /// Schedules a recovery job if the transaction is old enough to prevent it from being stuck.
    async fn handle_pending_state(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        // Check expiration and max lifetime
        if let Some(result) = self
            .check_expiration_and_max_lifetime(
                tx.clone(),
                "Transaction stuck in Pending status for too long".to_string(),
            )
            .await
        {
            return result;
        }

        // Check transaction age to determine if recovery is needed
        let age = self.get_time_since_created_at(&tx)?;

        // Only schedule recovery job if transaction exceeds recovery trigger timeout
        // This prevents scheduling a job on every status check
        if age.num_seconds() >= STELLAR_PENDING_RECOVERY_TRIGGER_SECONDS {
            info!(
                tx_id = %tx.id,
                age_seconds = age.num_seconds(),
                "pending transaction without hash may be stuck, scheduling recovery job"
            );

            let transaction_request = TransactionRequest::new(tx.id.clone(), tx.relayer_id.clone());
            if let Err(e) = self
                .job_producer()
                .produce_transaction_request_job(transaction_request, None)
                .await
            {
                warn!(
                    tx_id = %tx.id,
                    error = %e,
                    "failed to schedule recovery job for pending transaction"
                );
            }
        } else {
            debug!(
                tx_id = %tx.id,
                age_seconds = age.num_seconds(),
                "pending transaction without hash too young for recovery check"
            );
        }

        Ok(tx)
    }

    /// Get time since transaction was created.
    /// Returns an error if created_at is missing or invalid.
    fn get_time_since_created_at(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<chrono::Duration, TransactionError> {
        match DateTime::parse_from_rfc3339(&tx.created_at) {
            Ok(dt) => Ok(Utc::now().signed_duration_since(dt.with_timezone(&Utc))),
            Err(e) => {
                warn!(tx_id = %tx.id, ts = %tx.created_at, error = %e, "failed to parse created_at timestamp");
                Err(TransactionError::UnexpectedError(format!(
                    "Invalid created_at timestamp for transaction {}: {}",
                    tx.id, e
                )))
            }
        }
    }

    /// Handles the logic when a Stellar transaction is still pending or in an unknown state.
    pub async fn handle_stellar_pending(
        &self,
        tx: TransactionRepoModel,
        original_status_str: String,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(
            tx_id = %tx.id,
            relayer_id = %tx.relayer_id,
            status = %original_status_str,
            "stellar transaction status is still pending, will retry check later"
        );

        // Check for expiration and max lifetime for Submitted transactions
        if tx.status == TransactionStatus::Submitted {
            if let Some(result) = self
                .check_expiration_and_max_lifetime(
                    tx.clone(),
                    "Transaction stuck in Submitted status for too long".to_string(),
                )
                .await
            {
                return result;
            }
        }

        Ok(tx)
    }
}

/// Extracts the fee bid from a transaction envelope.
///
/// For fee-bump transactions, returns the outer bump fee (the max the submitter was
/// willing to pay). For regular V1 transactions, returns the `fee` field.
fn extract_fee_bid(envelope: &TransactionEnvelope) -> i64 {
    match envelope {
        TransactionEnvelope::TxFeeBump(fb) => fb.tx.fee,
        TransactionEnvelope::Tx(v1) => v1.tx.fee as i64,
        TransactionEnvelope::TxV0(v0) => v0.tx.fee as i64,
    }
}

/// Returns the `.name()` of the first failing operation in the results.
///
/// Scans left-to-right since earlier operations may show success while a later
/// one carries the actual failure code. Returns `None` if no failure is found.
fn first_failing_op(ops: &[OperationResult]) -> Option<&'static str> {
    let op = ops.iter().find(|op| match op {
        OperationResult::OpInner(tr) => match tr {
            OperationResultTr::InvokeHostFunction(r) => {
                !matches!(r, InvokeHostFunctionResult::Success(_))
            }
            OperationResultTr::ExtendFootprintTtl(r) => r.name() != "Success",
            OperationResultTr::RestoreFootprint(r) => r.name() != "Success",
            _ => false,
        },
        _ => true,
    })?;
    match op {
        OperationResult::OpInner(tr) => match tr {
            OperationResultTr::InvokeHostFunction(r) => Some(r.name()),
            OperationResultTr::ExtendFootprintTtl(r) => Some(r.name()),
            OperationResultTr::RestoreFootprint(r) => Some(r.name()),
            _ => Some(tr.name()),
        },
        _ => Some(op.name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{NetworkTransactionData, RepositoryError};
    use crate::repositories::PaginatedResult;
    use chrono::Duration;
    use mockall::predicate::eq;
    use soroban_rs::stellar_rpc_client::GetTransactionResponse;

    use crate::domain::transaction::stellar::test_helpers::*;

    fn dummy_get_transaction_response(status: &str) -> GetTransactionResponse {
        GetTransactionResponse {
            status: status.to_string(),
            ledger: None,
            envelope: None,
            result: None,
            result_meta: None,
            events: soroban_rs::stellar_rpc_client::GetTransactionEvents {
                contract_events: vec![],
                diagnostic_events: vec![],
                transaction_events: vec![],
            },
        }
    }

    fn dummy_get_transaction_response_with_result_meta(
        status: &str,
        has_return_value: bool,
    ) -> GetTransactionResponse {
        use soroban_rs::xdr::{ScVal, SorobanTransactionMeta, TransactionMeta, TransactionMetaV3};

        let result_meta = if has_return_value {
            // Create a dummy ScVal for testing (using I32(42) as a simple test value)
            let return_value = ScVal::I32(42);
            Some(TransactionMeta::V3(TransactionMetaV3 {
                ext: soroban_rs::xdr::ExtensionPoint::V0,
                tx_changes_before: soroban_rs::xdr::LedgerEntryChanges::default(),
                operations: soroban_rs::xdr::VecM::default(),
                tx_changes_after: soroban_rs::xdr::LedgerEntryChanges::default(),
                soroban_meta: Some(SorobanTransactionMeta {
                    ext: soroban_rs::xdr::SorobanTransactionMetaExt::V0,
                    return_value,
                    events: soroban_rs::xdr::VecM::default(),
                    diagnostic_events: soroban_rs::xdr::VecM::default(),
                }),
            }))
        } else {
            None
        };

        GetTransactionResponse {
            status: status.to_string(),
            ledger: None,
            envelope: None,
            result: None,
            result_meta,
            events: soroban_rs::stellar_rpc_client::GetTransactionEvents {
                contract_events: vec![],
                diagnostic_events: vec![],
                transaction_events: vec![],
            },
        }
    }

    mod handle_transaction_status_tests {
        use crate::services::provider::ProviderError;

        use super::*;

        #[tokio::test]
        async fn handle_transaction_status_confirmed_triggers_next() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.id = "tx-confirm-this".to_string();
            tx_to_handle.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
            let tx_hash_bytes = [1u8; 32];
            let tx_hash_hex = hex::encode(tx_hash_bytes);
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx_to_handle.network_data
            {
                stellar_data.hash = Some(tx_hash_hex.clone());
            } else {
                panic!("Expected Stellar network data for tx_to_handle");
            }
            tx_to_handle.status = TransactionStatus::Submitted;

            let expected_stellar_hash = soroban_rs::xdr::Hash(tx_hash_bytes);

            // 1. Mock provider to return SUCCESS
            mocks
                .provider
                .expect_get_transaction()
                .with(eq(expected_stellar_hash.clone()))
                .times(1)
                .returning(move |_| {
                    Box::pin(async { Ok(dummy_get_transaction_response("SUCCESS")) })
                });

            // 2. Mock partial_update for confirmation
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(move |id, update| {
                    id == "tx-confirm-this"
                        && update.status == Some(TransactionStatus::Confirmed)
                        && update.confirmed_at.is_some()
                })
                .times(1)
                .returning(move |id, update| {
                    let mut updated_tx = tx_to_handle.clone(); // Use the original tx_to_handle as base
                    updated_tx.id = id;
                    updated_tx.status = update.status.unwrap();
                    updated_tx.confirmed_at = update.confirmed_at;
                    Ok(updated_tx)
                });

            // Send notification for confirmed tx
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // 3. Mock find_by_status_paginated for pending transactions
            let mut oldest_pending_tx = create_test_transaction(&relayer.id);
            oldest_pending_tx.id = "tx-oldest-pending".to_string();
            oldest_pending_tx.status = TransactionStatus::Pending;
            let captured_oldest_pending_tx = oldest_pending_tx.clone();
            let relayer_id_clone = relayer.id.clone();
            mocks
                .tx_repo
                .expect_find_by_status_paginated()
                .withf(move |relayer_id, statuses, query, oldest_first| {
                    *relayer_id == relayer_id_clone
                        && statuses == [TransactionStatus::Pending]
                        && query.page == 1
                        && query.per_page == 1
                        && *oldest_first
                })
                .times(1)
                .returning(move |_, _, _, _| {
                    Ok(PaginatedResult {
                        items: vec![captured_oldest_pending_tx.clone()],
                        total: 1,
                        page: 1,
                        per_page: 1,
                    })
                });

            // 4. Mock produce_transaction_request_job for the next pending transaction
            mocks
                .job_producer
                .expect_produce_transaction_request_job()
                .withf(move |job, _delay| job.transaction_id == "tx-oldest-pending")
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut initial_tx_for_handling = create_test_transaction(&relayer.id);
            initial_tx_for_handling.id = "tx-confirm-this".to_string();
            initial_tx_for_handling.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
            if let NetworkTransactionData::Stellar(ref mut stellar_data) =
                initial_tx_for_handling.network_data
            {
                stellar_data.hash = Some(hex::encode(tx_hash_bytes));
            } else {
                panic!("Expected Stellar network data for initial_tx_for_handling");
            }
            initial_tx_for_handling.status = TransactionStatus::Submitted;

            let result = handler
                .handle_transaction_status_impl(initial_tx_for_handling, None)
                .await;

            assert!(result.is_ok());
            let handled_tx = result.unwrap();
            assert_eq!(handled_tx.id, "tx-confirm-this");
            assert_eq!(handled_tx.status, TransactionStatus::Confirmed);
            assert!(handled_tx.confirmed_at.is_some());
        }

        #[tokio::test]
        async fn handle_transaction_status_still_pending() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.id = "tx-pending-check".to_string();
            tx_to_handle.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
            let tx_hash_bytes = [2u8; 32];
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx_to_handle.network_data
            {
                stellar_data.hash = Some(hex::encode(tx_hash_bytes));
            } else {
                panic!("Expected Stellar network data");
            }
            tx_to_handle.status = TransactionStatus::Submitted; // Or any status that implies it's being watched

            let expected_stellar_hash = soroban_rs::xdr::Hash(tx_hash_bytes);

            // 1. Mock provider to return PENDING
            mocks
                .provider
                .expect_get_transaction()
                .with(eq(expected_stellar_hash.clone()))
                .times(1)
                .returning(move |_| {
                    Box::pin(async { Ok(dummy_get_transaction_response("PENDING")) })
                });

            // 2. Mock partial_update: should NOT be called
            mocks.tx_repo.expect_partial_update().never();

            // Notifications should NOT be sent for pending
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .never();

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let original_tx_clone = tx_to_handle.clone();

            let result = handler
                .handle_transaction_status_impl(tx_to_handle, None)
                .await;

            assert!(result.is_ok());
            let returned_tx = result.unwrap();
            // Transaction should be returned unchanged as it's still pending
            assert_eq!(returned_tx.id, original_tx_clone.id);
            assert_eq!(returned_tx.status, original_tx_clone.status);
            assert!(returned_tx.confirmed_at.is_none()); // Ensure it wasn't accidentally confirmed
        }

        #[tokio::test]
        async fn handle_transaction_status_failed() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.id = "tx-fail-this".to_string();
            tx_to_handle.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
            let tx_hash_bytes = [3u8; 32];
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx_to_handle.network_data
            {
                stellar_data.hash = Some(hex::encode(tx_hash_bytes));
            } else {
                panic!("Expected Stellar network data");
            }
            tx_to_handle.status = TransactionStatus::Submitted;

            let expected_stellar_hash = soroban_rs::xdr::Hash(tx_hash_bytes);

            // 1. Mock provider to return FAILED
            mocks
                .provider
                .expect_get_transaction()
                .with(eq(expected_stellar_hash.clone()))
                .times(1)
                .returning(move |_| {
                    Box::pin(async { Ok(dummy_get_transaction_response("FAILED")) })
                });

            // 2. Mock partial_update for failure - use actual update values
            let relayer_id_for_mock = relayer.id.clone();
            mocks
                .tx_repo
                .expect_partial_update()
                .times(1)
                .returning(move |id, update| {
                    // Use the actual update values instead of hardcoding
                    let mut updated_tx = create_test_transaction(&relayer_id_for_mock);
                    updated_tx.id = id;
                    updated_tx.status = update.status.unwrap();
                    updated_tx.status_reason = update.status_reason.clone();
                    Ok::<_, RepositoryError>(updated_tx)
                });

            // Send notification for failed tx
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // 3. Mock find_by_status_paginated for pending transactions (should be called by enqueue_next_pending_transaction)
            let relayer_id_clone = relayer.id.clone();
            mocks
                .tx_repo
                .expect_find_by_status_paginated()
                .withf(move |relayer_id, statuses, query, oldest_first| {
                    *relayer_id == relayer_id_clone
                        && statuses == [TransactionStatus::Pending]
                        && query.page == 1
                        && query.per_page == 1
                        && *oldest_first
                })
                .times(1)
                .returning(move |_, _, _, _| {
                    Ok(PaginatedResult {
                        items: vec![],
                        total: 0,
                        page: 1,
                        per_page: 1,
                    })
                }); // No pending transactions

            // Should NOT try to enqueue next transaction since there are no pending ones
            mocks
                .job_producer
                .expect_produce_transaction_request_job()
                .never();
            // Should NOT re-queue status check
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .never();

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut initial_tx_for_handling = create_test_transaction(&relayer.id);
            initial_tx_for_handling.id = "tx-fail-this".to_string();
            initial_tx_for_handling.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
            if let NetworkTransactionData::Stellar(ref mut stellar_data) =
                initial_tx_for_handling.network_data
            {
                stellar_data.hash = Some(hex::encode(tx_hash_bytes));
            } else {
                panic!("Expected Stellar network data");
            }
            initial_tx_for_handling.status = TransactionStatus::Submitted;

            let result = handler
                .handle_transaction_status_impl(initial_tx_for_handling, None)
                .await;

            assert!(result.is_ok());
            let handled_tx = result.unwrap();
            assert_eq!(handled_tx.id, "tx-fail-this");
            assert_eq!(handled_tx.status, TransactionStatus::Failed);
            assert!(handled_tx.status_reason.is_some());
            assert_eq!(
                handled_tx.status_reason.unwrap(),
                "Transaction failed on-chain. Provider status: FAILED. Specific XDR reason: unknown."
            );
        }

        #[tokio::test]
        async fn handle_transaction_status_provider_error() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.id = "tx-provider-error".to_string();
            tx_to_handle.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
            let tx_hash_bytes = [4u8; 32];
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx_to_handle.network_data
            {
                stellar_data.hash = Some(hex::encode(tx_hash_bytes));
            } else {
                panic!("Expected Stellar network data");
            }
            tx_to_handle.status = TransactionStatus::Submitted;

            let expected_stellar_hash = soroban_rs::xdr::Hash(tx_hash_bytes);

            // 1. Mock provider to return an error
            mocks
                .provider
                .expect_get_transaction()
                .with(eq(expected_stellar_hash.clone()))
                .times(1)
                .returning(move |_| {
                    Box::pin(async { Err(ProviderError::Other("RPC boom".to_string())) })
                });

            // 2. Mock partial_update: should NOT be called
            mocks.tx_repo.expect_partial_update().never();

            // Notifications should NOT be sent
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .never();
            // Should NOT try to enqueue next transaction
            mocks
                .job_producer
                .expect_produce_transaction_request_job()
                .never();

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);

            let result = handler
                .handle_transaction_status_impl(tx_to_handle, None)
                .await;

            // Provider errors are now propagated as errors (retriable)
            assert!(result.is_err());
            matches!(result.unwrap_err(), TransactionError::UnderlyingProvider(_));
        }

        #[tokio::test]
        async fn handle_transaction_status_no_hashes() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.id = "tx-no-hashes".to_string();
            tx_to_handle.status = TransactionStatus::Submitted;
            tx_to_handle.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();

            // With our new error handling, validation errors mark the transaction as failed
            mocks.provider.expect_get_transaction().never();

            // Expect partial_update to be called to mark as failed
            mocks
                .tx_repo
                .expect_partial_update()
                .times(1)
                .returning(|_, update| {
                    let mut updated_tx = create_test_transaction("test-relayer");
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    updated_tx.status_reason = update.status_reason.clone();
                    Ok(updated_tx)
                });

            // Expect notification to be sent after marking as failed
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Expect find_by_status_paginated to be called when enqueuing next transaction
            let relayer_id_clone = relayer.id.clone();
            mocks
                .tx_repo
                .expect_find_by_status_paginated()
                .withf(move |relayer_id, statuses, query, oldest_first| {
                    *relayer_id == relayer_id_clone
                        && statuses == [TransactionStatus::Pending]
                        && query.page == 1
                        && query.per_page == 1
                        && *oldest_first
                })
                .times(1)
                .returning(move |_, _, _, _| {
                    Ok(PaginatedResult {
                        items: vec![],
                        total: 0,
                        page: 1,
                        per_page: 1,
                    })
                }); // No pending transactions

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let result = handler
                .handle_transaction_status_impl(tx_to_handle, None)
                .await;

            // Should succeed but mark transaction as Failed
            assert!(result.is_ok(), "Expected Ok result");
            let updated_tx = result.unwrap();
            assert_eq!(updated_tx.status, TransactionStatus::Failed);
            assert!(
                updated_tx
                    .status_reason
                    .as_ref()
                    .unwrap()
                    .contains("Failed to parse and validate hash"),
                "Expected hash validation error in status_reason, got: {:?}",
                updated_tx.status_reason
            );
        }

        #[tokio::test]
        async fn test_on_chain_failure_does_not_decrement_sequence() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.id = "tx-on-chain-fail".to_string();
            tx_to_handle.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
            let tx_hash_bytes = [4u8; 32];
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx_to_handle.network_data
            {
                stellar_data.hash = Some(hex::encode(tx_hash_bytes));
                stellar_data.sequence_number = Some(100); // Has a sequence
            }
            tx_to_handle.status = TransactionStatus::Submitted;

            let expected_stellar_hash = soroban_rs::xdr::Hash(tx_hash_bytes);

            // Mock provider to return FAILED (on-chain failure)
            mocks
                .provider
                .expect_get_transaction()
                .with(eq(expected_stellar_hash.clone()))
                .times(1)
                .returning(move |_| {
                    Box::pin(async { Ok(dummy_get_transaction_response("FAILED")) })
                });

            // Decrement should NEVER be called for on-chain failures
            mocks.counter.expect_decrement().never();

            // Mock partial_update for failure
            mocks
                .tx_repo
                .expect_partial_update()
                .times(1)
                .returning(move |id, update| {
                    let mut updated_tx = create_test_transaction("test");
                    updated_tx.id = id;
                    updated_tx.status = update.status.unwrap();
                    updated_tx.status_reason = update.status_reason.clone();
                    Ok::<_, RepositoryError>(updated_tx)
                });

            // Mock notification
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Mock find_by_status_paginated
            mocks
                .tx_repo
                .expect_find_by_status_paginated()
                .returning(move |_, _, _, _| {
                    Ok(PaginatedResult {
                        items: vec![],
                        total: 0,
                        page: 1,
                        per_page: 1,
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let initial_tx = tx_to_handle.clone();

            let result = handler
                .handle_transaction_status_impl(initial_tx, None)
                .await;

            assert!(result.is_ok());
            let handled_tx = result.unwrap();
            assert_eq!(handled_tx.id, "tx-on-chain-fail");
            assert_eq!(handled_tx.status, TransactionStatus::Failed);
        }

        #[tokio::test]
        async fn test_on_chain_success_does_not_decrement_sequence() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.id = "tx-on-chain-success".to_string();
            tx_to_handle.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
            let tx_hash_bytes = [5u8; 32];
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx_to_handle.network_data
            {
                stellar_data.hash = Some(hex::encode(tx_hash_bytes));
                stellar_data.sequence_number = Some(101); // Has a sequence
            }
            tx_to_handle.status = TransactionStatus::Submitted;

            let expected_stellar_hash = soroban_rs::xdr::Hash(tx_hash_bytes);

            // Mock provider to return SUCCESS
            mocks
                .provider
                .expect_get_transaction()
                .with(eq(expected_stellar_hash.clone()))
                .times(1)
                .returning(move |_| {
                    Box::pin(async { Ok(dummy_get_transaction_response("SUCCESS")) })
                });

            // Decrement should NEVER be called for on-chain success
            mocks.counter.expect_decrement().never();

            // Mock partial_update for confirmation
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(move |id, update| {
                    id == "tx-on-chain-success"
                        && update.status == Some(TransactionStatus::Confirmed)
                        && update.confirmed_at.is_some()
                })
                .times(1)
                .returning(move |id, update| {
                    let mut updated_tx = create_test_transaction("test");
                    updated_tx.id = id;
                    updated_tx.status = update.status.unwrap();
                    updated_tx.confirmed_at = update.confirmed_at;
                    Ok(updated_tx)
                });

            // Mock notification
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Mock find_by_status_paginated for next transaction
            mocks
                .tx_repo
                .expect_find_by_status_paginated()
                .returning(move |_, _, _, _| {
                    Ok(PaginatedResult {
                        items: vec![],
                        total: 0,
                        page: 1,
                        per_page: 1,
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let initial_tx = tx_to_handle.clone();

            let result = handler
                .handle_transaction_status_impl(initial_tx, None)
                .await;

            assert!(result.is_ok());
            let handled_tx = result.unwrap();
            assert_eq!(handled_tx.id, "tx-on-chain-success");
            assert_eq!(handled_tx.status, TransactionStatus::Confirmed);
        }

        #[tokio::test]
        async fn test_handle_transaction_status_with_xdr_error_requeues() {
            // This test verifies that when get_transaction fails we re-queue for retry
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.id = "tx-xdr-error-requeue".to_string();
            tx_to_handle.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
            let tx_hash_bytes = [8u8; 32];
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx_to_handle.network_data
            {
                stellar_data.hash = Some(hex::encode(tx_hash_bytes));
            }
            tx_to_handle.status = TransactionStatus::Submitted;

            let expected_stellar_hash = soroban_rs::xdr::Hash(tx_hash_bytes);

            // Mock provider to return a non-XDR error (won't trigger fallback)
            mocks
                .provider
                .expect_get_transaction()
                .with(eq(expected_stellar_hash.clone()))
                .times(1)
                .returning(move |_| {
                    Box::pin(async { Err(ProviderError::Other("Network timeout".to_string())) })
                });

            // No partial update should occur
            mocks.tx_repo.expect_partial_update().never();
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .never();

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);

            let result = handler
                .handle_transaction_status_impl(tx_to_handle, None)
                .await;

            // Provider errors are now propagated as errors (retriable)
            assert!(result.is_err());
            matches!(result.unwrap_err(), TransactionError::UnderlyingProvider(_));
        }

        #[tokio::test]
        async fn handle_transaction_status_extracts_transaction_result_xdr() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.id = "tx-with-result".to_string();
            tx_to_handle.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
            let tx_hash_bytes = [9u8; 32];
            let tx_hash_hex = hex::encode(tx_hash_bytes);
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx_to_handle.network_data
            {
                stellar_data.hash = Some(tx_hash_hex.clone());
            } else {
                panic!("Expected Stellar network data");
            }
            tx_to_handle.status = TransactionStatus::Submitted;

            let expected_stellar_hash = soroban_rs::xdr::Hash(tx_hash_bytes);

            // Mock provider to return SUCCESS with result_meta containing return_value
            mocks
                .provider
                .expect_get_transaction()
                .with(eq(expected_stellar_hash.clone()))
                .times(1)
                .returning(move |_| {
                    Box::pin(async {
                        Ok(dummy_get_transaction_response_with_result_meta(
                            "SUCCESS", true,
                        ))
                    })
                });

            // Mock partial_update - verify that transaction_result_xdr is stored
            let tx_to_handle_clone = tx_to_handle.clone();
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(move |id, update| {
                    id == "tx-with-result"
                        && update.status == Some(TransactionStatus::Confirmed)
                        && update.confirmed_at.is_some()
                        && update.network_data.as_ref().is_some_and(|and| {
                            if let NetworkTransactionData::Stellar(stellar_data) = and {
                                // Verify transaction_result_xdr is present
                                stellar_data.transaction_result_xdr.is_some()
                            } else {
                                false
                            }
                        })
                })
                .times(1)
                .returning(move |id, update| {
                    let mut updated_tx = tx_to_handle_clone.clone();
                    updated_tx.id = id;
                    updated_tx.status = update.status.unwrap();
                    updated_tx.confirmed_at = update.confirmed_at;
                    if let Some(network_data) = update.network_data {
                        updated_tx.network_data = network_data;
                    }
                    Ok(updated_tx)
                });

            // Mock notification
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Mock find_by_status_paginated
            mocks
                .tx_repo
                .expect_find_by_status_paginated()
                .returning(move |_, _, _, _| {
                    Ok(PaginatedResult {
                        items: vec![],
                        total: 0,
                        page: 1,
                        per_page: 1,
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let result = handler
                .handle_transaction_status_impl(tx_to_handle, None)
                .await;

            assert!(result.is_ok());
            let handled_tx = result.unwrap();
            assert_eq!(handled_tx.id, "tx-with-result");
            assert_eq!(handled_tx.status, TransactionStatus::Confirmed);

            // Verify transaction_result_xdr is stored
            if let NetworkTransactionData::Stellar(stellar_data) = handled_tx.network_data {
                assert!(
                    stellar_data.transaction_result_xdr.is_some(),
                    "transaction_result_xdr should be stored when result_meta contains return_value"
                );
            } else {
                panic!("Expected Stellar network data");
            }
        }

        #[tokio::test]
        async fn handle_transaction_status_no_result_meta_does_not_store_xdr() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.id = "tx-no-result-meta".to_string();
            tx_to_handle.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
            let tx_hash_bytes = [10u8; 32];
            let tx_hash_hex = hex::encode(tx_hash_bytes);
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx_to_handle.network_data
            {
                stellar_data.hash = Some(tx_hash_hex.clone());
            } else {
                panic!("Expected Stellar network data");
            }
            tx_to_handle.status = TransactionStatus::Submitted;

            let expected_stellar_hash = soroban_rs::xdr::Hash(tx_hash_bytes);

            // Mock provider to return SUCCESS without result_meta
            mocks
                .provider
                .expect_get_transaction()
                .with(eq(expected_stellar_hash.clone()))
                .times(1)
                .returning(move |_| {
                    Box::pin(async {
                        Ok(dummy_get_transaction_response_with_result_meta(
                            "SUCCESS", false,
                        ))
                    })
                });

            // Mock partial_update
            let tx_to_handle_clone = tx_to_handle.clone();
            mocks
                .tx_repo
                .expect_partial_update()
                .times(1)
                .returning(move |id, update| {
                    let mut updated_tx = tx_to_handle_clone.clone();
                    updated_tx.id = id;
                    updated_tx.status = update.status.unwrap();
                    updated_tx.confirmed_at = update.confirmed_at;
                    if let Some(network_data) = update.network_data {
                        updated_tx.network_data = network_data;
                    }
                    Ok(updated_tx)
                });

            // Mock notification
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Mock find_by_status_paginated
            mocks
                .tx_repo
                .expect_find_by_status_paginated()
                .returning(move |_, _, _, _| {
                    Ok(PaginatedResult {
                        items: vec![],
                        total: 0,
                        page: 1,
                        per_page: 1,
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let result = handler
                .handle_transaction_status_impl(tx_to_handle, None)
                .await;

            assert!(result.is_ok());
            let handled_tx = result.unwrap();

            // Verify transaction_result_xdr is None when result_meta is missing
            if let NetworkTransactionData::Stellar(stellar_data) = handled_tx.network_data {
                assert!(
                    stellar_data.transaction_result_xdr.is_none(),
                    "transaction_result_xdr should be None when result_meta is missing"
                );
            } else {
                panic!("Expected Stellar network data");
            }
        }

        #[tokio::test]
        async fn test_sent_transaction_not_stuck_yet_returns_ok() {
            // Transaction in Sent status for < 5 minutes should NOT trigger recovery
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx = create_test_transaction(&relayer.id);
            tx.id = "tx-sent-not-stuck".to_string();
            tx.status = TransactionStatus::Sent;
            // Created just now - not stuck yet
            tx.created_at = Utc::now().to_rfc3339();
            // No hash (simulating stuck state)
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx.network_data {
                stellar_data.hash = None;
            }

            // Should NOT call any provider methods or update transaction
            mocks.provider.expect_get_transaction().never();
            mocks.tx_repo.expect_partial_update().never();
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .never();

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let result = handler
                .handle_transaction_status_impl(tx.clone(), None)
                .await;

            assert!(result.is_ok());
            let returned_tx = result.unwrap();
            // Transaction should be returned unchanged
            assert_eq!(returned_tx.id, tx.id);
            assert_eq!(returned_tx.status, TransactionStatus::Sent);
        }

        #[tokio::test]
        async fn test_stuck_sent_transaction_reenqueues_submit_job() {
            // Transaction in Sent status for > 5 minutes should re-enqueue submit job
            // The submit handler (not status checker) will handle signed XDR validation
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx = create_test_transaction(&relayer.id);
            tx.id = "tx-stuck-with-xdr".to_string();
            tx.status = TransactionStatus::Sent;
            // Created 10 minutes ago - definitely stuck
            tx.created_at = (Utc::now() - Duration::minutes(10)).to_rfc3339();
            // No hash (simulating stuck state)
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx.network_data {
                stellar_data.hash = None;
                stellar_data.signed_envelope_xdr = Some("AAAA...signed...".to_string());
            }

            // Should re-enqueue submit job (idempotent - submit handler will validate)
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let result = handler
                .handle_transaction_status_impl(tx.clone(), None)
                .await;

            assert!(result.is_ok());
            let returned_tx = result.unwrap();
            // Transaction status unchanged - submit job will handle the actual submission
            assert_eq!(returned_tx.status, TransactionStatus::Sent);
        }

        #[tokio::test]
        async fn test_stuck_sent_transaction_expired_marks_expired() {
            // Expired transaction should be marked as Expired
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx = create_test_transaction(&relayer.id);
            tx.id = "tx-expired".to_string();
            tx.status = TransactionStatus::Sent;
            // Created 10 minutes ago - definitely stuck
            tx.created_at = (Utc::now() - Duration::minutes(10)).to_rfc3339();
            // Set valid_until to a past time (expired)
            tx.valid_until = Some((Utc::now() - Duration::minutes(5)).to_rfc3339());
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx.network_data {
                stellar_data.hash = None;
                stellar_data.signed_envelope_xdr = Some("AAAA...signed...".to_string());
            }

            // Should mark as Expired
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_id, update| update.status == Some(TransactionStatus::Expired))
                .times(1)
                .returning(|id, update| {
                    let mut updated = create_test_transaction("test");
                    updated.id = id;
                    updated.status = update.status.unwrap();
                    updated.status_reason = update.status_reason.clone();
                    Ok(updated)
                });

            // Should NOT try to re-enqueue submit job (expired)
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .never();

            // Notification for expiration
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Try to enqueue next pending
            mocks
                .tx_repo
                .expect_find_by_status_paginated()
                .returning(move |_, _, _, _| {
                    Ok(PaginatedResult {
                        items: vec![],
                        total: 0,
                        page: 1,
                        per_page: 1,
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let result = handler.handle_transaction_status_impl(tx, None).await;

            assert!(result.is_ok());
            let expired_tx = result.unwrap();
            assert_eq!(expired_tx.status, TransactionStatus::Expired);
            assert!(expired_tx
                .status_reason
                .as_ref()
                .unwrap()
                .contains("expired"));
        }

        #[tokio::test]
        async fn test_stuck_sent_transaction_max_lifetime_marks_failed() {
            // Transaction stuck beyond max lifetime should be marked as Failed
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx = create_test_transaction(&relayer.id);
            tx.id = "tx-max-lifetime".to_string();
            tx.status = TransactionStatus::Sent;
            // Created 35 minutes ago - beyond 30 min max lifetime
            tx.created_at = (Utc::now() - Duration::minutes(35)).to_rfc3339();
            // No valid_until (unbounded transaction)
            tx.valid_until = None;
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx.network_data {
                stellar_data.hash = None;
                stellar_data.signed_envelope_xdr = Some("AAAA...signed...".to_string());
            }

            // Should mark as Failed (not Expired, since no time bounds)
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_id, update| update.status == Some(TransactionStatus::Failed))
                .times(1)
                .returning(|id, update| {
                    let mut updated = create_test_transaction("test");
                    updated.id = id;
                    updated.status = update.status.unwrap();
                    updated.status_reason = update.status_reason.clone();
                    Ok(updated)
                });

            // Should NOT try to re-enqueue submit job
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .never();

            // Notification for failure
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Try to enqueue next pending
            mocks
                .tx_repo
                .expect_find_by_status_paginated()
                .returning(|_, _, _, _| {
                    Ok(PaginatedResult {
                        items: vec![],
                        total: 0,
                        page: 1,
                        per_page: 1,
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let result = handler.handle_transaction_status_impl(tx, None).await;

            assert!(result.is_ok());
            let failed_tx = result.unwrap();
            assert_eq!(failed_tx.status, TransactionStatus::Failed);
            // assert_eq!(failed_tx.status_reason.as_ref().unwrap(), "Transaction stuck in Sent status for too long");
            assert!(failed_tx
                .status_reason
                .as_ref()
                .unwrap()
                .contains("stuck in Sent status for too long"));
        }
    }

    mod handle_pending_state_tests {
        use super::*;
        use crate::constants::get_stellar_max_stuck_transaction_lifetime;
        use crate::constants::STELLAR_PENDING_RECOVERY_TRIGGER_SECONDS;

        #[tokio::test]
        async fn test_pending_exceeds_max_lifetime_marks_failed() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx = create_test_transaction(&relayer.id);
            tx.id = "tx-pending-old".to_string();
            tx.status = TransactionStatus::Pending;
            // Created more than max lifetime ago (16 minutes > 15 minutes)
            tx.created_at =
                (Utc::now() - get_stellar_max_stuck_transaction_lifetime() - Duration::minutes(1))
                    .to_rfc3339();

            // Should mark as Failed
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_id, update| update.status == Some(TransactionStatus::Failed))
                .times(1)
                .returning(|id, update| {
                    let mut updated = create_test_transaction("test");
                    updated.id = id;
                    updated.status = update.status.unwrap();
                    updated.status_reason = update.status_reason.clone();
                    Ok(updated)
                });

            // Notification for failure
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Try to enqueue next pending
            mocks
                .tx_repo
                .expect_find_by_status_paginated()
                .returning(move |_, _, _, _| {
                    Ok(PaginatedResult {
                        items: vec![],
                        total: 0,
                        page: 1,
                        per_page: 1,
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let result = handler.handle_transaction_status_impl(tx, None).await;

            assert!(result.is_ok());
            let failed_tx = result.unwrap();
            assert_eq!(failed_tx.status, TransactionStatus::Failed);
            assert!(failed_tx
                .status_reason
                .as_ref()
                .unwrap()
                .contains("stuck in Pending status for too long"));
        }

        #[tokio::test]
        async fn test_pending_triggers_recovery_job_when_old_enough() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx = create_test_transaction(&relayer.id);
            tx.id = "tx-pending-recovery".to_string();
            tx.status = TransactionStatus::Pending;
            // Created more than recovery trigger seconds ago
            tx.created_at = (Utc::now()
                - Duration::seconds(STELLAR_PENDING_RECOVERY_TRIGGER_SECONDS + 5))
            .to_rfc3339();

            // Should schedule recovery job
            mocks
                .job_producer
                .expect_produce_transaction_request_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let result = handler.handle_transaction_status_impl(tx, None).await;

            assert!(result.is_ok());
            let tx_result = result.unwrap();
            assert_eq!(tx_result.status, TransactionStatus::Pending);
        }

        #[tokio::test]
        async fn test_pending_too_young_does_not_schedule_recovery() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx = create_test_transaction(&relayer.id);
            tx.id = "tx-pending-young".to_string();
            tx.status = TransactionStatus::Pending;
            // Created less than recovery trigger seconds ago
            tx.created_at = (Utc::now()
                - Duration::seconds(STELLAR_PENDING_RECOVERY_TRIGGER_SECONDS - 5))
            .to_rfc3339();

            // Should NOT schedule recovery job
            mocks
                .job_producer
                .expect_produce_transaction_request_job()
                .never();

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let result = handler.handle_transaction_status_impl(tx, None).await;

            assert!(result.is_ok());
            let tx_result = result.unwrap();
            assert_eq!(tx_result.status, TransactionStatus::Pending);
        }

        #[tokio::test]
        async fn test_sent_without_hash_handles_stuck_recovery() {
            use crate::constants::get_stellar_resend_timeout;

            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx = create_test_transaction(&relayer.id);
            tx.id = "tx-sent-no-hash".to_string();
            tx.status = TransactionStatus::Sent;
            // Created more than resend timeout ago (31 seconds > 30 seconds)
            tx.created_at =
                (Utc::now() - get_stellar_resend_timeout() - Duration::seconds(1)).to_rfc3339();
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx.network_data {
                stellar_data.hash = None; // No hash
            }

            // Should handle stuck Sent transaction and re-enqueue submit job
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let result = handler.handle_transaction_status_impl(tx, None).await;

            assert!(result.is_ok());
            let tx_result = result.unwrap();
            assert_eq!(tx_result.status, TransactionStatus::Sent);
        }

        #[tokio::test]
        async fn test_submitted_without_hash_marks_failed() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx = create_test_transaction(&relayer.id);
            tx.id = "tx-submitted-no-hash".to_string();
            tx.status = TransactionStatus::Submitted;
            tx.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx.network_data {
                stellar_data.hash = None; // No hash
            }

            // Should mark as Failed
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_id, update| update.status == Some(TransactionStatus::Failed))
                .times(1)
                .returning(|id, update| {
                    let mut updated = create_test_transaction("test");
                    updated.id = id;
                    updated.status = update.status.unwrap();
                    updated.status_reason = update.status_reason.clone();
                    Ok(updated)
                });

            // Notification for failure
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Try to enqueue next pending
            mocks
                .tx_repo
                .expect_find_by_status_paginated()
                .returning(move |_, _, _, _| {
                    Ok(PaginatedResult {
                        items: vec![],
                        total: 0,
                        page: 1,
                        per_page: 1,
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let result = handler.handle_transaction_status_impl(tx, None).await;

            assert!(result.is_ok());
            let failed_tx = result.unwrap();
            assert_eq!(failed_tx.status, TransactionStatus::Failed);
            assert!(failed_tx
                .status_reason
                .as_ref()
                .unwrap()
                .contains("Failed to parse and validate hash"));
        }

        #[tokio::test]
        async fn test_submitted_exceeds_max_lifetime_marks_failed() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx = create_test_transaction(&relayer.id);
            tx.id = "tx-submitted-old".to_string();
            tx.status = TransactionStatus::Submitted;
            // Created more than max lifetime ago (16 minutes > 15 minutes)
            tx.created_at =
                (Utc::now() - get_stellar_max_stuck_transaction_lifetime() - Duration::minutes(1))
                    .to_rfc3339();
            // Set a hash so it can query provider
            let tx_hash_bytes = [6u8; 32];
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx.network_data {
                stellar_data.hash = Some(hex::encode(tx_hash_bytes));
            }

            let expected_stellar_hash = soroban_rs::xdr::Hash(tx_hash_bytes);

            // Mock provider to return PENDING status (not SUCCESS or FAILED)
            mocks
                .provider
                .expect_get_transaction()
                .with(eq(expected_stellar_hash.clone()))
                .times(1)
                .returning(move |_| {
                    Box::pin(async { Ok(dummy_get_transaction_response("PENDING")) })
                });

            // Should mark as Failed
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_id, update| update.status == Some(TransactionStatus::Failed))
                .times(1)
                .returning(|id, update| {
                    let mut updated = create_test_transaction("test");
                    updated.id = id;
                    updated.status = update.status.unwrap();
                    updated.status_reason = update.status_reason.clone();
                    Ok(updated)
                });

            // Notification for failure
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Try to enqueue next pending
            mocks
                .tx_repo
                .expect_find_by_status_paginated()
                .returning(move |_, _, _, _| {
                    Ok(PaginatedResult {
                        items: vec![],
                        total: 0,
                        page: 1,
                        per_page: 1,
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let result = handler.handle_transaction_status_impl(tx, None).await;

            assert!(result.is_ok());
            let failed_tx = result.unwrap();
            assert_eq!(failed_tx.status, TransactionStatus::Failed);
            assert!(failed_tx
                .status_reason
                .as_ref()
                .unwrap()
                .contains("stuck in Submitted status for too long"));
        }

        #[tokio::test]
        async fn test_submitted_expired_marks_expired() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx = create_test_transaction(&relayer.id);
            tx.id = "tx-submitted-expired".to_string();
            tx.status = TransactionStatus::Submitted;
            tx.created_at = (Utc::now() - Duration::minutes(10)).to_rfc3339();
            // Set valid_until to a past time (expired)
            tx.valid_until = Some((Utc::now() - Duration::minutes(5)).to_rfc3339());
            // Set a hash so it can query provider
            let tx_hash_bytes = [7u8; 32];
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx.network_data {
                stellar_data.hash = Some(hex::encode(tx_hash_bytes));
            }

            let expected_stellar_hash = soroban_rs::xdr::Hash(tx_hash_bytes);

            // Mock provider to return PENDING status (not SUCCESS or FAILED)
            mocks
                .provider
                .expect_get_transaction()
                .with(eq(expected_stellar_hash.clone()))
                .times(1)
                .returning(move |_| {
                    Box::pin(async { Ok(dummy_get_transaction_response("PENDING")) })
                });

            // Should mark as Expired
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_id, update| update.status == Some(TransactionStatus::Expired))
                .times(1)
                .returning(|id, update| {
                    let mut updated = create_test_transaction("test");
                    updated.id = id;
                    updated.status = update.status.unwrap();
                    updated.status_reason = update.status_reason.clone();
                    Ok(updated)
                });

            // Notification for expiration
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Try to enqueue next pending
            mocks
                .tx_repo
                .expect_find_by_status_paginated()
                .returning(move |_, _, _, _| {
                    Ok(PaginatedResult {
                        items: vec![],
                        total: 0,
                        page: 1,
                        per_page: 1,
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let result = handler.handle_transaction_status_impl(tx, None).await;

            assert!(result.is_ok());
            let expired_tx = result.unwrap();
            assert_eq!(expired_tx.status, TransactionStatus::Expired);
            assert!(expired_tx
                .status_reason
                .as_ref()
                .unwrap()
                .contains("expired"));
        }
    }

    mod is_valid_until_expired_tests {
        use super::*;
        use crate::{
            jobs::MockJobProducerTrait,
            repositories::{
                MockRelayerRepository, MockTransactionCounterTrait, MockTransactionRepository,
            },
            services::{
                provider::MockStellarProviderTrait, stellar_dex::MockStellarDexServiceTrait,
            },
        };
        use chrono::{Duration, Utc};

        // Type alias for testing static methods
        type TestHandler = StellarRelayerTransaction<
            MockRelayerRepository,
            MockTransactionRepository,
            MockJobProducerTrait,
            MockStellarCombinedSigner,
            MockStellarProviderTrait,
            MockTransactionCounterTrait,
            MockStellarDexServiceTrait,
        >;

        #[test]
        fn test_rfc3339_expired() {
            let past = (Utc::now() - Duration::hours(1)).to_rfc3339();
            assert!(TestHandler::is_valid_until_string_expired(&past));
        }

        #[test]
        fn test_rfc3339_not_expired() {
            let future = (Utc::now() + Duration::hours(1)).to_rfc3339();
            assert!(!TestHandler::is_valid_until_string_expired(&future));
        }

        #[test]
        fn test_numeric_timestamp_expired() {
            let past_timestamp = (Utc::now() - Duration::hours(1)).timestamp().to_string();
            assert!(TestHandler::is_valid_until_string_expired(&past_timestamp));
        }

        #[test]
        fn test_numeric_timestamp_not_expired() {
            let future_timestamp = (Utc::now() + Duration::hours(1)).timestamp().to_string();
            assert!(!TestHandler::is_valid_until_string_expired(
                &future_timestamp
            ));
        }

        #[test]
        fn test_zero_timestamp_unbounded() {
            // Zero means unbounded in Stellar
            assert!(!TestHandler::is_valid_until_string_expired("0"));
        }

        #[test]
        fn test_invalid_format_not_expired() {
            // Invalid format should be treated as not expired (conservative)
            assert!(!TestHandler::is_valid_until_string_expired("not-a-date"));
        }
    }

    // Tests for circuit breaker functionality
    mod circuit_breaker_tests {
        use super::*;
        use crate::jobs::StatusCheckContext;
        use crate::models::NetworkType;

        /// Helper to create a context that should trigger the circuit breaker
        fn create_triggered_context() -> StatusCheckContext {
            StatusCheckContext::new(
                110, // consecutive_failures: exceeds Stellar threshold of 100
                150, // total_failures
                160, // total_retries
                100, // max_consecutive_failures (Stellar default)
                300, // max_total_failures (Stellar default)
                NetworkType::Stellar,
            )
        }

        /// Helper to create a context that should NOT trigger the circuit breaker
        fn create_safe_context() -> StatusCheckContext {
            StatusCheckContext::new(
                10,  // consecutive_failures: below threshold
                20,  // total_failures
                25,  // total_retries
                100, // max_consecutive_failures
                300, // max_total_failures
                NetworkType::Stellar,
            )
        }

        /// Helper to create a context that triggers via total failures (safety net)
        fn create_total_triggered_context() -> StatusCheckContext {
            StatusCheckContext::new(
                20,  // consecutive_failures: below threshold
                310, // total_failures: exceeds Stellar threshold of 300
                350, // total_retries
                100, // max_consecutive_failures
                300, // max_total_failures
                NetworkType::Stellar,
            )
        }

        #[tokio::test]
        async fn test_circuit_breaker_submitted_marks_as_failed() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.status = TransactionStatus::Submitted;
            tx_to_handle.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();

            // Expect partial_update to be called with Failed status
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, update| update.status == Some(TransactionStatus::Failed))
                .times(1)
                .returning(|_, update| {
                    let mut updated_tx = create_test_transaction("test-relayer");
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    updated_tx.status_reason = update.status_reason.clone();
                    Ok(updated_tx)
                });

            // Mock notification
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Try to enqueue next pending (called after lane cleanup)
            mocks
                .tx_repo
                .expect_find_by_status_paginated()
                .returning(|_, _, _, _| {
                    Ok(PaginatedResult {
                        items: vec![],
                        total: 0,
                        page: 1,
                        per_page: 1,
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let ctx = create_triggered_context();

            let result = handler
                .handle_transaction_status_impl(tx_to_handle, Some(ctx))
                .await;

            assert!(result.is_ok());
            let tx = result.unwrap();
            assert_eq!(tx.status, TransactionStatus::Failed);
            assert!(tx.status_reason.is_some());
            assert!(tx.status_reason.unwrap().contains("consecutive errors"));
        }

        #[tokio::test]
        async fn test_circuit_breaker_pending_marks_as_failed() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.status = TransactionStatus::Pending;
            tx_to_handle.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();

            // Expect partial_update to be called with Failed status
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, update| update.status == Some(TransactionStatus::Failed))
                .times(1)
                .returning(|_, update| {
                    let mut updated_tx = create_test_transaction("test-relayer");
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    updated_tx.status_reason = update.status_reason.clone();
                    Ok(updated_tx)
                });

            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            mocks
                .tx_repo
                .expect_find_by_status_paginated()
                .returning(|_, _, _, _| {
                    Ok(PaginatedResult {
                        items: vec![],
                        total: 0,
                        page: 1,
                        per_page: 1,
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let ctx = create_triggered_context();

            let result = handler
                .handle_transaction_status_impl(tx_to_handle, Some(ctx))
                .await;

            assert!(result.is_ok());
            let tx = result.unwrap();
            assert_eq!(tx.status, TransactionStatus::Failed);
        }

        #[tokio::test]
        async fn test_circuit_breaker_total_failures_triggers() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.status = TransactionStatus::Submitted;
            tx_to_handle.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();

            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, update| update.status == Some(TransactionStatus::Failed))
                .times(1)
                .returning(|_, update| {
                    let mut updated_tx = create_test_transaction("test-relayer");
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    updated_tx.status_reason = update.status_reason.clone();
                    Ok(updated_tx)
                });

            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            mocks
                .tx_repo
                .expect_find_by_status_paginated()
                .returning(|_, _, _, _| {
                    Ok(PaginatedResult {
                        items: vec![],
                        total: 0,
                        page: 1,
                        per_page: 1,
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            // Use context that triggers via total failures (safety net)
            let ctx = create_total_triggered_context();

            let result = handler
                .handle_transaction_status_impl(tx_to_handle, Some(ctx))
                .await;

            assert!(result.is_ok());
            let tx = result.unwrap();
            assert_eq!(tx.status, TransactionStatus::Failed);
        }

        #[tokio::test]
        async fn test_circuit_breaker_below_threshold_continues() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.status = TransactionStatus::Submitted;
            tx_to_handle.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
            let tx_hash_bytes = [1u8; 32];
            let tx_hash_hex = hex::encode(tx_hash_bytes);
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx_to_handle.network_data
            {
                stellar_data.hash = Some(tx_hash_hex.clone());
            }

            // Below threshold, should continue with normal status checking
            mocks
                .provider
                .expect_get_transaction()
                .returning(|_| Box::pin(async { Ok(dummy_get_transaction_response("SUCCESS")) }));

            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = create_test_transaction("test-relayer");
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            mocks
                .tx_repo
                .expect_find_by_status_paginated()
                .returning(|_, _, _, _| {
                    Ok(PaginatedResult {
                        items: vec![],
                        total: 0,
                        page: 1,
                        per_page: 1,
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let ctx = create_safe_context();

            let result = handler
                .handle_transaction_status_impl(tx_to_handle, Some(ctx))
                .await;

            assert!(result.is_ok());
            let tx = result.unwrap();
            // Should become Confirmed (normal flow), not Failed (circuit breaker)
            assert_eq!(tx.status, TransactionStatus::Confirmed);
        }

        #[tokio::test]
        async fn test_circuit_breaker_final_state_early_return() {
            let relayer = create_test_relayer();
            let mocks = default_test_mocks();

            // Transaction is already in final state
            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.status = TransactionStatus::Confirmed;

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let ctx = create_triggered_context();

            // Even with triggered context, final states should return early
            let result = handler
                .handle_transaction_status_impl(tx_to_handle.clone(), Some(ctx))
                .await;

            assert!(result.is_ok());
            assert_eq!(result.unwrap().id, tx_to_handle.id);
        }

        #[tokio::test]
        async fn test_circuit_breaker_no_context_continues() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.status = TransactionStatus::Submitted;
            tx_to_handle.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
            let tx_hash_bytes = [1u8; 32];
            let tx_hash_hex = hex::encode(tx_hash_bytes);
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx_to_handle.network_data
            {
                stellar_data.hash = Some(tx_hash_hex.clone());
            }

            // No context means no circuit breaker
            mocks
                .provider
                .expect_get_transaction()
                .returning(|_| Box::pin(async { Ok(dummy_get_transaction_response("SUCCESS")) }));

            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = create_test_transaction("test-relayer");
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            mocks
                .tx_repo
                .expect_find_by_status_paginated()
                .returning(|_, _, _, _| {
                    Ok(PaginatedResult {
                        items: vec![],
                        total: 0,
                        page: 1,
                        per_page: 1,
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);

            // Pass None for context - should continue normally
            let result = handler
                .handle_transaction_status_impl(tx_to_handle, None)
                .await;

            assert!(result.is_ok());
            let tx = result.unwrap();
            assert_eq!(tx.status, TransactionStatus::Confirmed);
        }
    }

    mod failure_detail_helper_tests {
        use super::*;
        use soroban_rs::xdr::{InvokeHostFunctionResult, OperationResult, OperationResultTr, VecM};

        #[test]
        fn first_failing_op_finds_trapped() {
            let ops: VecM<OperationResult> = vec![OperationResult::OpInner(
                OperationResultTr::InvokeHostFunction(InvokeHostFunctionResult::Trapped),
            )]
            .try_into()
            .unwrap();
            assert_eq!(first_failing_op(ops.as_slice()), Some("Trapped"));
        }

        #[test]
        fn first_failing_op_skips_success() {
            let ops: VecM<OperationResult> = vec![
                OperationResult::OpInner(OperationResultTr::InvokeHostFunction(
                    InvokeHostFunctionResult::Success(soroban_rs::xdr::Hash([0u8; 32])),
                )),
                OperationResult::OpInner(OperationResultTr::InvokeHostFunction(
                    InvokeHostFunctionResult::ResourceLimitExceeded,
                )),
            ]
            .try_into()
            .unwrap();
            assert_eq!(
                first_failing_op(ops.as_slice()),
                Some("ResourceLimitExceeded")
            );
        }

        #[test]
        fn first_failing_op_all_success_returns_none() {
            let ops: VecM<OperationResult> = vec![OperationResult::OpInner(
                OperationResultTr::InvokeHostFunction(InvokeHostFunctionResult::Success(
                    soroban_rs::xdr::Hash([0u8; 32]),
                )),
            )]
            .try_into()
            .unwrap();
            assert_eq!(first_failing_op(ops.as_slice()), None);
        }

        #[test]
        fn first_failing_op_empty_returns_none() {
            assert_eq!(first_failing_op(&[]), None);
        }

        #[test]
        fn first_failing_op_op_bad_auth() {
            let ops: VecM<OperationResult> = vec![OperationResult::OpBadAuth].try_into().unwrap();
            assert_eq!(first_failing_op(ops.as_slice()), Some("OpBadAuth"));
        }
    }
}
