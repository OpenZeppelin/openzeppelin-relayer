//! Transaction status monitoring handler.
//!
//! Monitors the status of submitted transactions by:
//! - Checking transaction status on the network
//! - Updating transaction status in storage
//! - Tracking failure counts for circuit breaker decisions
//! - Triggering notifications on status changes
use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Data, *};
use eyre::Result;
use std::sync::Arc;
use tracing::{debug, instrument, warn};

use crate::{
    constants::{get_max_consecutive_status_failures, get_max_total_status_failures},
    domain::{get_relayer_transaction, get_transaction_by_id, is_final_state, Transaction},
    jobs::{
        read_counter_from_metadata, Job, StatusCheckContext, TransactionStatusCheck,
        META_CONSECUTIVE_FAILURES, META_TOTAL_FAILURES,
    },
    models::{DefaultAppState, NetworkType, TransactionRepoModel},
    observability::request_id::set_request_id,
};

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
    mut job: Job<TransactionStatusCheck>,
    state: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
) -> Result<(), Error> {
    if let Some(request_id) = job.request_id.clone() {
        set_request_id(request_id);
    }

    // Read current failure counters from job metadata
    let consecutive_failures =
        read_counter_from_metadata(&job.data.metadata, META_CONSECUTIVE_FAILURES);
    let total_failures = read_counter_from_metadata(&job.data.metadata, META_TOTAL_FAILURES);

    // Get network type and max failures (both consecutive and total)
    let network_type = job.data.network_type.unwrap_or(NetworkType::Evm);
    let max_consecutive = get_max_consecutive_status_failures(network_type);
    let max_total = get_max_total_status_failures(network_type);

    debug!(
        tx_id = %job.data.transaction_id,
        consecutive_failures,
        total_failures,
        max_consecutive,
        max_total,
        attempt = attempt.current(),
        "handling transaction status check"
    );

    // Build circuit breaker context with attempt count for total retries
    let total_retries = attempt.current() as u32;
    let context = StatusCheckContext::new(
        consecutive_failures,
        total_failures,
        total_retries,
        max_consecutive,
        max_total,
        network_type,
    );

    // Execute status check with context
    let result = handle_request(job.data.clone(), state, context).await;

    // Handle result and update metadata for next retry
    handle_result(&mut job, result, consecutive_failures, total_failures)
}

/// Handles status check results with circuit breaker tracking.
///
/// # Strategy
/// - If transaction is in final state → Return Ok (job completes)
/// - If success but not final → Reset consecutive to 0, return Err (Apalis retries)
/// - If error → Increment counters, return Err (Apalis retries)
///
/// Metadata updates persist across Apalis retries, enabling proper failure tracking.
fn handle_result(
    job: &mut Job<TransactionStatusCheck>,
    result: Result<TransactionRepoModel>,
    consecutive_failures: u32,
    total_failures: u32,
) -> Result<(), Error> {
    match result {
        Ok(tx) if is_final_state(&tx.status) => {
            // Transaction reached final state - job complete
            debug!(
                tx_id = %tx.id,
                status = ?tx.status,
                consecutive_failures,
                total_failures,
                "transaction in final state, status check complete"
            );
            Ok(())
        }
        Ok(tx) => {
            // Success but not final - RESET consecutive counter, keep total unchanged
            debug!(
                tx_id = %tx.id,
                status = ?tx.status,
                "transaction not in final state, resetting consecutive failures"
            );

            update_job_metadata(job, 0, total_failures);

            // Return error to trigger Apalis retry
            Err(Error::Failed(Arc::new(
                format!(
                    "transaction status: {:?} - not in final state, retrying",
                    tx.status
                )
                .into(),
            )))
        }
        Err(e) => {
            // Error occurred - INCREMENT both counters
            let new_consecutive = consecutive_failures.saturating_add(1);
            let new_total = total_failures.saturating_add(1);

            warn!(
                error = %e,
                consecutive_failures = new_consecutive,
                total_failures = new_total,
                "status check failed, incrementing failure counters"
            );

            update_job_metadata(job, new_consecutive, new_total);

            // Return error to trigger Apalis retry
            Err(Error::Failed(Arc::new(format!("{e}").into())))
        }
    }
}

