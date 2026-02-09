use eyre::Report;
use tracing::{debug, error, warn};

use crate::{
    observability::request_id::get_request_id,
    queues::types::{HandlerError, WorkerContext},
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
}
