use eyre::Report;
use tracing::{debug, error, info, warn};

use crate::{
    models::{TransactionRepoModel, TransactionStatus, TransactionUpdateRequest},
    observability::request_id::get_request_id,
    queues::{HandlerError, WorkerContext},
    repositories::{transaction::TransactionRepository, Repository},
};

mod transaction_request_handler;
pub use transaction_request_handler::*;

mod transaction_submission_handler;
pub use transaction_submission_handler::*;

mod notification_handler;
pub use notification_handler::*;

mod transaction_status_handler;
pub use transaction_status_handler::*;

mod token_swap_request_handler;
pub use token_swap_request_handler::*;

mod transaction_cleanup_handler;
pub use transaction_cleanup_handler::*;

mod system_cleanup_handler;
pub use system_cleanup_handler::*;

// Handles job results for simple handlers (no transaction state management).
//
// Used by: notification_handler, solana_swap_request_handler, transaction_cleanup_handler
//
// # Retry Strategy
// - On success: Job completes
// - On error: Retry until max_attempts reached
// - At max_attempts: Abort job
mod relayer_health_check_handler;
pub use relayer_health_check_handler::*;

pub fn handle_result(
    result: Result<(), Report>,
    ctx: &WorkerContext,
    job_type: &str,
    max_attempts: usize,
) -> Result<(), HandlerError> {
    if result.is_ok() {
        debug!(
            job_type = %job_type,
            request_id = ?get_request_id(),
            "request handled successfully"
        );
        return Ok(());
    }

    let err = result.as_ref().unwrap_err();
    warn!(
        job_type = %job_type,
        request_id = ?get_request_id(),
        error = %err,
        attempt = %ctx.attempt,
        max_attempts = %max_attempts,
        "request failed"
    );

    if ctx.attempt >= max_attempts {
        error!(
            job_type = %job_type,
            request_id = ?get_request_id(),
            max_attempts = %max_attempts,
            "max attempts reached, failing job"
        );
        return Err(HandlerError::Abort("Failed to handle request".into()));
    }

    Err(HandlerError::Retry(
        "Failed to handle request. Retrying".into(),
    ))
}

/// Mark a transaction as `Failed` once retries are exhausted.
///
/// Bridges apalis's job-attempt counter to the TransactionRepository
/// row state. Without this, a tx whose `prepare_transaction` fails
/// permanently has no path from `Pending` → `Failed` — the job dies in
/// apalis but the row stays at the pre-prepare status forever, and the
/// API surfaces a stuck-pending tx.
///
/// No-op when not on the final attempt; the next retry may succeed.
pub async fn mark_tx_failed_on_final_attempt<TR>(
    tx_id: &str,
    transaction_repository: &TR,
    err: &str,
    ctx: &WorkerContext,
    max_attempts: usize,
) where
    TR: TransactionRepository + Repository<TransactionRepoModel, String>,
{
    if ctx.attempt < max_attempts {
        return;
    }

    let update = TransactionUpdateRequest {
        status: Some(TransactionStatus::Failed),
        status_reason: Some(format!("Job aborted after {max_attempts} attempts: {err}")),
        ..Default::default()
    };

    match transaction_repository
        .partial_update(tx_id.to_string(), update)
        .await
    {
        Ok(_) => info!(
            tx_id = %tx_id,
            "marked tx as Failed after exhausting job retries"
        ),
        Err(e) => error!(
            tx_id = %tx_id,
            error = %e,
            "failed to mark tx as Failed on final-attempt abort"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_result_success() {
        let result: Result<(), Report> = Ok(());
        let ctx = WorkerContext::new(0, "test-task".into());

        let handled = handle_result(result, &ctx, "test_job", 3);
        assert!(handled.is_ok());
    }

    #[test]
    fn test_handle_result_retry() {
        let result: Result<(), Report> = Err(Report::msg("Test error"));
        let ctx = WorkerContext::new(0, "test-task".into());

        let handled = handle_result(result, &ctx, "test_job", 3);

        assert!(handled.is_err());
        assert!(matches!(handled, Err(HandlerError::Retry(_))));
    }

    #[test]
    fn test_handle_result_abort() {
        let result: Result<(), Report> = Err(Report::msg("Test error"));
        let ctx = WorkerContext::new(3, "test-task".into());

        let handled = handle_result(result, &ctx, "test_job", 3);

        assert!(handled.is_err());
        assert!(matches!(handled, Err(HandlerError::Abort(_))));
    }

    #[test]
    fn test_handle_result_max_attempts_exceeded() {
        let result: Result<(), Report> = Err(Report::msg("Test error"));
        let ctx = WorkerContext::new(5, "test-task".into());

        let handled = handle_result(result, &ctx, "test_job", 3);

        assert!(handled.is_err());
        assert!(matches!(handled, Err(HandlerError::Abort(_))));
    }

    #[tokio::test]
    async fn test_mark_tx_failed_no_op_before_final_attempt() {
        use crate::repositories::TransactionRepositoryStorage;
        let repo = TransactionRepositoryStorage::new_in_memory();
        let tx = crate::utils::mocks::mockutils::create_mock_transaction();
        let tx_id = tx.id.clone();
        let initial_status = tx.status.clone();
        repo.create(tx).await.unwrap();

        let ctx = WorkerContext::new(0, "test-task".into());
        mark_tx_failed_on_final_attempt(&tx_id, &repo, "boom", &ctx, 3).await;

        let after = repo.get_by_id(tx_id).await.unwrap();
        assert_eq!(
            after.status, initial_status,
            "should not modify status before final attempt"
        );
        assert!(after.status_reason.is_none());
    }

    #[tokio::test]
    async fn test_mark_tx_failed_on_final_attempt_writes_failed_status() {
        use crate::repositories::TransactionRepositoryStorage;
        let repo = TransactionRepositoryStorage::new_in_memory();
        let tx = crate::utils::mocks::mockutils::create_mock_transaction();
        let tx_id = tx.id.clone();
        repo.create(tx).await.unwrap();

        let ctx = WorkerContext::new(3, "test-task".into());
        mark_tx_failed_on_final_attempt(&tx_id, &repo, "build error", &ctx, 3).await;

        let after = repo.get_by_id(tx_id).await.unwrap();
        assert!(matches!(after.status, TransactionStatus::Failed));
        let reason = after.status_reason.expect("status_reason should be set");
        assert!(reason.contains("Job aborted after 3 attempts"));
        assert!(reason.contains("build error"));
    }
}
