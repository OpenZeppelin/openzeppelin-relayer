//! This module contains the submission-related functionality for Stellar transactions.
//! It includes methods for submitting transactions with robust error handling,
//! ensuring proper transaction state management on failure.

use chrono::Utc;
use tracing::{debug, info, warn};

use super::{
    is_final_state,
    utils::{decode_transaction_result_code, is_bad_sequence_error, is_insufficient_fee_error},
    StellarRelayerTransaction,
};
use crate::{
    constants::{
        STELLAR_FAST_RESUBMIT_BASE_DELAY_SECONDS, STELLAR_INSUFFICIENT_FEE_MAX_RETRIES,
        STELLAR_TRY_AGAIN_LATER_FAST_RETRIES,
    },
    domain::transaction::stellar::prepare::common::send_submit_transaction_job,
    jobs::JobProducerTrait,
    metrics::{STELLAR_SUBMISSION_FAILURES, TRANSACTIONS_INSUFFICIENT_FEE},
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
    /// Main submission method with robust error handling.
    /// Unlike prepare, submit doesn't claim lanes but still needs proper error handling.
    pub async fn submit_transaction_impl(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!(
            tx_id = %tx.id,
            relayer_id = %tx.relayer_id,
            status = ?tx.status,
            "submitting stellar transaction"
        );

        // Defensive check: if transaction is in a final state or unexpected state, don't retry
        if is_final_state(&tx.status) {
            warn!(
                tx_id = %tx.id,
                relayer_id = %tx.relayer_id,
                status = ?tx.status,
                "transaction already in final state, skipping submission"
            );
            return Ok(tx);
        }

        // Check if transaction has expired before attempting submission
        if self.is_transaction_expired(&tx)? {
            info!(
                tx_id = %tx.id,
                relayer_id = %tx.relayer_id,
                valid_until = ?tx.valid_until,
                "transaction has expired, marking as Expired"
            );
            return self
                .mark_as_expired(tx, "Transaction time_bounds expired".to_string())
                .await;
        }

        // Call core submission logic with error handling
        match self.submit_core(tx.clone()).await {
            Ok(submitted_tx) => Ok(submitted_tx),
            Err(error) => {
                // Handle submission failure - mark as failed and send notification
                self.handle_submit_failure(tx, error).await
            }
        }
    }

    /// Core submission logic - pure business logic without error handling concerns.
    ///
    /// Uses `send_transaction_with_status` to get full status information from the RPC.
    /// Handles status codes:
    /// - PENDING: Transaction accepted for processing
    /// - DUPLICATE: Transaction already submitted (treat as success)
    /// - TRY_AGAIN_LATER: Network congested but tx is valid; update sent_at and schedule a
    ///   fast delayed resubmit (5s × attempt) for the first 3 rejections
    /// - ERROR: Transaction validation failed, except insufficient fee schedules the same fast
    ///   delayed resubmit through the insufficient-fee retry cap (6)
    ///
    /// The status checker remains the fallback rescue. Each fast attempt refreshes sent_at before
    /// enqueueing, so the ladder's ≥10s time-since-last-submit gate stays closed while the
    /// 5s-spaced fast attempts are active.
    async fn submit_core(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let stellar_data = tx.network_data.get_stellar_transaction_data()?;
        let tx_envelope = stellar_data
            .get_envelope_for_submission()
            .map_err(TransactionError::from)?;

        // Use send_transaction_with_status to get full status information
        let response = self
            .provider()
            .send_transaction_with_status(&tx_envelope)
            .await
            .map_err(|e| {
                STELLAR_SUBMISSION_FAILURES
                    .with_label_values(&["provider_error", "n/a"])
                    .inc();
                TransactionError::from(e)
            })?;

        // Handle status codes from the RPC response
        match response.status.as_str() {
            "PENDING" | "DUPLICATE" => {
                // Success - transaction is accepted or already exists
                if response.status == "DUPLICATE" {
                    info!(
                        tx_id = %tx.id,
                        relayer_id = %tx.relayer_id,
                        hash = %response.hash,
                        "transaction already submitted (DUPLICATE status)"
                    );
                }
                let tx_hash_hex = response.hash.clone();
                let updated_stellar_data = stellar_data.with_hash(tx_hash_hex.clone());

                let mut hashes = tx.hashes.clone();
                if !hashes.contains(&tx_hash_hex) {
                    hashes.push(tx_hash_hex);
                }

                let update_req = TransactionUpdateRequest {
                    status: Some(TransactionStatus::Submitted),
                    sent_at: Some(Utc::now().to_rfc3339()),
                    network_data: Some(NetworkTransactionData::Stellar(updated_stellar_data)),
                    hashes: Some(hashes),
                    ..Default::default()
                };

                let updated_tx = self
                    .transaction_repository()
                    .partial_update(tx.id.clone(), update_req)
                    .await?;

                // Send notification for newly submitted transaction
                if response.status == "PENDING" {
                    info!(
                        tx_id = %tx.id,
                        relayer_id = %tx.relayer_id,
                        "sending transaction update notification for pending transaction"
                    );
                    self.send_transaction_update_notification(&updated_tx).await;
                }

                Ok(updated_tx)
            }
            "TRY_AGAIN_LATER" => {
                // Network is temporarily congested — the transaction is valid but the
                // node's queue is full. Atomically update sent_at and increment
                // try_again_later_retries so the status checker's backoff gate measures
                // time since this attempt. Return Ok to keep the transaction alive;
                // the fast path handles the bounded early retries, and the status
                // checker remains the fallback rescue.
                let updated_tx = self
                    .transaction_repository()
                    .record_stellar_try_again_later_retry(tx.id.clone(), Utc::now().to_rfc3339())
                    .await?;

                // The repo returns the transaction unchanged if it reached a final state
                // between this submission and the record call (e.g. a racing job finalized
                // it). Nothing left to rescue — skip metrics and scheduling.
                if is_final_state(&updated_tx.status) {
                    debug!(
                        tx_id = %updated_tx.id,
                        relayer_id = %updated_tx.relayer_id,
                        status = ?updated_tx.status,
                        "transaction reached final state during retry recording; skipping fast resubmit"
                    );
                    return Ok(updated_tx);
                }

                let retries = updated_tx
                    .metadata
                    .as_ref()
                    .map_or(0, |m| m.try_again_later_retries);

                // Only push on first encounter (dedup: won't fire on retry 2, 3, etc.)
                if retries == 1 {
                    crate::metrics::STELLAR_TRY_AGAIN_LATER
                        .with_label_values(&[&tx.relayer_id, &tx.status.to_string()])
                        .inc();
                }

                if retries <= STELLAR_TRY_AGAIN_LATER_FAST_RETRIES {
                    let delay_seconds =
                        STELLAR_FAST_RESUBMIT_BASE_DELAY_SECONDS * i64::from(retries);

                    info!(
                        tx_id = %updated_tx.id,
                        relayer_id = %updated_tx.relayer_id,
                        status = ?updated_tx.status,
                        try_again_later_retries = retries,
                        delay_seconds,
                        "enqueueing fast resubmit after TRY_AGAIN_LATER"
                    );

                    if let Err(error) = send_submit_transaction_job(
                        self.job_producer(),
                        &updated_tx,
                        Some(delay_seconds),
                    )
                    .await
                    {
                        warn!(
                            tx_id = %updated_tx.id,
                            relayer_id = %updated_tx.relayer_id,
                            error = %error,
                            try_again_later_retries = retries,
                            delay_seconds,
                            "failed to enqueue fast resubmit after TRY_AGAIN_LATER"
                        );
                    }
                } else {
                    debug!(
                        tx_id = %tx.id,
                        relayer_id = %tx.relayer_id,
                        status = ?tx.status,
                        try_again_later_retries = retries,
                        "TRY_AGAIN_LATER — status checker will retry"
                    );
                }
                Ok(updated_tx)
            }
            "ERROR" => {
                // Transaction validation failed
                let error_detail = response
                    .error_result_xdr
                    .unwrap_or_else(|| "No error details provided".to_string());
                let decoded_result_code = decode_transaction_result_code(&error_detail);

                // Insufficient fee is a transient condition (network fee spike).
                // Treat like TRY_AGAIN_LATER: update sent_at and let the status
                // checker retry with exponential backoff.
                if decoded_result_code
                    .as_deref()
                    .is_some_and(is_insufficient_fee_error)
                {
                    let mut meta = tx.metadata.clone().unwrap_or_default();
                    meta.insufficient_fee_retries = meta.insufficient_fee_retries.saturating_add(1);

                    // Only push on first encounter (dedup: won't fire on retry 2, 3, etc.)
                    if meta.insufficient_fee_retries == 1 {
                        TRANSACTIONS_INSUFFICIENT_FEE
                            .with_label_values(&[tx.relayer_id.as_str(), "stellar"])
                            .inc();
                    }

                    if meta.insufficient_fee_retries > STELLAR_INSUFFICIENT_FEE_MAX_RETRIES {
                        STELLAR_SUBMISSION_FAILURES
                            .with_label_values(&["error", "tx_insufficient_fee"])
                            .inc();
                        return Err(TransactionError::UnexpectedError(format!(
                            "Transaction submission error: insufficient fee retry limit exceeded ({STELLAR_INSUFFICIENT_FEE_MAX_RETRIES})"
                        )));
                    }

                    // Atomically sets `sent_at` and increments Stellar insufficient-fee retries.
                    let updated_tx = self
                        .transaction_repository()
                        .record_stellar_insufficient_fee_retry(
                            tx.id.clone(),
                            Utc::now().to_rfc3339(),
                        )
                        .await?;

                    // The repo returns the transaction unchanged if it reached a final state
                    // between this submission and the record call (e.g. a racing job finalized
                    // it). Nothing left to rescue — skip metrics and scheduling.
                    if is_final_state(&updated_tx.status) {
                        debug!(
                            tx_id = %updated_tx.id,
                            relayer_id = %updated_tx.relayer_id,
                            status = ?updated_tx.status,
                            "transaction reached final state during retry recording; skipping fast resubmit"
                        );
                        return Ok(updated_tx);
                    }

                    let retries = updated_tx
                        .metadata
                        .as_ref()
                        .map_or(0, |metadata| metadata.insufficient_fee_retries);

                    // This can only happen when a concurrent submission increments the counter
                    // between our read and record. That winner already scheduled the retry; skip
                    // this enqueue so the next rejection terminates normally at the pre-record cap.
                    if retries > STELLAR_INSUFFICIENT_FEE_MAX_RETRIES {
                        warn!(
                            tx_id = %updated_tx.id,
                            relayer_id = %updated_tx.relayer_id,
                            insufficient_fee_retries = retries,
                            "post-record retry count exceeds cap; skipping fast resubmit - concurrent submission detected"
                        );
                        return Ok(updated_tx);
                    }

                    let delay_seconds =
                        STELLAR_FAST_RESUBMIT_BASE_DELAY_SECONDS * i64::from(retries);

                    info!(
                        tx_id = %updated_tx.id,
                        relayer_id = %updated_tx.relayer_id,
                        status = ?updated_tx.status,
                        insufficient_fee_retries = retries,
                        delay_seconds,
                        result_code = decoded_result_code.as_deref().unwrap_or("Unknown"),
                        "enqueueing fast resubmit after insufficient fee"
                    );

                    if let Err(error) = send_submit_transaction_job(
                        self.job_producer(),
                        &updated_tx,
                        Some(delay_seconds),
                    )
                    .await
                    {
                        warn!(
                            tx_id = %updated_tx.id,
                            relayer_id = %updated_tx.relayer_id,
                            error = %error,
                            insufficient_fee_retries = retries,
                            delay_seconds,
                            "failed to enqueue fast resubmit after insufficient fee"
                        );
                    }
                    return Ok(updated_tx);
                }
                STELLAR_SUBMISSION_FAILURES
                    .with_label_values(&[
                        "error",
                        decoded_result_code.as_deref().unwrap_or("unknown"),
                    ])
                    .inc();
                Err(TransactionError::UnexpectedError(format!(
                    "Transaction submission error: {}",
                    decoded_result_code.unwrap_or(error_detail)
                )))
            }
            unknown => {
                // Unknown status - treat as error
                STELLAR_SUBMISSION_FAILURES
                    .with_label_values(&["unknown_status", "n/a"])
                    .inc();
                warn!(
                    tx_id = %tx.id,
                    relayer_id = %tx.relayer_id,
                    status = %unknown,
                    "received unknown transaction status from RPC"
                );
                Err(TransactionError::UnexpectedError(format!(
                    "Unknown transaction status: {unknown}"
                )))
            }
        }
    }

    /// Handles submission failures with comprehensive cleanup and error reporting.
    /// For bad sequence errors, resets the transaction and re-enqueues it for retry.
    async fn handle_submit_failure(
        &self,
        tx: TransactionRepoModel,
        error: TransactionError,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let error_reason = format!("Submission failed: {error}");
        let tx_id = tx.id.clone();
        let relayer_id = tx.relayer_id.clone();
        warn!(
            tx_id = %tx_id,
            relayer_id = %relayer_id,
            reason = %error_reason,
            "transaction submission failed"
        );

        // CAS conflict in the submission path only occurs after the RPC
        // already accepted the transaction (PENDING status update raced).
        // The on-chain state is valid; reload the latest DB state and return
        // Ok — the status checker will reconcile on its next poll.
        if error.is_concurrent_update_conflict() {
            info!(
                tx_id = %tx_id,
                relayer_id = %relayer_id,
                "concurrent transaction update detected during submission, reloading latest state"
            );
            return self
                .transaction_repository()
                .get_by_id(tx_id)
                .await
                .map_err(TransactionError::from);
        }

        if is_bad_sequence_error(&error_reason) {
            // Reset the transaction to pending state BEFORE syncing the counter: the
            // reset clears this tx's (bad) sequence_number, so the drift rewind inside
            // sync_sequence_from_chain is not bounded by the failing tx's own record —
            // otherwise the tx would pin the counter one past its burned sequence and
            // the drift would never heal.
            // Status check will handle resubmission when it detects a pending transaction without hash
            info!(
                tx_id = %tx_id,
                relayer_id = %relayer_id,
                "bad sequence error detected, resetting transaction to pending state"
            );
            let reset_result = self.reset_transaction_for_retry(tx.clone()).await;

            // Sync sequence from chain (best effort)
            if let Ok(stellar_data) = tx.network_data.get_stellar_transaction_data() {
                info!(
                    tx_id = %tx_id,
                    relayer_id = %relayer_id,
                    "syncing sequence from chain after bad sequence error"
                );
                match self
                    .sync_sequence_from_chain(&stellar_data.source_account)
                    .await
                {
                    Ok(()) => {
                        info!(
                            tx_id = %tx_id,
                            relayer_id = %relayer_id,
                            "successfully synced sequence from chain"
                        );
                    }
                    Err(sync_error) => {
                        warn!(
                            tx_id = %tx_id,
                            relayer_id = %relayer_id,
                            error = %sync_error,
                            "failed to sync sequence from chain"
                        );
                    }
                }
            }

            match reset_result {
                Ok(reset_tx) => {
                    info!(
                        tx_id = %tx_id,
                        relayer_id = %relayer_id,
                        "transaction reset to pending, status check will handle resubmission"
                    );
                    // Return success since we've reset the transaction
                    // Status check job (scheduled with delay) will detect pending without hash
                    // and schedule a recovery job to go through the pipeline again
                    return Ok(reset_tx);
                }
                Err(reset_error) => {
                    warn!(
                        tx_id = %tx_id,
                        relayer_id = %relayer_id,
                        error = %reset_error,
                        "failed to reset transaction for retry"
                    );
                    // Fall through to normal failure handling
                }
            }
        }

        // For non-bad-sequence errors or if reset failed, mark as failed
        // Step 1: Mark transaction as Failed with detailed reason
        let update_request = TransactionUpdateRequest {
            status: Some(TransactionStatus::Failed),
            status_reason: Some(error_reason.clone()),
            ..Default::default()
        };
        let failed_tx = match self
            .finalize_transaction_state(tx_id.clone(), update_request)
            .await
        {
            Ok(updated_tx) => updated_tx,
            Err(finalize_error) => {
                warn!(
                    tx_id = %tx_id,
                    relayer_id = %relayer_id,
                    error = %finalize_error,
                    "failed to mark transaction as failed, continuing with lane cleanup"
                );
                // Finalization failed — propagate error so the queue retries
                // and the next attempt will either finalize or hit is_final_state
                return Err(error);
            }
        };

        // Attempt to enqueue next pending transaction or release lane
        if let Err(enqueue_error) = self.enqueue_next_pending_transaction(&tx_id).await {
            warn!(
                tx_id = %tx_id,
                relayer_id = %relayer_id,
                error = %enqueue_error,
                "failed to enqueue next pending transaction after submission failure"
            );
        }

        info!(
            tx_id = %tx_id,
            relayer_id = %relayer_id,
            error = %error_reason,
            "transaction submission failure handled, marked as failed"
        );

        // Transaction successfully marked as failed — return Ok to avoid
        // a pointless queue retry (the defensive is_final_state check at the
        // top of submit_transaction_impl would short-circuit anyway).
        Ok(failed_tx)
    }

    /// Resubmit transaction - delegates to submit_transaction_impl
    pub async fn resubmit_transaction_impl(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        self.submit_transaction_impl(tx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_rs::stellar_rpc_client::SendTransactionResponse;
    use soroban_rs::xdr::WriteXdr;

    use crate::domain::transaction::stellar::test_helpers::*;
    use crate::models::TransactionMetadata;

    /// Helper to create a SendTransactionResponse with given status
    fn create_send_tx_response(status: &str, hash: &str) -> SendTransactionResponse {
        SendTransactionResponse {
            status: status.to_string(),
            hash: hash.to_string(),
            error_result_xdr: None,
            latest_ledger: 100,
            latest_ledger_close_time: 1700000000,
        }
    }

    mod submit_transaction_tests {
        use crate::{
            models::RepositoryError, repositories::PaginatedResult,
            services::provider::ProviderError,
        };

        use super::*;

        #[tokio::test]
        async fn submit_transaction_happy_path() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // provider returns PENDING status
            let response = create_send_tx_response(
                "PENDING",
                "0101010101010101010101010101010101010101010101010101010101010101",
            );
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(move |_| {
                    let r = response.clone();
                    Box::pin(async move { Ok(r) })
                });

            // expect partial update to Submitted
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, upd| upd.status == Some(TransactionStatus::Submitted))
                .returning(|id, upd| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = upd.status.unwrap();
                    Ok::<_, RepositoryError>(tx)
                });

            // Expect notification
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);

            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent; // Must be Sent for idempotent submit
            if let NetworkTransactionData::Stellar(ref mut d) = tx.network_data {
                d.signatures.push(dummy_signature());
                d.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
                // Valid XDR
            }

            let res = handler.submit_transaction_impl(tx).await.unwrap();
            assert_eq!(res.status, TransactionStatus::Submitted);
        }

        #[tokio::test]
        async fn submit_transaction_provider_error_marks_failed() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Provider fails with non-bad-sequence error
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(|_| {
                    Box::pin(async { Err(ProviderError::Other("Network error".to_string())) })
                });

            // Mock finalize_transaction_state for failure handling
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, upd| upd.status == Some(TransactionStatus::Failed))
                .returning(|id, upd| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = upd.status.unwrap();
                    Ok::<_, RepositoryError>(tx)
                });

            // Mock notification for failed transaction
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Mock find_by_status_paginated for enqueue_next_pending_transaction
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
                }); // No pending transactions

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent; // Must be Sent for idempotent submit
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.sequence_number = Some(42); // Set sequence number
                data.signed_envelope_xdr = Some("test-xdr".to_string()); // Required for submission
            }

            let res = handler.submit_transaction_impl(tx).await;

            // Transaction is marked as failed and returned as Ok (no queue retry needed)
            let failed_tx = res.unwrap();
            assert_eq!(failed_tx.status, TransactionStatus::Failed);
        }

        #[tokio::test]
        async fn submit_transaction_repository_error_marks_failed() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Provider returns PENDING status
            let response = create_send_tx_response(
                "PENDING",
                "0101010101010101010101010101010101010101010101010101010101010101",
            );
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(move |_| {
                    let r = response.clone();
                    Box::pin(async move { Ok(r) })
                });

            // Repository fails on first update (submission)
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, upd| upd.status == Some(TransactionStatus::Submitted))
                .returning(|_, _| Err(RepositoryError::Unknown("Database error".to_string())));

            // Mock finalize_transaction_state for failure handling
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, upd| upd.status == Some(TransactionStatus::Failed))
                .returning(|id, upd| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = upd.status.unwrap();
                    Ok::<_, RepositoryError>(tx)
                });

            // Mock notification for failed transaction
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Mock find_by_status_paginated for enqueue_next_pending_transaction
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
                }); // No pending transactions

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent; // Must be Sent for idempotent submit
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.sequence_number = Some(42); // Set sequence number
                data.signed_envelope_xdr = Some("test-xdr".to_string()); // Required for submission
            }

            let res = handler.submit_transaction_impl(tx).await;

            // Even though provider succeeded and repo failed on Submitted update,
            // the failure handler marks the tx as Failed and returns Ok
            let failed_tx = res.unwrap();
            assert_eq!(failed_tx.status, TransactionStatus::Failed);
        }

        #[tokio::test]
        async fn submit_transaction_uses_signed_envelope_xdr() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Create a transaction with signed_envelope_xdr set
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent; // Must be Sent for idempotent submit
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                // Build and store the signed envelope XDR
                let envelope = data.get_envelope_for_submission().unwrap();
                let xdr = envelope
                    .to_xdr_base64(soroban_rs::xdr::Limits::none())
                    .unwrap();
                data.signed_envelope_xdr = Some(xdr);
            }

            // Provider should receive the envelope decoded from signed_envelope_xdr
            let response = create_send_tx_response(
                "PENDING",
                "0202020202020202020202020202020202020202020202020202020202020202",
            );
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(move |_| {
                    let r = response.clone();
                    Box::pin(async move { Ok(r) })
                });

            // Update to Submitted
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, upd| upd.status == Some(TransactionStatus::Submitted))
                .returning(|id, upd| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = upd.status.unwrap();
                    Ok::<_, RepositoryError>(tx)
                });

            // Expect notification
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let res = handler.submit_transaction_impl(tx).await.unwrap();

            assert_eq!(res.status, TransactionStatus::Submitted);
        }

        #[tokio::test]
        async fn resubmit_transaction_delegates_to_submit() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // provider returns PENDING status
            let response = create_send_tx_response(
                "PENDING",
                "0101010101010101010101010101010101010101010101010101010101010101",
            );
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(move |_| {
                    let r = response.clone();
                    Box::pin(async move { Ok(r) })
                });

            // expect partial update to Submitted
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, upd| upd.status == Some(TransactionStatus::Submitted))
                .returning(|id, upd| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = upd.status.unwrap();
                    Ok::<_, RepositoryError>(tx)
                });

            // Expect notification
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);

            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent; // Must be Sent for idempotent submit
            if let NetworkTransactionData::Stellar(ref mut d) = tx.network_data {
                d.signatures.push(dummy_signature());
                d.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
                // Valid XDR
            }

            let res = handler.resubmit_transaction_impl(tx).await.unwrap();
            assert_eq!(res.status, TransactionStatus::Submitted);
        }

        #[tokio::test]
        async fn submit_transaction_failure_enqueues_next_transaction() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Provider fails with non-bad-sequence error
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(|_| {
                    Box::pin(async { Err(ProviderError::Other("Network error".to_string())) })
                });

            // No sync expected for non-bad-sequence errors

            // Mock finalize_transaction_state for failure handling
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, upd| upd.status == Some(TransactionStatus::Failed))
                .returning(|id, upd| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = upd.status.unwrap();
                    Ok::<_, RepositoryError>(tx)
                });

            // Mock notification for failed transaction
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Mock find_by_status to return a pending transaction
            let mut pending_tx = create_test_transaction(&relayer.id);
            pending_tx.id = "next-pending-tx".to_string();
            pending_tx.status = TransactionStatus::Pending;
            let captured_pending_tx = pending_tx.clone();
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
                        items: vec![captured_pending_tx.clone()],
                        total: 1,
                        page: 1,
                        per_page: 1,
                    })
                });

            // Mock produce_transaction_request_job for the next pending transaction
            mocks
                .job_producer
                .expect_produce_transaction_request_job()
                .withf(move |job, _delay| job.transaction_id == "next-pending-tx")
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent; // Must be Sent for idempotent submit
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.sequence_number = Some(42); // Set sequence number
                data.signed_envelope_xdr = Some("test-xdr".to_string()); // Required for submission
            }

            let res = handler.submit_transaction_impl(tx).await;

            // Transaction marked as failed and next transaction enqueued
            let failed_tx = res.unwrap();
            assert_eq!(failed_tx.status, TransactionStatus::Failed);
        }

        #[tokio::test]
        async fn test_submit_bad_sequence_resets_and_retries() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock provider to return bad sequence error
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(|_| {
                    Box::pin(async {
                        Err(ProviderError::Other(
                            "transaction submission failed: TxBadSeq".to_string(),
                        ))
                    })
                });

            // Mock get_account for sync_sequence_from_chain
            mocks.provider.expect_get_account().times(1).returning(|_| {
                Box::pin(async {
                    use soroban_rs::xdr::{
                        AccountEntry, AccountEntryExt, AccountId, PublicKey, SequenceNumber,
                        String32, Thresholds, Uint256,
                    };
                    use stellar_strkey::ed25519;

                    let pk = ed25519::PublicKey::from_string(TEST_PK).unwrap();
                    let account_id = AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(pk.0)));

                    Ok(AccountEntry {
                        account_id,
                        balance: 1000000,
                        seq_num: SequenceNumber(100),
                        num_sub_entries: 0,
                        inflation_dest: None,
                        flags: 0,
                        home_domain: String32::default(),
                        thresholds: Thresholds([1, 1, 1, 1]),
                        signers: Default::default(),
                        ext: AccountEntryExt::V0,
                    })
                })
            });

            // Mock counter sync_floor for sync_sequence_from_chain
            mocks
                .counter
                .expect_sync_floor()
                .times(1)
                .returning(|_, _, floor| Box::pin(async move { Ok(floor) }));

            // Mock partial_update for reset_transaction_for_retry - should reset to Pending
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, upd| upd.status == Some(TransactionStatus::Pending))
                .times(1)
                .returning(|id, upd| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = upd.status.unwrap();
                    if let Some(network_data) = upd.network_data {
                        tx.network_data = network_data;
                    }
                    Ok::<_, RepositoryError>(tx)
                });

            // Note: Status check will handle resubmission when it detects a pending transaction without hash
            // We don't schedule the job here - it will be scheduled by status check when the transaction is old enough

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent; // Must be Sent for idempotent submit
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.sequence_number = Some(42);
                data.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
                // Valid XDR
            }

            let result = handler.submit_transaction_impl(tx).await;

            // Should return Ok since we're handling the retry
            assert!(result.is_ok());
            let reset_tx = result.unwrap();
            assert_eq!(reset_tx.status, TransactionStatus::Pending);

            // Verify stellar data was reset
            if let NetworkTransactionData::Stellar(data) = &reset_tx.network_data {
                assert!(data.sequence_number.is_none());
                assert!(data.signatures.is_empty());
                assert!(data.hash.is_none());
                assert!(data.signed_envelope_xdr.is_none());
            } else {
                panic!("Expected Stellar transaction data");
            }
        }

        #[tokio::test]
        async fn test_submit_bad_sequence_rewinds_drifted_counter() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock provider to return bad sequence error
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(|_| {
                    Box::pin(async {
                        Err(ProviderError::Other(
                            "transaction submission failed: TxBadSeq".to_string(),
                        ))
                    })
                });

            // Mock get_account for sync_sequence_from_chain: chain seq 100 → next usable 101
            mocks.provider.expect_get_account().times(1).returning(|_| {
                Box::pin(async {
                    use soroban_rs::xdr::{
                        AccountEntry, AccountEntryExt, AccountId, PublicKey, SequenceNumber,
                        String32, Thresholds, Uint256,
                    };
                    use stellar_strkey::ed25519;

                    let pk = ed25519::PublicKey::from_string(TEST_PK).unwrap();
                    let account_id = AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(pk.0)));

                    Ok(AccountEntry {
                        account_id,
                        balance: 1000000,
                        seq_num: SequenceNumber(100),
                        num_sub_entries: 0,
                        inflation_dest: None,
                        flags: 0,
                        home_domain: String32::default(),
                        thresholds: Thresholds([1, 1, 1, 1]),
                        signers: Default::default(),
                        ext: AccountEntryExt::V0,
                    })
                })
            });

            // Counter drifted above the chain floor: sync_floor reports 150
            mocks
                .counter
                .expect_sync_floor()
                .times(1)
                .returning(|_, _, _| Box::pin(async { Ok(150) }));

            // The reset must run BEFORE the occupancy scan: the reset clears this tx's
            // burned sequence_number, so its record no longer bounds the rewind. The
            // Sequence pins that ordering — reverting to sync-before-reset fails here.
            let mut call_order = mockall::Sequence::new();

            // Mock partial_update for reset_transaction_for_retry - should reset to Pending
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, upd| upd.status == Some(TransactionStatus::Pending))
                .times(1)
                .in_sequence(&mut call_order)
                .returning(|id, upd| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = upd.status.unwrap();
                    if let Some(network_data) = upd.network_data {
                        tx.network_data = network_data;
                    }
                    Ok::<_, RepositoryError>(tx)
                });

            // The scan then sees the reset tx (sequence_number cleared) — target is
            // the chain floor
            let mut reset_tx = create_test_transaction(&relayer.id);
            reset_tx.status = TransactionStatus::Pending;
            if let NetworkTransactionData::Stellar(ref mut data) = reset_tx.network_data {
                data.sequence_number = None;
            }
            mocks
                .tx_repo
                .expect_find_by_status()
                .times(1)
                .in_sequence(&mut call_order)
                .returning(move |_, _| Ok(vec![reset_tx.clone()]));

            // No Confirmed txs to bound the rewind
            mocks
                .tx_repo
                .expect_find_by_status_paginated()
                .times(1)
                .returning(|_, _, _, _| {
                    Ok(PaginatedResult {
                        items: vec![],
                        total: 0,
                        page: 1,
                        per_page: 1,
                    })
                });

            // The drifted counter is rewound 150 → 101 via CAS
            mocks
                .counter
                .expect_set_if_equals()
                .withf(|relayer_id, addr, expected, value| {
                    relayer_id == "relayer-1"
                        && addr == TEST_PK
                        && *expected == 150
                        && *value == 101
                })
                .times(1)
                .returning(|_, _, _, _| Box::pin(async { Ok(true) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent; // Must be Sent for idempotent submit
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.sequence_number = Some(42);
                data.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
            }

            let result = handler.submit_transaction_impl(tx).await;

            // The tx is reset for retry and the counter rewind was applied
            assert!(result.is_ok());
            assert_eq!(result.unwrap().status, TransactionStatus::Pending);
        }

        #[tokio::test]
        async fn test_submit_bad_sequence_reset_failure_still_syncs_and_marks_failed() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock provider to return bad sequence error
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(|_| {
                    Box::pin(async {
                        Err(ProviderError::Other(
                            "transaction submission failed: TxBadSeq".to_string(),
                        ))
                    })
                });

            // Pin the flow: reset is attempted first, the chain sync still runs after
            // the reset fails, and the tx then falls through to being marked Failed.
            let mut call_order = mockall::Sequence::new();

            // Reset attempt fails
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, upd| upd.status == Some(TransactionStatus::Pending))
                .times(1)
                .in_sequence(&mut call_order)
                .returning(|_, _| Err(RepositoryError::Unknown("reset write failed".to_string())));

            // Sync still runs (best effort): chain seq 100 → floor 101, no drift
            mocks
                .provider
                .expect_get_account()
                .times(1)
                .in_sequence(&mut call_order)
                .returning(|_| {
                    Box::pin(async {
                        use soroban_rs::xdr::{
                            AccountEntry, AccountEntryExt, AccountId, PublicKey, SequenceNumber,
                            String32, Thresholds, Uint256,
                        };
                        use stellar_strkey::ed25519;

                        let pk = ed25519::PublicKey::from_string(TEST_PK).unwrap();
                        let account_id = AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(pk.0)));

                        Ok(AccountEntry {
                            account_id,
                            balance: 1000000,
                            seq_num: SequenceNumber(100),
                            num_sub_entries: 0,
                            inflation_dest: None,
                            flags: 0,
                            home_domain: String32::default(),
                            thresholds: Thresholds([1, 1, 1, 1]),
                            signers: Default::default(),
                            ext: AccountEntryExt::V0,
                        })
                    })
                });

            mocks
                .counter
                .expect_sync_floor()
                .times(1)
                .returning(|_, _, floor| Box::pin(async move { Ok(floor) }));

            // Fall-through marks the tx Failed
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, upd| upd.status == Some(TransactionStatus::Failed))
                .times(1)
                .in_sequence(&mut call_order)
                .returning(|id, upd| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = upd.status.unwrap();
                    tx.status_reason = upd.status_reason;
                    Ok::<_, RepositoryError>(tx)
                });

            // Notification for the failed transaction
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // No pending transactions to enqueue after the failure
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
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent; // Must be Sent for idempotent submit
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.sequence_number = Some(42);
                data.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
            }

            let result = handler.submit_transaction_impl(tx).await;

            // Marked as failed and returned Ok (no pointless queue retry)
            assert!(result.is_ok());
            assert_eq!(result.unwrap().status, TransactionStatus::Failed);
        }

        #[tokio::test]
        async fn submit_transaction_duplicate_status_succeeds() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Provider returns DUPLICATE status
            let response = create_send_tx_response(
                "DUPLICATE",
                "0101010101010101010101010101010101010101010101010101010101010101",
            );
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(move |_| {
                    let r = response.clone();
                    Box::pin(async move { Ok(r) })
                });

            // expect partial update to Submitted
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, upd| upd.status == Some(TransactionStatus::Submitted))
                .returning(|id, upd| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = upd.status.unwrap();
                    Ok::<_, RepositoryError>(tx)
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);

            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent;
            if let NetworkTransactionData::Stellar(ref mut d) = tx.network_data {
                d.signatures.push(dummy_signature());
                d.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
            }

            let res = handler.submit_transaction_impl(tx).await.unwrap();
            assert_eq!(res.status, TransactionStatus::Submitted);
        }

        #[tokio::test]
        async fn submit_transaction_try_again_later_keeps_tx_alive() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Provider returns TRY_AGAIN_LATER status
            let response = create_send_tx_response(
                "TRY_AGAIN_LATER",
                "0101010101010101010101010101010101010101010101010101010101010101",
            );
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(move |_| {
                    let r = response.clone();
                    Box::pin(async move { Ok(r) })
                });

            mocks
                .tx_repo
                .expect_record_stellar_try_again_later_retry()
                .withf(|id, sent_at| id == "tx-1" && !sent_at.is_empty())
                .returning(|id, _| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = TransactionStatus::Sent;
                    tx.metadata = Some(TransactionMetadata {
                        consecutive_failures: 0,
                        total_failures: 0,
                        insufficient_fee_retries: 0,
                        try_again_later_retries: 1,
                        nonce_too_high_retries: 0,
                    });
                    Ok::<_, RepositoryError>(tx)
                });

            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .withf(|_, scheduled_on| scheduled_on.is_some())
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent;
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
            }

            let res = handler.submit_transaction_impl(tx).await;

            // Transaction stays in Sent and gets a delayed fast resubmit.
            let returned_tx = res.unwrap();
            assert_eq!(returned_tx.status, TransactionStatus::Sent);
        }

        #[tokio::test]
        async fn submit_try_again_later_then_status_checker_reenqueues_submit() {
            let relayer = create_test_relayer();

            // submission returns TRY_AGAIN_LATER, transaction remains Sent.
            let mut submit_mocks = default_test_mocks();
            let response = create_send_tx_response(
                "TRY_AGAIN_LATER",
                "0101010101010101010101010101010101010101010101010101010101010101",
            );
            submit_mocks
                .provider
                .expect_send_transaction_with_status()
                .times(1)
                .returning(move |_| {
                    let r = response.clone();
                    Box::pin(async move { Ok(r) })
                });
            submit_mocks
                .tx_repo
                .expect_record_stellar_try_again_later_retry()
                .withf(|id, sent_at| id == "tx-1" && !sent_at.is_empty())
                .times(1)
                .returning(|id, sent_at| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = TransactionStatus::Sent;
                    tx.sent_at = Some(sent_at);
                    tx.metadata = Some(TransactionMetadata {
                        consecutive_failures: 0,
                        total_failures: 0,
                        insufficient_fee_retries: 0,
                        try_again_later_retries: 1,
                        nonce_too_high_retries: 0,
                    });
                    Ok::<_, RepositoryError>(tx)
                });

            submit_mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .withf(|_, scheduled_on| scheduled_on.is_some())
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let submit_handler = make_stellar_tx_handler(relayer.clone(), submit_mocks);
            let mut sent_tx = create_test_transaction(&relayer.id);
            sent_tx.status = TransactionStatus::Sent;
            if let NetworkTransactionData::Stellar(ref mut data) = sent_tx.network_data {
                data.signatures.push(dummy_signature());
                data.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
            }

            let mut returned_tx = submit_handler
                .submit_transaction_impl(sent_tx)
                .await
                .unwrap();
            assert_eq!(returned_tx.status, TransactionStatus::Sent);
            assert!(returned_tx.sent_at.is_some());

            // status check sees stale Sent tx and re-enqueues submit job.
            // Both created_at and sent_at must exceed the base resubmit interval
            // for the backoff logic to trigger. created_at is set earlier than sent_at
            // to match real-world invariants (transaction is created before being sent).
            use crate::constants::STELLAR_RESUBMIT_BASE_INTERVAL_SECONDS;
            let buffer = 2;
            let created_at = (Utc::now()
                - chrono::Duration::seconds(STELLAR_RESUBMIT_BASE_INTERVAL_SECONDS + buffer))
            .to_rfc3339();
            let sent_at = (Utc::now()
                - chrono::Duration::seconds(STELLAR_RESUBMIT_BASE_INTERVAL_SECONDS + 1))
            .to_rfc3339();
            returned_tx.created_at = created_at;
            returned_tx.sent_at = Some(sent_at);

            let mut status_mocks = default_test_mocks();
            status_mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let status_handler = make_stellar_tx_handler(relayer.clone(), status_mocks);
            let status_result = status_handler
                .handle_transaction_status_impl(returned_tx, None)
                .await
                .unwrap();
            assert_eq!(status_result.status, TransactionStatus::Sent);
        }

        #[tokio::test]
        async fn resubmit_try_again_later_returns_ok_for_submitted_tx() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Provider returns TRY_AGAIN_LATER status
            let response = create_send_tx_response(
                "TRY_AGAIN_LATER",
                "0101010101010101010101010101010101010101010101010101010101010101",
            );
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(move |_| {
                    let r = response.clone();
                    Box::pin(async move { Ok(r) })
                });

            mocks
                .tx_repo
                .expect_record_stellar_try_again_later_retry()
                .withf(|id, sent_at| id == "tx-1" && !sent_at.is_empty())
                .returning(|id, _| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = TransactionStatus::Submitted;
                    tx.metadata = Some(TransactionMetadata {
                        consecutive_failures: 0,
                        total_failures: 0,
                        insufficient_fee_retries: 0,
                        try_again_later_retries: 1,
                        nonce_too_high_retries: 0,
                    });
                    Ok::<_, RepositoryError>(tx)
                });

            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .withf(|_, scheduled_on| scheduled_on.is_some())
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Submitted; // Already submitted (resubmission path)
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
            }

            let res = handler.submit_transaction_impl(tx).await;

            // Should succeed without marking as failed and get a delayed fast resubmit.
            let returned_tx = res.unwrap();
            assert_eq!(returned_tx.status, TransactionStatus::Submitted);
        }

        #[tokio::test]
        async fn submit_transaction_try_again_later_fast_window_closes() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let response = create_send_tx_response(
                "TRY_AGAIN_LATER",
                "0101010101010101010101010101010101010101010101010101010101010101",
            );
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(move |_| {
                    let r = response.clone();
                    Box::pin(async move { Ok(r) })
                });

            mocks
                .tx_repo
                .expect_record_stellar_try_again_later_retry()
                .withf(|id, sent_at| id == "tx-1" && !sent_at.is_empty())
                .returning(|id, _| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = TransactionStatus::Sent;
                    tx.metadata = Some(TransactionMetadata {
                        consecutive_failures: 0,
                        total_failures: 0,
                        insufficient_fee_retries: 0,
                        try_again_later_retries: 4,
                        nonce_too_high_retries: 0,
                    });
                    Ok::<_, RepositoryError>(tx)
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent;
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
            }

            let res = handler.submit_transaction_impl(tx).await;

            let returned_tx = res.unwrap();
            assert_eq!(returned_tx.status, TransactionStatus::Sent);
            assert_eq!(
                returned_tx
                    .metadata
                    .as_ref()
                    .map(|metadata| metadata.try_again_later_retries),
                Some(4)
            );
        }

        #[tokio::test]
        async fn submit_transaction_try_again_later_final_state_skips_enqueue() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let response = create_send_tx_response(
                "TRY_AGAIN_LATER",
                "0101010101010101010101010101010101010101010101010101010101010101",
            );
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(move |_| {
                    let r = response.clone();
                    Box::pin(async move { Ok(r) })
                });

            mocks
                .tx_repo
                .expect_record_stellar_try_again_later_retry()
                .withf(|id, sent_at| id == "tx-1" && !sent_at.is_empty())
                .returning(|id, _| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = TransactionStatus::Confirmed;
                    tx.metadata = Some(TransactionMetadata {
                        consecutive_failures: 0,
                        total_failures: 0,
                        insufficient_fee_retries: 0,
                        try_again_later_retries: 0,
                        nonce_too_high_retries: 0,
                    });
                    Ok::<_, RepositoryError>(tx)
                })
                .times(1);

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent;
            tx.metadata = Some(TransactionMetadata {
                consecutive_failures: 0,
                total_failures: 0,
                insufficient_fee_retries: 0,
                try_again_later_retries: 0,
                nonce_too_high_retries: 0,
            });
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
            }

            let res = handler.submit_core(tx).await;

            assert!(res.is_ok());
            let returned_tx = res.unwrap();
            assert_eq!(returned_tx.status, TransactionStatus::Confirmed);
        }

        #[tokio::test]
        async fn submit_transaction_error_status_fails() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Provider returns ERROR status with error XDR
            let mut response = create_send_tx_response(
                "ERROR",
                "0101010101010101010101010101010101010101010101010101010101010101",
            );
            response.error_result_xdr = Some("not-base64".to_string());
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(move |_| {
                    let r = response.clone();
                    Box::pin(async move { Ok(r) })
                });

            // Mock finalize_transaction_state for failure handling
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, upd| upd.status == Some(TransactionStatus::Failed))
                .returning(|id, upd| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = upd.status.unwrap();
                    Ok::<_, RepositoryError>(tx)
                });

            // Mock notification for failed transaction
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Mock find_by_status_paginated for enqueue_next_pending_transaction
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
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent;
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
            }

            let res = handler.submit_transaction_impl(tx).await;

            // Transaction marked as failed — no error propagated
            let failed_tx = res.unwrap();
            assert_eq!(failed_tx.status, TransactionStatus::Failed);
        }

        #[tokio::test]
        async fn submit_transaction_insufficient_fee_keeps_tx_alive() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Provider returns ERROR status with insufficient fee XDR
            let mut response = create_send_tx_response(
                "ERROR",
                "0101010101010101010101010101010101010101010101010101010101010101",
            );
            response.error_result_xdr = Some("AAAAAAAAY/n////3AAAAAA==".to_string());
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(move |_| {
                    let r = response.clone();
                    Box::pin(async move { Ok(r) })
                });

            // insufficient-fee retry updates sent_at and retry metadata atomically
            mocks
                .tx_repo
                .expect_record_stellar_insufficient_fee_retry()
                .withf(|id, sent_at| id == "tx-1" && !sent_at.is_empty())
                .returning(|id, _| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = TransactionStatus::Sent;
                    tx.metadata = Some(TransactionMetadata {
                        consecutive_failures: 0,
                        total_failures: 0,
                        insufficient_fee_retries: 1,
                        try_again_later_retries: 0,
                        nonce_too_high_retries: 0,
                    });
                    Ok::<_, RepositoryError>(tx)
                })
                .times(1);

            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .withf(|_, scheduled_on| scheduled_on.is_some())
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent;
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
            }

            let res = handler.submit_transaction_impl(tx).await;

            // Transaction stays alive — status checker will retry
            let returned_tx = res.unwrap();
            assert_eq!(returned_tx.status, TransactionStatus::Sent);
            assert_eq!(
                returned_tx
                    .metadata
                    .as_ref()
                    .map(|metadata| metadata.insufficient_fee_retries),
                Some(1)
            );
        }

        #[tokio::test]
        async fn submit_transaction_insufficient_fee_escalates_delay() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();
            let before_submit = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));
            let before_submit_for_mock = before_submit.clone();
            let expected_delay = STELLAR_FAST_RESUBMIT_BASE_DELAY_SECONDS * 3;

            let mut response = create_send_tx_response(
                "ERROR",
                "0101010101010101010101010101010101010101010101010101010101010101",
            );
            response.error_result_xdr = Some("AAAAAAAAY/n////3AAAAAA==".to_string());
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(move |_| {
                    let r = response.clone();
                    Box::pin(async move { Ok(r) })
                });

            mocks
                .tx_repo
                .expect_record_stellar_insufficient_fee_retry()
                .withf(|id, sent_at| id == "tx-1" && !sent_at.is_empty())
                .returning(|id, _| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = TransactionStatus::Sent;
                    tx.metadata = Some(TransactionMetadata {
                        consecutive_failures: 0,
                        total_failures: 0,
                        insufficient_fee_retries: 3,
                        try_again_later_retries: 0,
                        nonce_too_high_retries: 0,
                    });
                    Ok::<_, RepositoryError>(tx)
                })
                .times(1);

            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .withf(move |_, scheduled_on| {
                    let before_submit =
                        before_submit_for_mock.load(std::sync::atomic::Ordering::SeqCst);
                    let now = Utc::now().timestamp();
                    scheduled_on.is_some_and(|scheduled_on| {
                        scheduled_on >= before_submit + expected_delay - 1
                            && scheduled_on <= now + expected_delay + 1
                    })
                })
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent;
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
            }

            before_submit.store(Utc::now().timestamp(), std::sync::atomic::Ordering::SeqCst);
            let res = handler.submit_transaction_impl(tx).await;

            let returned_tx = res.unwrap();
            assert_eq!(returned_tx.status, TransactionStatus::Sent);
            assert_eq!(
                returned_tx
                    .metadata
                    .as_ref()
                    .map(|metadata| metadata.insufficient_fee_retries),
                Some(3)
            );
        }

        #[tokio::test]
        async fn submit_transaction_insufficient_fee_post_record_over_cap_skips_enqueue() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut response = create_send_tx_response(
                "ERROR",
                "0101010101010101010101010101010101010101010101010101010101010101",
            );
            response.error_result_xdr = Some("AAAAAAAAY/n////3AAAAAA==".to_string());
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(move |_| {
                    let r = response.clone();
                    Box::pin(async move { Ok(r) })
                });

            mocks
                .tx_repo
                .expect_record_stellar_insufficient_fee_retry()
                .withf(|id, sent_at| id == "tx-1" && !sent_at.is_empty())
                .returning(|id, _| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = TransactionStatus::Sent;
                    tx.metadata = Some(TransactionMetadata {
                        consecutive_failures: 0,
                        total_failures: 0,
                        insufficient_fee_retries: STELLAR_INSUFFICIENT_FEE_MAX_RETRIES + 1,
                        try_again_later_retries: 0,
                        nonce_too_high_retries: 0,
                    });
                    Ok::<_, RepositoryError>(tx)
                })
                .times(1);

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent;
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
            }

            let res = handler.submit_core(tx).await;

            let returned_tx = res.unwrap();
            assert_eq!(returned_tx.status, TransactionStatus::Sent);
            assert_eq!(
                returned_tx
                    .metadata
                    .as_ref()
                    .map(|metadata| metadata.insufficient_fee_retries),
                Some(STELLAR_INSUFFICIENT_FEE_MAX_RETRIES + 1)
            );
        }

        #[tokio::test]
        async fn submit_transaction_insufficient_fee_final_state_skips_enqueue() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut response = create_send_tx_response(
                "ERROR",
                "0101010101010101010101010101010101010101010101010101010101010101",
            );
            response.error_result_xdr = Some("AAAAAAAAY/n////3AAAAAA==".to_string());
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(move |_| {
                    let r = response.clone();
                    Box::pin(async move { Ok(r) })
                });

            mocks
                .tx_repo
                .expect_record_stellar_insufficient_fee_retry()
                .withf(|id, sent_at| id == "tx-1" && !sent_at.is_empty())
                .returning(|id, _| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = TransactionStatus::Confirmed;
                    tx.metadata = Some(TransactionMetadata {
                        consecutive_failures: 0,
                        total_failures: 0,
                        insufficient_fee_retries: 0,
                        try_again_later_retries: 0,
                        nonce_too_high_retries: 0,
                    });
                    Ok::<_, RepositoryError>(tx)
                })
                .times(1);

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent;
            tx.metadata = Some(TransactionMetadata {
                consecutive_failures: 0,
                total_failures: 0,
                insufficient_fee_retries: 0,
                try_again_later_retries: 0,
                nonce_too_high_retries: 0,
            });
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
            }

            let res = handler.submit_core(tx).await;

            assert!(res.is_ok());
            let returned_tx = res.unwrap();
            assert_eq!(returned_tx.status, TransactionStatus::Confirmed);
        }

        #[tokio::test]
        async fn submit_transaction_fast_resubmit_enqueue_failure_is_swallowed() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut response = create_send_tx_response(
                "ERROR",
                "0101010101010101010101010101010101010101010101010101010101010101",
            );
            response.error_result_xdr = Some("AAAAAAAAY/n////3AAAAAA==".to_string());
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(move |_| {
                    let r = response.clone();
                    Box::pin(async move { Ok(r) })
                });

            mocks
                .tx_repo
                .expect_record_stellar_insufficient_fee_retry()
                .withf(|id, sent_at| id == "tx-1" && !sent_at.is_empty())
                .returning(|id, _| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = TransactionStatus::Sent;
                    tx.metadata = Some(TransactionMetadata {
                        consecutive_failures: 0,
                        total_failures: 0,
                        insufficient_fee_retries: 1,
                        try_again_later_retries: 0,
                        nonce_too_high_retries: 0,
                    });
                    Ok::<_, RepositoryError>(tx)
                })
                .times(1);

            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .withf(|_, scheduled_on| scheduled_on.is_some())
                .times(1)
                .returning(|_, _| {
                    Box::pin(async {
                        Err(crate::jobs::JobProducerError::QueueError(
                            "queue unavailable".to_string(),
                        ))
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent;
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
            }

            let res = handler.submit_transaction_impl(tx).await;

            let returned_tx = res.unwrap();
            assert_eq!(returned_tx.status, TransactionStatus::Sent);
            assert_eq!(
                returned_tx
                    .metadata
                    .as_ref()
                    .map(|metadata| metadata.insufficient_fee_retries),
                Some(1)
            );
        }

        #[tokio::test]
        async fn submit_transaction_try_again_later_enqueue_failure_is_swallowed() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let response = create_send_tx_response(
                "TRY_AGAIN_LATER",
                "0101010101010101010101010101010101010101010101010101010101010101",
            );
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(move |_| {
                    let r = response.clone();
                    Box::pin(async move { Ok(r) })
                });

            mocks
                .tx_repo
                .expect_record_stellar_try_again_later_retry()
                .withf(|id, sent_at| id == "tx-1" && !sent_at.is_empty())
                .returning(|id, _| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = TransactionStatus::Sent;
                    tx.metadata = Some(TransactionMetadata {
                        consecutive_failures: 0,
                        total_failures: 0,
                        insufficient_fee_retries: 0,
                        try_again_later_retries: 1,
                        nonce_too_high_retries: 0,
                    });
                    Ok::<_, RepositoryError>(tx)
                })
                .times(1);

            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .withf(|_, scheduled_on| scheduled_on.is_some())
                .times(1)
                .returning(|_, _| {
                    Box::pin(async {
                        Err(crate::jobs::JobProducerError::QueueError(
                            "queue unavailable".to_string(),
                        ))
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent;
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
            }

            let res = handler.submit_transaction_impl(tx).await;

            let returned_tx = res.unwrap();
            assert_eq!(returned_tx.status, TransactionStatus::Sent);
            assert_eq!(
                returned_tx
                    .metadata
                    .as_ref()
                    .map(|metadata| metadata.try_again_later_retries),
                Some(1)
            );
        }

        #[tokio::test]
        async fn submit_transaction_insufficient_fee_exceeding_retry_limit_fails() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();
            let retry_limit_reason = format!(
                "insufficient fee retry limit exceeded ({STELLAR_INSUFFICIENT_FEE_MAX_RETRIES})"
            );
            let retry_limit_reason_for_mock = retry_limit_reason.clone();

            let mut response = create_send_tx_response(
                "ERROR",
                "0101010101010101010101010101010101010101010101010101010101010101",
            );
            response.error_result_xdr = Some("AAAAAAAAY/n////3AAAAAA==".to_string());
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(move |_| {
                    let r = response.clone();
                    Box::pin(async move { Ok(r) })
                });

            mocks
                .tx_repo
                .expect_partial_update()
                .withf(move |_, upd| {
                    upd.status == Some(TransactionStatus::Failed)
                        && upd
                            .status_reason
                            .as_ref()
                            .is_some_and(|reason| reason.contains(&retry_limit_reason_for_mock))
                })
                .returning(|id, upd| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = upd.status.unwrap();
                    tx.status_reason = upd.status_reason;
                    Ok::<_, RepositoryError>(tx)
                });

            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

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
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent;
            tx.metadata = Some(TransactionMetadata {
                insufficient_fee_retries: STELLAR_INSUFFICIENT_FEE_MAX_RETRIES,
                ..Default::default()
            });
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
            }

            let res = handler.submit_transaction_impl(tx).await;

            let failed_tx = res.unwrap();
            assert_eq!(failed_tx.status, TransactionStatus::Failed);
            assert!(failed_tx
                .status_reason
                .as_ref()
                .is_some_and(|reason| reason.contains(&retry_limit_reason)));
        }

        #[tokio::test]
        async fn submit_transaction_error_non_fee_still_fails() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Provider returns ERROR status with a non-fee error
            let mut response = create_send_tx_response(
                "ERROR",
                "0101010101010101010101010101010101010101010101010101010101010101",
            );
            response.error_result_xdr = Some("AAAAAAAAA/v////6AAAAAA==".to_string());
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(move |_| {
                    let r = response.clone();
                    Box::pin(async move { Ok(r) })
                });

            // Mock finalize_transaction_state for failure handling
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, upd| upd.status == Some(TransactionStatus::Failed))
                .returning(|id, upd| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = upd.status.unwrap();
                    Ok::<_, RepositoryError>(tx)
                });

            // Mock notification for failed transaction
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Mock find_by_status_paginated for enqueue_next_pending_transaction
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
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent;
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
            }

            let res = handler.submit_transaction_impl(tx).await;

            // Non-fee ERROR still marks as failed
            let failed_tx = res.unwrap();
            assert_eq!(failed_tx.status, TransactionStatus::Failed);
        }

        #[tokio::test]
        async fn submit_transaction_concurrent_update_conflict_reloads_latest_state() {
            // When partial_update fails with ConcurrentUpdateConflict during submission,
            // the handler should reload the latest state via get_by_id and return Ok.
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Provider returns PENDING — submission to RPC succeeded
            let response = create_send_tx_response(
                "PENDING",
                "0101010101010101010101010101010101010101010101010101010101010101",
            );
            mocks
                .provider
                .expect_send_transaction_with_status()
                .returning(move |_| {
                    let r = response.clone();
                    Box::pin(async move { Ok(r) })
                });

            // partial_update (Submitted) fails with CAS conflict
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, upd| upd.status == Some(TransactionStatus::Submitted))
                .times(1)
                .returning(|_, _| {
                    Err(RepositoryError::ConcurrentUpdateConflict(
                        "CAS mismatch".to_string(),
                    ))
                });

            // After conflict, handler reloads via get_by_id
            let reloaded_tx = {
                let mut t = create_test_transaction(&relayer.id);
                t.status = TransactionStatus::Submitted;
                t
            };
            let reloaded_clone = reloaded_tx.clone();
            mocks
                .tx_repo
                .expect_get_by_id()
                .times(1)
                .returning(move |_| Ok(reloaded_clone.clone()));

            // No failure handling (notifications, next-pending) should occur
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .never();
            mocks
                .job_producer
                .expect_produce_transaction_request_job()
                .never();

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            tx.status = TransactionStatus::Sent;
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.signed_envelope_xdr = Some(create_signed_xdr(TEST_PK, TEST_PK_2));
            }

            let res = handler.submit_transaction_impl(tx).await;

            assert!(res.is_ok(), "CAS conflict should return Ok after reload");
            let returned_tx = res.unwrap();
            // Reloaded state reflects the concurrent writer's update
            assert_eq!(returned_tx.status, TransactionStatus::Submitted);
        }
    }
}
