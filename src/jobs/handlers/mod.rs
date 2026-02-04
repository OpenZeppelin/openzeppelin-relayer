use apalis::prelude::{AbortError, Attempt, BoxDynError};
use eyre::Report;
use tracing::{debug, error, warn};

use crate::observability::request_id::get_request_id;

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
    attempt: Attempt,
    job_type: &str,
    max_attempts: usize,
) -> Result<(), BoxDynError> {
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
        attempt = %attempt.current(),
        max_attempts = %max_attempts,
        "request failed"
    );

    if attempt.current() >= max_attempts {
        error!(
            job_type = %job_type,
            request_id = ?get_request_id(),
            max_attempts = %max_attempts,
            "max attempts reached, failing job"
        );
        return Err(AbortError::new("Failed to handle request").into());
    }

    // Return error to trigger retry - apalis will retry on any non-abort error
    Err(eyre::eyre!("Failed to handle request. Retrying").into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use apalis::prelude::Attempt;

    #[test]
    fn test_handle_result_success() {
        let result: Result<(), Report> = Ok(());
        let attempt = Attempt::default();

        let handled = handle_result(result, attempt, "test_job", 3);
        assert!(handled.is_ok());
    }

    #[test]
    fn test_handle_result_retry() {
        let result: Result<(), Report> = Err(Report::msg("Test error"));
        let attempt = Attempt::default();

        let handled = handle_result(result, attempt, "test_job", 3);

        assert!(handled.is_err());
        // In apalis 1.0.0-rc.3, any non-AbortError triggers retry
        // The error should not be an AbortError (which would abort the job)
        let err = handled.unwrap_err();
        assert!(!err.is::<AbortError>(), "Expected non-AbortError for retry");
    }

    #[test]
    fn test_handle_result_abort() {
        let result: Result<(), Report> = Err(Report::msg("Test error"));
        let attempt = Attempt::default();
        for _ in 0..3 {
            attempt.increment();
        }

        let handled = handle_result(result, attempt, "test_job", 3);

        assert!(handled.is_err());
        // AbortError is returned when max attempts reached
        let err = handled.unwrap_err();
        assert!(
            err.is::<AbortError>(),
            "Expected AbortError for max attempts"
        );
    }

    #[test]
    fn test_handle_result_max_attempts_exceeded() {
        let result: Result<(), Report> = Err(Report::msg("Test error"));
        let attempt = Attempt::default();
        for _ in 0..5 {
            attempt.increment();
        }

        let handled = handle_result(result, attempt, "test_job", 3);

        assert!(handled.is_err());
        // AbortError is returned when max attempts exceeded
        let err = handled.unwrap_err();
        assert!(
            err.is::<AbortError>(),
            "Expected AbortError for exceeding max attempts"
        );
    }
}
