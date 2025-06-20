//! This module contains the preparation-related functionality for Stellar transactions.
//! It includes methods for preparing transactions with robust error handling,
//! ensuring lanes are always properly cleaned up on failure.

use eyre::Result;
use log::{info, warn};

use super::{i64_from_u64, lane_gate, StellarRelayerTransaction};
use crate::{
    domain::{needs_simulation, SignTransactionResponse},
    jobs::{JobProducerTrait, TransactionSend},
    models::{
        NetworkTransactionData, OperationSpec, RelayerRepoModel, TransactionError,
        TransactionRepoModel, TransactionStatus, TransactionUpdateRequest,
    },
    repositories::{Repository, TransactionCounterTrait, TransactionRepository},
    services::{Signer, StellarProviderTrait},
};
use soroban_rs::{stellar_rpc_client::SimulateTransactionResponse, xdr::TransactionEnvelope};

impl<R, T, J, S, P, C> StellarRelayerTransaction<R, T, J, S, P, C>
where
    R: Repository<RelayerRepoModel, String> + Send + Sync,
    T: TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    S: Signer + Send + Sync,
    P: StellarProviderTrait + Send + Sync,
    C: TransactionCounterTrait + Send + Sync,
{
    /// Optionally invoke the RPC simulation depending on the transaction operations.
    /// Returns the simulation response if simulation was needed and successful.
    async fn simulate_if_needed(
        &self,
        unsigned_env: &TransactionEnvelope,
        operations: &[OperationSpec],
    ) -> Result<Option<SimulateTransactionResponse>, TransactionError> {
        if needs_simulation(operations) {
            let resp = self
                .provider()
                .simulate_transaction_envelope(unsigned_env)
                .await
                .map_err(TransactionError::from)?;

            if let Some(err_msg) = resp.error.clone() {
                warn!("Stellar simulation failed: {}", err_msg);
                return Err(TransactionError::SimulationFailed(err_msg));
            }

            return Ok(Some(resp));
        }

        Ok(None)
    }

    /// Send a submit-transaction job for the given transaction.
    async fn send_submit_transaction_job(
        &self,
        tx: &TransactionRepoModel,
        delay_seconds: Option<i64>,
    ) -> Result<(), TransactionError> {
        let job = TransactionSend::submit(tx.id.clone(), tx.relayer_id.clone());
        self.job_producer()
            .produce_submit_transaction_job(job, delay_seconds)
            .await?;
        Ok(())
    }

    /// Get the next sequence number for this relayer.
    fn next_sequence(&self) -> Result<i64, TransactionError> {
        let sequence_u64 = self
            .transaction_counter_service()
            .get_and_increment(&self.relayer().id, &self.relayer().address)
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        i64_from_u64(sequence_u64).map_err(|relayer_err| {
            let msg = format!(
                "Sequence conversion error for {}: {}",
                sequence_u64, relayer_err
            );
            TransactionError::ValidationError(msg)
        })
    }

    /// Main preparation method with robust error handling and guaranteed lane cleanup.
    pub async fn prepare_transaction_impl(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        if !lane_gate::claim(&self.relayer().id, &tx.id) {
            info!(
                "Relayer {} already has a transaction in flight – {} must wait.",
                self.relayer().id,
                tx.id
            );
            return Ok(tx);
        }

        info!("Preparing transaction: {:?}", tx.id);

        // Call core preparation logic with error handling
        match self.prepare_core(tx.clone()).await {
            Ok(prepared_tx) => Ok(prepared_tx),
            Err(error) => {
                // Always cleanup on failure - this is the critical safety mechanism
                self.handle_prepare_failure(tx, error).await
            }
        }
    }

    /// Core preparation logic - pure business logic without lane management concerns.
    async fn prepare_core(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let sequence_i64 = self.next_sequence()?;
        info!(
            "Using sequence number {} for Stellar transaction {}",
            sequence_i64, tx.id
        );

        let stellar_data = tx.network_data.get_stellar_transaction_data()?;
        let mut stellar_data_with_seq = stellar_data.with_sequence_number(sequence_i64);

        let unsigned_env = stellar_data_with_seq
            .unsigned_envelope()
            .map_err(TransactionError::from)?;

        let simulation_response = self
            .simulate_if_needed(&unsigned_env, &stellar_data_with_seq.operations)
            .await?;

        // Apply simulation results if available
        if let Some(sim_resp) = simulation_response {
            info!("Applying simulation results to transaction");
            stellar_data_with_seq = stellar_data_with_seq.with_simulation_data(sim_resp)?;
        }

        let sig_resp = self
            .signer()
            .sign_transaction(NetworkTransactionData::Stellar(
                stellar_data_with_seq.clone(),
            ))
            .await?;

        let signature = match sig_resp {
            SignTransactionResponse::Stellar(s) => s.signature,
            _ => {
                return Err(TransactionError::InvalidType(
                    "Expected Stellar signature".into(),
                ));
            }
        };

        let final_stellar_data = stellar_data_with_seq.attach_signature(signature);
        let updated_network_data = NetworkTransactionData::Stellar(final_stellar_data);

        let update_req = TransactionUpdateRequest {
            status: Some(TransactionStatus::Sent),
            network_data: Some(updated_network_data),
            ..Default::default()
        };

        let saved_tx = self
            .transaction_repository()
            .partial_update(tx.id.clone(), update_req)
            .await?;

        self.send_submit_transaction_job(&saved_tx, None).await?;
        self.send_transaction_update_notification(&saved_tx).await?;

        Ok(saved_tx)
    }

    /// Handles preparation failures with comprehensive cleanup and error reporting.
    /// This method ensures lanes are never left claimed after any failure.
    async fn handle_prepare_failure(
        &self,
        tx: TransactionRepoModel,
        error: TransactionError,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let error_reason = format!("Preparation failed: {}", error);
        let tx_id = tx.id.clone(); // Clone the ID before moving tx
        warn!("Transaction {} preparation failed: {}", tx_id, error_reason);

        // Step 1: Mark transaction as Failed with detailed reason
        let _failed_tx = match self
            .finalize_transaction_state(
                tx_id.clone(),
                TransactionStatus::Failed,
                Some(error_reason.clone()),
                None,
            )
            .await
        {
            Ok(updated_tx) => updated_tx,
            Err(finalize_error) => {
                warn!(
                    "Failed to mark transaction {} as failed: {}. Proceeding with lane cleanup.",
                    tx_id, finalize_error
                );
                // Continue with cleanup even if we can't update the transaction
                tx
            }
        };

        // Step 2: Attempt to enqueue next pending transaction or release lane
        if let Err(enqueue_error) = self.enqueue_next_pending_transaction(&tx_id).await {
            warn!(
                "Failed to enqueue next pending transaction after {} failure: {}. Releasing lane directly.",
                tx_id, enqueue_error
            );
            // Fallback: release lane directly if we can't hand it over
            lane_gate::free(&self.relayer().id, &tx_id);
        }

        // Step 3: Log failure for monitoring (prepare_fail_total metric would go here)
        info!(
            "Transaction {} preparation failure handled. Lane cleaned up. Error: {}",
            tx_id, error_reason
        );

        // Step 4: Return original error to maintain API compatibility
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{domain::SignTransactionResponse, models::RepositoryError};

    use crate::domain::transaction::stellar::test_helpers::*;

    mod prepare_transaction_tests {
        use super::*;

        #[tokio::test]
        async fn prepare_transaction_happy_path() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // sequence counter
            mocks
                .counter
                .expect_get_and_increment()
                .returning(|_, _| Ok(1));

            // signer
            mocks.signer.expect_sign_transaction().returning(|_| {
                Box::pin(async {
                    Ok(SignTransactionResponse::Stellar(
                        crate::domain::SignTransactionResponseStellar {
                            signature: dummy_signature(),
                        },
                    ))
                })
            });

            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, upd| {
                    upd.status == Some(TransactionStatus::Sent) && upd.network_data.is_some()
                })
                .returning(|id, upd| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = upd.status.unwrap();
                    tx.network_data = upd.network_data.unwrap();
                    Ok::<_, RepositoryError>(tx)
                });

            // submit-job + notification
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);

            assert!(handler.prepare_transaction_impl(tx).await.is_ok());
        }

        #[tokio::test]
        async fn prepare_transaction_sequence_failure_cleans_up_lane() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock sequence counter to fail
            mocks.counter.expect_get_and_increment().returning(|_, _| {
                Err(crate::repositories::TransactionCounterError::NotFound(
                    "Counter service failure".to_string(),
                ))
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

            // Mock find_by_status for enqueue_next_pending_transaction
            mocks
                .tx_repo
                .expect_find_by_status()
                .returning(|_, _| Ok(vec![])); // No pending transactions

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);

            // Verify that lane is claimed initially
            assert!(lane_gate::claim(&relayer.id, &tx.id));

            let result = handler.prepare_transaction_impl(tx.clone()).await;

            // Should return error but lane should be cleaned up
            assert!(result.is_err());

            // Verify lane is released - another transaction should be able to claim it
            let another_tx_id = "another-tx";
            assert!(lane_gate::claim(&relayer.id, another_tx_id));
            lane_gate::free(&relayer.id, another_tx_id)
        }

        #[tokio::test]
        async fn prepare_transaction_signer_failure_cleans_up_lane() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // sequence counter succeeds
            mocks
                .counter
                .expect_get_and_increment()
                .returning(|_, _| Ok(1));

            // signer fails
            mocks.signer.expect_sign_transaction().returning(|_| {
                Box::pin(async {
                    Err(crate::models::SignerError::SigningError(
                        "Signer failure".to_string(),
                    ))
                })
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

            // Mock find_by_status for enqueue_next_pending_transaction
            mocks
                .tx_repo
                .expect_find_by_status()
                .returning(|_, _| Ok(vec![])); // No pending transactions

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);

            let result = handler.prepare_transaction_impl(tx.clone()).await;

            // Should return error but lane should be cleaned up
            assert!(result.is_err());

            // Verify lane is released
            let another_tx_id = "another-tx";
            assert!(lane_gate::claim(&relayer.id, another_tx_id));
            lane_gate::free(&relayer.id, another_tx_id); // cleanup
        }

        #[tokio::test]
        async fn prepare_transaction_already_claimed_lane_returns_original() {
            let mut relayer = create_test_relayer();
            relayer.id = "unique-relayer-for-lane-test".to_string(); // Use unique relayer ID
            let mocks = default_test_mocks();

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);

            // Claim lane with different transaction
            assert!(lane_gate::claim(&relayer.id, "other-tx"));

            let result = handler.prepare_transaction_impl(tx.clone()).await;

            // Should return Ok with original transaction (waiting)
            assert!(result.is_ok());
            let returned_tx = result.unwrap();
            assert_eq!(returned_tx.id, tx.id);
            assert_eq!(returned_tx.status, tx.status);

            // Cleanup
            lane_gate::free(&relayer.id, "other-tx");
        }
    }

    mod next_sequence_tests {
        use super::*;
        use crate::repositories::TransactionCounterError;

        #[test]
        fn next_sequence_success() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock counter service to return a valid sequence
            mocks
                .counter
                .expect_get_and_increment()
                .withf(|relayer_id, address| relayer_id == "relayer-1" && address == TEST_PK)
                .returning(|_, _| Ok(42u64));

            let handler = make_stellar_tx_handler(relayer, mocks);
            let result = handler.next_sequence();

            assert!(result.is_ok());
            assert_eq!(result.unwrap(), 42i64);
        }

        #[test]
        fn next_sequence_counter_error() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock counter service to return an error
            mocks.counter.expect_get_and_increment().returning(|_, _| {
                Err(TransactionCounterError::NotFound(
                    "Counter not found".to_string(),
                ))
            });

            let handler = make_stellar_tx_handler(relayer, mocks);
            let result = handler.next_sequence();

            assert!(result.is_err());
            match result.unwrap_err() {
                TransactionError::UnexpectedError(msg) => {
                    assert!(msg.contains("Counter not found"));
                }
                _ => panic!("Expected UnexpectedError"),
            }
        }

        #[test]
        fn next_sequence_conversion_error() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock counter service to return a value that can't be converted to i64
            mocks
                .counter
                .expect_get_and_increment()
                .returning(|_, _| Ok(u64::MAX)); // This will cause conversion error

            let handler = make_stellar_tx_handler(relayer, mocks);
            let result = handler.next_sequence();

            assert!(result.is_err());
            match result.unwrap_err() {
                TransactionError::ValidationError(msg) => {
                    assert!(msg.contains("Sequence conversion error"));
                    assert!(msg.contains(&u64::MAX.to_string()));
                }
                _ => panic!("Expected ValidationError"),
            }
        }

        #[test]
        fn next_sequence_edge_case_max_i64() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Test with the maximum valid i64 value
            mocks
                .counter
                .expect_get_and_increment()
                .returning(|_, _| Ok(i64::MAX as u64));

            let handler = make_stellar_tx_handler(relayer, mocks);
            let result = handler.next_sequence();

            assert!(result.is_ok());
            assert_eq!(result.unwrap(), i64::MAX);
        }

        #[test]
        fn next_sequence_edge_case_just_above_i64_max() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Test with a value just above i64::MAX
            mocks
                .counter
                .expect_get_and_increment()
                .returning(|_, _| Ok((i64::MAX as u64) + 1));

            let handler = make_stellar_tx_handler(relayer, mocks);
            let result = handler.next_sequence();

            assert!(result.is_err());
            match result.unwrap_err() {
                TransactionError::ValidationError(msg) => {
                    assert!(msg.contains("Sequence conversion error"));
                }
                _ => panic!("Expected ValidationError"),
            }
        }
    }

    mod send_submit_transaction_job_tests {
        use super::*;

        #[tokio::test]
        async fn send_submit_transaction_job_success() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock successful job production
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .withf(|job, delay| {
                    job.transaction_id == "tx-1" && job.relayer_id == "relayer-1" && delay.is_none()
                })
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);

            let result = handler.send_submit_transaction_job(&tx, None).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn send_submit_transaction_job_with_delay() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock successful job production with delay
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .withf(|job, delay| {
                    job.transaction_id == "tx-1"
                        && job.relayer_id == "relayer-1"
                        && delay == &Some(30)
                })
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);

            let result = handler.send_submit_transaction_job(&tx, Some(30)).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn send_submit_transaction_job_handles_producer_error() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock job producer failure
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .times(1)
                .returning(|_, _| {
                    Box::pin(async {
                        Err(crate::jobs::JobProducerError::QueueError(
                            "Job queue is full".to_string(),
                        ))
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);

            let result = handler.send_submit_transaction_job(&tx, None).await;
            assert!(result.is_err());
        }
    }
}
