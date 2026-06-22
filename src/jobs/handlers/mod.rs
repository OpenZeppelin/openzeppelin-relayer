use eyre::Report;
use tracing::{debug, error, warn};

use crate::{
    models::{ApiError, RepositoryError},
    observability::request_id::get_request_id,
    queues::{HandlerError, WorkerContext},
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

/// Returns `true` when retrying the job can never succeed, so it should be
/// aborted immediately instead of consuming the retry budget.
///
/// A `NotFound` for the entity a job operates on (e.g. a transaction that was
/// deleted or expired from the repository) will never reappear by re-running
/// the same job. On standard SQS queues this is critical: each retry
/// re-enqueues a fresh message with `ApproximateReceiveCount` reset to 1, so
/// the attempt counter never advances and the job would otherwise loop
/// forever, also bypassing the SQS dead-letter redrive policy.
fn is_terminal_error(err: &Report) -> bool {
    if let Some(api_err) = err.downcast_ref::<ApiError>() {
        if matches!(api_err, ApiError::NotFound(_)) {
            return true;
        }
    }
    if let Some(repo_err) = err.downcast_ref::<RepositoryError>() {
        if matches!(repo_err, RepositoryError::NotFound(_)) {
            return true;
        }
    }
    false
}

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

    if is_terminal_error(err) {
        error!(
            job_type = %job_type,
            request_id = ?get_request_id(),
            error = %err,
            "terminal error (entity not found), aborting job without retry"
        );
        return Err(HandlerError::Abort(
            "Terminal error: target entity not found, not retryable".into(),
        ));
    }

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

    #[test]
    fn test_handle_result_terminal_api_not_found_aborts_at_first_attempt() {
        // A missing entity will never appear by retrying the same job, so a
        // NotFound must abort immediately instead of burning the whole retry
        // budget (or looping forever on standard SQS queues where attempt stays 0).
        let result: Result<(), Report> = Err(Report::new(crate::models::ApiError::NotFound(
            "Transaction with ID 4af0a83b not found".into(),
        )));
        let ctx = WorkerContext::new(0, "test-task".into());

        let handled = handle_result(result, &ctx, "Transaction Request", 5);

        assert!(matches!(handled, Err(HandlerError::Abort(_))));
    }

    #[test]
    fn test_handle_result_terminal_repository_not_found_aborts_at_first_attempt() {
        let result: Result<(), Report> = Err(Report::new(
            crate::models::RepositoryError::NotFound("relayer xyz not found".into()),
        ));
        let ctx = WorkerContext::new(0, "test-task".into());

        let handled = handle_result(result, &ctx, "Transaction Request", 5);

        assert!(matches!(handled, Err(HandlerError::Abort(_))));
    }

    #[test]
    fn test_handle_result_non_terminal_error_still_retries() {
        // Regression guard: transient errors must keep retrying below max.
        let result: Result<(), Report> = Err(Report::new(
            crate::models::RepositoryError::ConnectionError("redis timeout".into()),
        ));
        let ctx = WorkerContext::new(0, "test-task".into());

        let handled = handle_result(result, &ctx, "Transaction Request", 5);

        assert!(matches!(handled, Err(HandlerError::Retry(_))));
    }
}