/// Updates job metadata with new failure counters.
fn update_job_metadata(job: &mut Job<TransactionStatusCheck>, consecutive: u32, total: u32) {
    let mut metadata = job.data.metadata.clone().unwrap_or_default();
    metadata.insert(
        META_CONSECUTIVE_FAILURES.to_string(),
        consecutive.to_string(),
    );
    metadata.insert(META_TOTAL_FAILURES.to_string(), total.to_string());
    job.data.metadata = Some(metadata);
}

async fn handle_request(
    status_request: TransactionStatusCheck,
    state: Data<ThinData<DefaultAppState>>,
    context: StatusCheckContext,
) -> Result<TransactionRepoModel> {
    let relayer_transaction =
        get_relayer_transaction(status_request.relayer_id.clone(), &state).await?;

    let transaction = get_transaction_by_id(status_request.transaction_id.clone(), &state).await?;

    let updated_transaction = relayer_transaction
        .handle_transaction_status(transaction, Some(context))
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
    use crate::jobs::{JobType, META_CONSECUTIVE_FAILURES, META_TOTAL_FAILURES};
    use crate::models::TransactionStatus;
    use crate::utils::mocks::mockutils::create_mock_transaction;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_status_check_job_validation() {
        let check_job = TransactionStatusCheck::new("tx123", "relayer-1", NetworkType::Evm);
        let job = Job::new(JobType::TransactionStatusCheck, check_job);

        assert_eq!(job.data.transaction_id, "tx123");
        assert_eq!(job.data.relayer_id, "relayer-1");
        assert!(job.data.metadata.is_none());
    }

    #[tokio::test]
    async fn test_status_check_with_metadata() {
        let mut metadata = HashMap::new();
        metadata.insert("retry_count".to_string(), "2".to_string());
        metadata.insert("last_status".to_string(), "pending".to_string());

        let check_job = TransactionStatusCheck::new("tx123", "relayer-1", NetworkType::Evm)
            .with_metadata(metadata.clone());

        assert!(check_job.metadata.is_some());
        let job_metadata = check_job.metadata.unwrap();
        assert_eq!(job_metadata.get("retry_count").unwrap(), "2");
        assert_eq!(job_metadata.get("last_status").unwrap(), "pending");
    }

    #[test]
    fn test_update_job_metadata() {
        let check_job = TransactionStatusCheck::new("tx123", "relayer-1", NetworkType::Evm);
        let mut job = Job::new(JobType::TransactionStatusCheck, check_job);

        update_job_metadata(&mut job, 5, 10);

        let metadata = job.data.metadata.unwrap();
        assert_eq!(metadata.get(META_CONSECUTIVE_FAILURES).unwrap(), "5");
        assert_eq!(metadata.get(META_TOTAL_FAILURES).unwrap(), "10");
    }

    #[test]
    fn test_update_job_metadata_preserves_existing() {
        let mut original_metadata = HashMap::new();
        original_metadata.insert("custom_key".to_string(), "custom_value".to_string());

        let check_job = TransactionStatusCheck::new("tx123", "relayer-1", NetworkType::Evm)
            .with_metadata(original_metadata);
        let mut job = Job::new(JobType::TransactionStatusCheck, check_job);

        update_job_metadata(&mut job, 3, 7);

        let metadata = job.data.metadata.unwrap();
        assert_eq!(metadata.get(META_CONSECUTIVE_FAILURES).unwrap(), "3");
        assert_eq!(metadata.get(META_TOTAL_FAILURES).unwrap(), "7");
        assert_eq!(metadata.get("custom_key").unwrap(), "custom_value");
    }

    mod handle_result_tests {
        use super::*;

        fn create_test_job() -> Job<TransactionStatusCheck> {
            let check_job = TransactionStatusCheck::new("tx123", "relayer-1", NetworkType::Evm);
            Job::new(JobType::TransactionStatusCheck, check_job)
        }

        #[test]
        fn test_final_state_returns_ok() {
            let mut job = create_test_job();
            let mut tx = create_mock_transaction();
            tx.status = TransactionStatus::Confirmed;

            let result = handle_result(&mut job, Ok(tx), 5, 10);

            assert!(result.is_ok());
        }

        #[test]
        fn test_non_final_state_resets_consecutive() {
            let mut job = create_test_job();
            let mut tx = create_mock_transaction();
            tx.status = TransactionStatus::Submitted;

            let result = handle_result(&mut job, Ok(tx), 5, 10);

            assert!(result.is_err());
            let metadata = job.data.metadata.unwrap();
            assert_eq!(metadata.get(META_CONSECUTIVE_FAILURES).unwrap(), "0");
            assert_eq!(metadata.get(META_TOTAL_FAILURES).unwrap(), "10");
        }

        #[test]
        fn test_error_increments_counters() {
            let mut job = create_test_job();
            let error: Result<TransactionRepoModel> = Err(eyre::eyre!("RPC error"));

            let result = handle_result(&mut job, error, 5, 10);

            assert!(result.is_err());
            let metadata = job.data.metadata.unwrap();
            assert_eq!(metadata.get(META_CONSECUTIVE_FAILURES).unwrap(), "6");
            assert_eq!(metadata.get(META_TOTAL_FAILURES).unwrap(), "11");
        }
    }

    mod context_tests {
        use super::*;

        #[test]
        fn test_context_should_force_finalize_below_both_thresholds() {
            // consecutive: 5 < 15, total: 10 < 45
            let ctx = StatusCheckContext::new(5, 10, 20, 15, 45, NetworkType::Evm);
            assert!(!ctx.should_force_finalize());
        }

        #[test]
        fn test_context_should_force_finalize_consecutive_at_threshold() {
            // consecutive: 15 >= 15 (triggers), total: 20 < 45
            let ctx = StatusCheckContext::new(15, 20, 30, 15, 45, NetworkType::Evm);
            assert!(ctx.should_force_finalize());
        }

        #[test]
        fn test_context_should_force_finalize_total_at_threshold() {
            // consecutive: 5 < 15, total: 45 >= 45 (triggers - safety net)
            let ctx = StatusCheckContext::new(5, 45, 50, 15, 45, NetworkType::Evm);
            assert!(ctx.should_force_finalize());
        }
    }

    mod final_state_tests {
        use super::*;

        fn verify_final_state(status: TransactionStatus) {
            assert!(
                is_final_state(&status),
                "{:?} should be a final state",
                status
            );
        }

        fn verify_non_final_state(status: TransactionStatus) {
            assert!(
                !is_final_state(&status),
                "{:?} should NOT be a final state",
                status
            );
        }

        #[test]
        fn test_confirmed_is_final() {
            verify_final_state(TransactionStatus::Confirmed);
        }

        #[test]
        fn test_failed_is_final() {
            verify_final_state(TransactionStatus::Failed);
        }

        #[test]
        fn test_expired_is_final() {
            verify_final_state(TransactionStatus::Expired);
        }

        #[test]
        fn test_canceled_is_final() {
            verify_final_state(TransactionStatus::Canceled);
        }

        #[test]
        fn test_pending_is_not_final() {
            verify_non_final_state(TransactionStatus::Pending);
        }

        #[test]
        fn test_sent_is_not_final() {
            verify_non_final_state(TransactionStatus::Sent);
        }

        #[test]
        fn test_submitted_is_not_final() {
            verify_non_final_state(TransactionStatus::Submitted);
        }

        #[test]
        fn test_mined_is_not_final() {
            verify_non_final_state(TransactionStatus::Mined);
        }
    }
}
