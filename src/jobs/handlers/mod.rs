use std::sync::Arc;

use apalis::prelude::{Attempt, Error};
use eyre::Report;

mod transaction_request_handler;
use log::info;
pub use transaction_request_handler::*;

mod transaction_submission_handler;
pub use transaction_submission_handler::*;

mod notification_handler;
pub use notification_handler::*;

mod transaction_status_handler;
pub use transaction_status_handler::*;

mod solana_swap_request_handler;
pub use solana_swap_request_handler::*;

mod transaction_cleanup_handler;
pub use transaction_cleanup_handler::*;

pub fn handle_result(
    result: Result<(), Report>,
    attempt: Attempt,
    job_type: &str,
    max_attempts: usize,
) -> Result<(), Error> {
    if result.is_ok() {
        info!("{} request handled successfully", job_type);
        return Ok(());
    }
    info!("{} request failed: {:?}", job_type, result);

    if attempt.current() >= max_attempts {
        info!("Max attempts ({}) reached, failing job", max_attempts);
        Err(Error::Abort(Arc::new("Failed to handle request".into())))?
    }

    Err(Error::Failed(Arc::new(
        "Failed to handle request. Retrying".into(),
    )))?
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
        match handled {
            Err(Error::Failed(_)) => {
                // This is the expected error type for a retry
            }
            _ => panic!("Expected Failed error for retry"),
        }
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
        match handled {
            Err(Error::Abort(_)) => {
                // This is the expected error type for an abort
            }
            _ => panic!("Expected Abort error for max attempts"),
        }
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
        match handled {
            Err(Error::Abort(_)) => {
                // This is the expected error type for exceeding max attempts
            }
            _ => panic!("Expected Abort error for exceeding max attempts"),
        }
    }
}
