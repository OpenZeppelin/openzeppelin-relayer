//! This module contains the submission-related functionality for Stellar transactions.
//! It includes methods for submitting transactions with robust error handling,
//! ensuring proper transaction state management on failure.

use chrono::Utc;
use log::{info, warn};

use super::{utils::is_bad_sequence_error, StellarRelayerTransaction};
use crate::{
    jobs::{JobProducerTrait, TransactionStatusCheck},
    models::{
        NetworkTransactionData, RelayerRepoModel, TransactionError, TransactionRepoModel,
        TransactionStatus, TransactionUpdateRequest,
    },
    repositories::{Repository, TransactionCounterTrait, TransactionRepository},
    services::{Signer, StellarProviderTrait},
};

impl<R, T, J, S, P, C> StellarRelayerTransaction<R, T, J, S, P, C>
where
    R: Repository<RelayerRepoModel, String> + Send + Sync,
    T: TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    S: Signer + Send + Sync,
    P: StellarProviderTrait + Send + Sync,
    C: TransactionCounterTrait + Send + Sync,
{
    /// Main submission method with robust error handling.
    /// Unlike prepare, submit doesn't claim lanes but still needs proper error handling.
    pub async fn submit_transaction_impl(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!("Submitting Stellar transaction: {:?}", tx.id);

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
    async fn submit_core(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let stellar_data = tx.network_data.get_stellar_transaction_data()?;
        let tx_envelope = stellar_data
            .get_envelope_for_submission()
            .map_err(TransactionError::from)?;

        let hash = self
            .provider()
            .send_transaction(&tx_envelope)
            .await
            .map_err(TransactionError::from)?;

        let tx_hash_hex = hex::encode(hash.as_slice());
        let updated_stellar_data = stellar_data.with_hash(tx_hash_hex.clone());

        let mut hashes = tx.hashes.clone();
        hashes.push(tx_hash_hex);

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

        // Enqueue status check job
        self.job_producer()
            .produce_check_transaction_status_job(
                TransactionStatusCheck::new(updated_tx.id.clone(), updated_tx.relayer_id.clone()),
                None,
            )
            .await?;

        // Send notification
        self.send_transaction_update_notification(&updated_tx)
            .await?;

        Ok(updated_tx)
    }

    /// Handles submission failures with comprehensive cleanup and error reporting.
    /// For bad sequence errors, resets the transaction and re-enqueues it for retry.
    async fn handle_submit_failure(
        &self,
        tx: TransactionRepoModel,
        error: TransactionError,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let error_reason = format!("Submission failed: {}", error);
        let tx_id = tx.id.clone();
        warn!("Transaction {} submission failed: {}", tx_id, error_reason);

        if is_bad_sequence_error(&error_reason) {
            // For bad sequence errors, sync sequence from chain first
            if let Ok(stellar_data) = tx.network_data.get_stellar_transaction_data() {
                info!(
                    "Syncing sequence from chain after bad sequence error for transaction {}",
                    tx_id
                );
                match self
                    .sync_sequence_from_chain(&stellar_data.source_account)
                    .await
                {
                    Ok(()) => {
                        info!(
                            "Successfully synced sequence from chain for transaction {}",
                            tx_id
                        );
                    }
                    Err(sync_error) => {
                        warn!(
                            "Failed to sync sequence from chain for transaction {}: {}",
                            tx_id, sync_error
                        );
                    }
                }
            }

            // Reset the transaction and re-enqueue it
            info!(
                "Bad sequence error detected for transaction {}. Resetting and re-enqueueing.",
                tx_id
            );

            // Reset the transaction to pending state
            match self.reset_transaction_for_retry(tx.clone()).await {
                Ok(reset_tx) => {
                    // Re-enqueue the transaction to go through the pipeline again
                    if let Err(e) = self.send_transaction_request_job(&reset_tx, Some(2)).await {
                        warn!(
                            "Failed to re-enqueue transaction {} after reset: {}",
                            tx_id, e
                        );
                    } else {
                        info!(
                            "Transaction {} reset and re-enqueued for retry through pipeline",
                            tx_id
                        );
                    }

                    // Return success since we're handling the retry
                    return Ok(reset_tx);
                }
                Err(reset_error) => {
                    warn!(
                        "Failed to reset transaction {} for retry: {}",
                        tx_id, reset_error
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
        let _failed_tx = match self
            .finalize_transaction_state(tx_id.clone(), update_request)
            .await
        {
            Ok(updated_tx) => updated_tx,
            Err(finalize_error) => {
                warn!(
                    "Failed to mark transaction {} as failed: {}. Continuing with lane cleanup.",
                    tx_id, finalize_error
                );
                tx
            }
        };

        // Attempt to enqueue next pending transaction or release lane
        if let Err(enqueue_error) = self.enqueue_next_pending_transaction(&tx_id).await {
            warn!(
                "Failed to enqueue next pending transaction after {} submission failure: {}.",
                tx_id, enqueue_error
            );
        }

        info!(
            "Transaction {} submission failure handled. Error: {}",
            tx_id, error_reason
        );

        Err(error)
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
    use soroban_rs::xdr::{Hash, WriteXdr};

    use crate::domain::transaction::stellar::test_helpers::*;

    mod submit_transaction_tests {
        use crate::models::RepositoryError;

        use super::*;

        #[tokio::test]
        async fn submit_transaction_happy_path() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // provider gives a hash
            mocks
                .provider
                .expect_send_transaction()
                .returning(|_| Box::pin(async { Ok(Hash([1u8; 32])) }));

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

            // enqueue status-check & notification
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);

            let mut tx = create_test_transaction(&relayer.id);
            if let NetworkTransactionData::Stellar(ref mut d) = tx.network_data {
                d.signatures.push(dummy_signature());
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
                .expect_send_transaction()
                .returning(|_| Box::pin(async { Err(eyre::eyre!("Network error")) }));

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

            // Mock find_by_status for enqueue_next_pending_transaction
            mocks
                .tx_repo
                .expect_find_by_status()
                .returning(|_, _| Ok(vec![])); // No pending transactions

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.sequence_number = Some(42); // Set sequence number
            }

            let res = handler.submit_transaction_impl(tx).await;

            // Should return error but transaction should be marked as failed
            assert!(res.is_err());
            matches!(res.unwrap_err(), TransactionError::UnexpectedError(_));
        }

        #[tokio::test]
        async fn submit_transaction_repository_error_marks_failed() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Provider succeeds
            mocks
                .provider
                .expect_send_transaction()
                .returning(|_| Box::pin(async { Ok(Hash([1u8; 32])) }));

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

            // Mock find_by_status for enqueue_next_pending_transaction
            mocks
                .tx_repo
                .expect_find_by_status()
                .returning(|_, _| Ok(vec![])); // No pending transactions

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.sequence_number = Some(42); // Set sequence number
            }

            let res = handler.submit_transaction_impl(tx).await;

            // Should return error but transaction should be marked as failed
            assert!(res.is_err());
        }

        #[tokio::test]
        async fn submit_transaction_uses_signed_envelope_xdr() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Create a transaction with signed_envelope_xdr set
            let mut tx = create_test_transaction(&relayer.id);
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
            mocks
                .provider
                .expect_send_transaction()
                .returning(|_| Box::pin(async { Ok(Hash([2u8; 32])) }));

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

            // Job and notification expectations
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));
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

            // provider gives a hash
            mocks
                .provider
                .expect_send_transaction()
                .returning(|_| Box::pin(async { Ok(Hash([1u8; 32])) }));

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

            // enqueue status-check & notification
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);

            let mut tx = create_test_transaction(&relayer.id);
            if let NetworkTransactionData::Stellar(ref mut d) = tx.network_data {
                d.signatures.push(dummy_signature());
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
                .expect_send_transaction()
                .returning(|_| Box::pin(async { Err(eyre::eyre!("Network error")) }));

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
            mocks
                .tx_repo
                .expect_find_by_status()
                .with(
                    mockall::predicate::eq(relayer.id.clone()),
                    mockall::predicate::eq(vec![TransactionStatus::Pending]),
                )
                .times(1)
                .returning(move |_, _| Ok(vec![captured_pending_tx.clone()]));

            // Mock produce_transaction_request_job for the next pending transaction
            mocks
                .job_producer
                .expect_produce_transaction_request_job()
                .withf(move |job, _delay| job.transaction_id == "next-pending-tx")
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.sequence_number = Some(42); // Set sequence number
            }

            let res = handler.submit_transaction_impl(tx).await;

            // Should return error but next transaction should be enqueued
            assert!(res.is_err());
            matches!(res.unwrap_err(), TransactionError::UnexpectedError(_));
        }

        #[tokio::test]
        async fn test_submit_bad_sequence_resets_and_retries() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock provider to return bad sequence error
            mocks.provider.expect_send_transaction().returning(|_| {
                Box::pin(async { Err(eyre::eyre!("transaction submission failed: TxBadSeq")) })
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

            // Mock counter set for sync_sequence_from_chain
            mocks
                .counter
                .expect_set()
                .times(1)
                .returning(|_, _, _| Box::pin(async { Ok(()) }));

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

            // Mock produce_transaction_request_job for re-enqueue
            mocks
                .job_producer
                .expect_produce_transaction_request_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(dummy_signature());
                data.sequence_number = Some(42);
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
    }
}
