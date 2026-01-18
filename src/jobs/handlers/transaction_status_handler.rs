//! Transaction status monitoring handler.
//!
//! Monitors the status of submitted transactions by:
//! - Checking transaction status on the network
//! - Updating transaction status in storage
//! - Triggering notifications on status changes
use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Data, *};
use eyre::Result;
use tracing::{debug, instrument};

use std::{sync::Arc, time::Duration};

use crate::{
    domain::{get_relayer_transaction, get_transaction_by_id, is_final_state, Transaction},
    jobs::{job_producer::JobProducerTrait, Job, TransactionStatusCheck},
    models::{DefaultAppState, TransactionRepoModel},
    observability::request_id::set_request_id,
    utils::calculate_scheduled_timestamp,
};

// Status check backoff schedule
const STATUS_CHECK_BACKOFF_SCHEDULE: &[(u32, u64)] = &[
    (0, 5),  // First retry: 5 seconds
    (1, 10), // Second retry: 10 seconds
    (2, 20), // Third retry: 20 seconds
    (3, 30), // Fourth retry: 30 seconds
    (4, 45), // Fifth retry: 45 seconds
];
const STATUS_CHECK_MAX_BACKOFF_SECONDS: u64 = 60; // Max: 60 seconds

/// Calculates exponential backoff delay for status checks based on retry count
fn calculate_status_check_backoff(retry_count: u32) -> Duration {
    let seconds = STATUS_CHECK_BACKOFF_SCHEDULE
        .iter()
        .find(|(count, _)| *count == retry_count)
        .map(|(_, delay)| *delay)
        .unwrap_or(STATUS_CHECK_MAX_BACKOFF_SECONDS);

    Duration::from_secs(seconds)
}

#[cfg(test)]
use crate::models::NetworkType;

#[instrument(
    level = "debug",
    skip(job, state),
    fields(
        request_id = ?job.request_id,
        job_id = %job.message_id,
        job_type = %job.job_type.to_string(),
        attempt = %attempt.current(),
        tx_id = %job.data.transaction_id,
        relayer_id = %job.data.relayer_id,
    )
)]
pub async fn transaction_status_handler(
    job: Job<TransactionStatusCheck>,
    state: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
) -> Result<(), Error> {
    if let Some(request_id) = job.request_id.clone() {
        set_request_id(request_id);
    }

    debug!(
        "handling transaction status check for tx_id {}",
        job.data.transaction_id
    );

    let result = handle_request(job.data.clone(), state.clone()).await;

    handle_status_check_result(result, &job, &state)
}

/// Handles status check results with exponential backoff retry logic.
///
/// # Retry Strategy
/// - If transaction is in final state → Job completes successfully
/// - If error occurred → Retry with backoff (let Apalis handle retry)
/// - If transaction still not final → Schedule next check with exponential backoff
fn handle_status_check_result(
    result: Result<TransactionRepoModel>,
    job: &Job<TransactionStatusCheck>,
    state: &Data<ThinData<DefaultAppState>>,
) -> Result<(), Error> {
    match result {
        Ok(updated_tx) => {
            // Check if transaction reached final state
            if is_final_state(&updated_tx.status) {
                debug!(
                    tx_id = %updated_tx.id,
                    status = ?updated_tx.status,
                    "transaction reached final state, status check complete"
                );
                Ok(())
            } else {
                // Transaction still processing, schedule retry with backoff

                // Extract retry count from metadata
                let retry_count = job
                    .data
                    .metadata
                    .as_ref()
                    .and_then(|m| m.get("retry_count"))
                    .and_then(|c| c.parse::<u32>().ok())
                    .unwrap_or(0);

                let delay = calculate_status_check_backoff(retry_count);

                debug!(
                    tx_id = %updated_tx.id,
                    status = ?updated_tx.status,
                    retry_count = retry_count,
                    next_check_delay_secs = delay.as_secs(),
                    "transaction not in final state, scheduling retry with backoff"
                );

                // Schedule next status check with backoff
                let mut metadata = job.data.metadata.clone().unwrap_or_default();
                metadata.insert("retry_count".to_string(), (retry_count + 1).to_string());

                let next_check = TransactionStatusCheck {
                    transaction_id: updated_tx.id.clone(),
                    relayer_id: updated_tx.relayer_id.clone(),
                    network_type: job.data.network_type,
                    metadata: Some(metadata),
                };

                let scheduled_at = calculate_scheduled_timestamp(delay.as_secs() as i64);

                // Schedule the job asynchronously
                tokio::spawn({
                    let job_producer = state.job_producer.clone();
                    async move {
                        if let Err(e) = job_producer
                            .produce_check_transaction_status_job(next_check, Some(scheduled_at))
                            .await
                        {
                            tracing::warn!(error = ?e, "failed to schedule next status check with backoff");
                        }
                    }
                });

                // Complete this job successfully (next check is scheduled)
                Ok(())
            }
        }
        Err(e) => {
            // Error occurred, retry immediately
            Err(Error::Failed(Arc::new(format!("{e}").into())))
        }
    }
}

async fn handle_request(
    status_request: TransactionStatusCheck,
    state: Data<ThinData<DefaultAppState>>,
) -> Result<TransactionRepoModel> {
    let relayer_transaction =
        get_relayer_transaction(status_request.relayer_id.clone(), &state).await?;

    let transaction = get_transaction_by_id(status_request.transaction_id.clone(), &state).await?;

    // Avoid scheduling status checks for unsubmitted transactions
    // No on-chain hashes yet - submit handler will check after submission
    use crate::domain::is_unsubmitted_transaction;
    if is_unsubmitted_transaction(&transaction.status) {
        debug!(
            tx_id = %transaction.id,
            status = ?transaction.status,
            "skipping status check for unsubmitted transaction"
        );
        return Ok(transaction);
    }

    let updated_transaction = relayer_transaction
        .handle_transaction_status(transaction)
        .await?;

    debug!(
        "status check handled successfully for tx_id {}",
        status_request.transaction_id
    );

    Ok(updated_transaction)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TransactionStatus;
    use crate::utils::mocks::mockutils::create_mock_transaction;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_status_check_job_validation() {
        // Create a basic status check job
        let check_job = TransactionStatusCheck::new("tx123", "relayer-1", NetworkType::Evm);
        let job = Job::new(crate::jobs::JobType::TransactionStatusCheck, check_job);

        // Validate the job data
        assert_eq!(job.data.transaction_id, "tx123");
        assert_eq!(job.data.relayer_id, "relayer-1");
        assert!(job.data.metadata.is_none());
    }

    #[tokio::test]
    async fn test_status_check_with_metadata() {
        // Create a job with retry metadata
        let mut metadata = HashMap::new();
        metadata.insert("retry_count".to_string(), "2".to_string());
        metadata.insert("last_status".to_string(), "pending".to_string());

        let check_job = TransactionStatusCheck::new("tx123", "relayer-1", NetworkType::Evm)
            .with_metadata(metadata.clone());

        // Validate the metadata
        assert!(check_job.metadata.is_some());
        let job_metadata = check_job.metadata.unwrap();
        assert_eq!(job_metadata.get("retry_count").unwrap(), "2");
        assert_eq!(job_metadata.get("last_status").unwrap(), "pending");
    }

    // Legacy tests for old immediate-retry behavior
    #[cfg(ignore)]
    mod handle_status_check_result_tests {
        use super::*;

        #[test]
        fn test_final_state_confirmed_returns_ok() {
            let mut tx = create_mock_transaction();
            tx.status = TransactionStatus::Confirmed;
            let result = Ok(tx);

            let check_result = handle_status_check_result(result);

            assert!(
                check_result.is_ok(),
                "Should return Ok for Confirmed (final) state"
            );
        }

        #[test]
        fn test_final_state_failed_returns_ok() {
            let mut tx = create_mock_transaction();
            tx.status = TransactionStatus::Failed;
            let result = Ok(tx);

            let check_result = handle_status_check_result(result);

            assert!(
                check_result.is_ok(),
                "Should return Ok for Failed (final) state"
            );
        }

        #[test]
        fn test_final_state_expired_returns_ok() {
            let mut tx = create_mock_transaction();
            tx.status = TransactionStatus::Expired;
            let result = Ok(tx);

            let check_result = handle_status_check_result(result);

            assert!(
                check_result.is_ok(),
                "Should return Ok for Expired (final) state"
            );
        }

        #[test]
        fn test_final_state_canceled_returns_ok() {
            let mut tx = create_mock_transaction();
            tx.status = TransactionStatus::Canceled;
            let result = Ok(tx);

            let check_result = handle_status_check_result(result);

            assert!(
                check_result.is_ok(),
                "Should return Ok for Canceled (final) state"
            );
        }

        #[test]
        fn test_non_final_state_pending_returns_error() {
            let mut tx = create_mock_transaction();
            tx.status = TransactionStatus::Pending;
            let result = Ok(tx);

            let check_result = handle_status_check_result(result);

            assert!(
                check_result.is_err(),
                "Should return Err for Pending (non-final) state to trigger retry"
            );
        }

        #[test]
        fn test_non_final_state_sent_returns_error() {
            let mut tx = create_mock_transaction();
            tx.status = TransactionStatus::Sent;
            let result = Ok(tx);

            let check_result = handle_status_check_result(result);

            assert!(
                check_result.is_err(),
                "Should return Err for Sent (non-final) state to trigger retry"
            );
        }

        #[test]
        fn test_non_final_state_submitted_returns_error() {
            let mut tx = create_mock_transaction();
            tx.status = TransactionStatus::Submitted;
            let result = Ok(tx);

            let check_result = handle_status_check_result(result);

            assert!(
                check_result.is_err(),
                "Should return Err for Submitted (non-final) state to trigger retry"
            );
        }

        #[test]
        fn test_non_final_state_mined_returns_error() {
            let mut tx = create_mock_transaction();
            tx.status = TransactionStatus::Mined;
            let result = Ok(tx);

            let check_result = handle_status_check_result(result);

            assert!(
                check_result.is_err(),
                "Should return Err for Mined (non-final) state to trigger retry"
            );
        }

        #[test]
        fn test_error_result_returns_error() {
            let result: Result<TransactionRepoModel> =
                Err(eyre::eyre!("Network timeout during status check"));

            let check_result = handle_status_check_result(result);

            assert!(
                check_result.is_err(),
                "Should return Err when original result is an error"
            );
        }

        #[test]
        fn test_error_message_propagation() {
            let error_message = "RPC call failed: connection timeout";
            let result: Result<TransactionRepoModel> = Err(eyre::eyre!(error_message));

            let check_result = handle_status_check_result(result);

            match check_result {
                Err(Error::Failed(arc)) => {
                    let err_string = arc.to_string();
                    assert!(
                        err_string.contains(error_message),
                        "Error message should contain original error: {}",
                        err_string
                    );
                }
                _ => panic!("Expected Error::Failed"),
            }
        }

        #[test]
        fn test_non_final_state_error_message() {
            let mut tx = create_mock_transaction();
            tx.status = TransactionStatus::Submitted;
            let result = Ok(tx);

            let check_result = handle_status_check_result(result);

            match check_result {
                Err(Error::Failed(arc)) => {
                    let err_string = arc.to_string();
                    assert!(
                        err_string.contains("not in final state"),
                        "Error message should indicate non-final state: {}",
                        err_string
                    );
                    assert!(
                        err_string.contains("Submitted"),
                        "Error message should mention the status: {}",
                        err_string
                    );
                }
                _ => panic!("Expected Error::Failed for non-final state"),
            }
        }
    }

    /// Tests for exponential backoff calculation
    mod backoff_tests {
        use super::*;

        #[test]
        fn test_calculate_status_check_backoff() {
            // Test the backoff schedule
            assert_eq!(calculate_status_check_backoff(0), Duration::from_secs(5));
            assert_eq!(calculate_status_check_backoff(1), Duration::from_secs(10));
            assert_eq!(calculate_status_check_backoff(2), Duration::from_secs(20));
            assert_eq!(calculate_status_check_backoff(3), Duration::from_secs(30));
            assert_eq!(calculate_status_check_backoff(4), Duration::from_secs(45));

            // Test max backoff for higher retry counts
            assert_eq!(calculate_status_check_backoff(5), Duration::from_secs(60));
            assert_eq!(calculate_status_check_backoff(10), Duration::from_secs(60));
            assert_eq!(calculate_status_check_backoff(100), Duration::from_secs(60));
        }

        #[test]
        fn test_status_check_with_retry_metadata() {
            let mut metadata = HashMap::new();
            metadata.insert("retry_count".to_string(), "3".to_string());

            let check_job = TransactionStatusCheck::new("tx123", "relayer-1", NetworkType::Stellar)
                .with_metadata(metadata.clone());

            assert!(check_job.metadata.is_some());
            let job_metadata = check_job.metadata.unwrap();
            assert_eq!(job_metadata.get("retry_count").unwrap(), "3");

            // Verify the backoff calculation for this retry count
            let backoff = calculate_status_check_backoff(3);
            assert_eq!(backoff, Duration::from_secs(30));
        }

        #[test]
        fn test_retry_count_parsing_from_metadata() {
            // Test parsing valid retry count
            let mut metadata = HashMap::new();
            metadata.insert("retry_count".to_string(), "2".to_string());

            let retry_count: u32 = metadata
                .get("retry_count")
                .and_then(|c| c.parse().ok())
                .unwrap_or(0);

            assert_eq!(retry_count, 2);

            // Test parsing invalid retry count (should default to 0)
            let mut invalid_metadata = HashMap::new();
            invalid_metadata.insert("retry_count".to_string(), "invalid".to_string());

            let retry_count: u32 = invalid_metadata
                .get("retry_count")
                .and_then(|c| c.parse().ok())
                .unwrap_or(0);

            assert_eq!(retry_count, 0);

            // Test missing retry count (should default to 0)
            let empty_metadata: HashMap<String, String> = HashMap::new();
            let retry_count: u32 = empty_metadata
                .get("retry_count")
                .and_then(|c| c.parse().ok())
                .unwrap_or(0);

            assert_eq!(retry_count, 0);
        }

        #[test]
        fn test_backoff_schedule_progression() {
            // Verify backoff increases exponentially
            let backoffs: Vec<u64> = (0..6)
                .map(|i| calculate_status_check_backoff(i).as_secs())
                .collect();

            // Each step should be >= previous (except max)
            for i in 0..backoffs.len() - 1 {
                assert!(
                    backoffs[i + 1] >= backoffs[i],
                    "Backoff should increase or stay at max: {} -> {}",
                    backoffs[i],
                    backoffs[i + 1]
                );
            }

            // Verify we reach max and stay there
            assert_eq!(backoffs[5], STATUS_CHECK_MAX_BACKOFF_SECONDS);
        }
    }
}
