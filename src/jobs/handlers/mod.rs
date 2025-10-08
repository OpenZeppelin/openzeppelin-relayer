use std::sync::Arc;

use apalis::prelude::{Attempt, Error};
use eyre::Report;
use tracing::{debug, error, warn};

use crate::{
    models::{TransactionStatus, TransactionUpdateRequest},
    repositories::TransactionRepository,
};

mod transaction_request_handler;
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

/// Handles job results for simple handlers (no transaction state management).
///
/// Used by: notification_handler, solana_swap_request_handler, transaction_cleanup_handler
///
/// # Retry Strategy
/// - On success: Job completes
/// - On error: Retry until max_attempts reached
/// - At max_attempts: Abort job
pub fn handle_result(
    result: Result<(), Report>,
    attempt: Attempt,
    job_type: &str,
    max_attempts: usize,
) -> Result<(), Error> {
    if result.is_ok() {
        debug!(
            job_type = %job_type,
            "request handled successfully"
        );
        return Ok(());
    }

    let err = result.as_ref().unwrap_err();
    warn!(
        job_type = %job_type,
        error = %err,
        attempt = %attempt.current(),
        max_attempts = %max_attempts,
        "request failed"
    );

    if attempt.current() >= max_attempts {
        error!(
            job_type = %job_type,
            max_attempts = %max_attempts,
            "max attempts reached, failing job"
        );
        Err(Error::Abort(Arc::new("Failed to handle request".into())))?
    }

    Err(Error::Failed(Arc::new(
        "Failed to handle request. Retrying".into(),
    )))?
}

/// Handles job results for transaction lifecycle handlers.
///
/// Used by: transaction_request_handler, transaction_submission_handler
///
/// # Retry Strategy
/// - On success: Job completes
/// - On error: Retry until max_attempts reached
/// - At max_attempts: Set transaction to Failed status and abort job
///
/// # Arguments
/// * `result` - The result of the job execution
/// * `attempt` - Current attempt information
/// * `job_type` - Type of job for logging
/// * `max_attempts` - Maximum number of retry attempts
/// * `transaction_id` - ID of the transaction to update on failure
/// * `transaction_repo` - Repository for updating transaction state
pub async fn handle_transaction_result<R>(
    result: Result<(), Report>,
    attempt: Attempt,
    job_type: &str,
    max_attempts: usize,
    transaction_id: String,
    transaction_repo: Arc<R>,
) -> Result<(), Error>
where
    R: TransactionRepository,
{
    if result.is_ok() {
        debug!(
            job_type = %job_type,
            tx_id = %transaction_id,
            "request handled successfully"
        );
        return Ok(());
    }

    let err = result.as_ref().unwrap_err();
    warn!(
        job_type = %job_type,
        tx_id = %transaction_id,
        error = %err,
        attempt = %attempt.current(),
        max_attempts = %max_attempts,
        "request failed"
    );

    if attempt.current() >= max_attempts {
        error!(
            job_type = %job_type,
            tx_id = %transaction_id,
            max_attempts = %max_attempts,
            error = %err,
            "max attempts reached, marking transaction as failed"
        );

        // Update transaction to Failed status with reason
        let status_reason = format!(
            "Transaction processing failed after {} retry attempts. The transaction could not be completed due to persistent errors. Please check the transaction details and try again.",
            max_attempts
        );

        let update_request = TransactionUpdateRequest {
            status: Some(TransactionStatus::Failed),
            status_reason: Some(status_reason),
            ..Default::default()
        };

        if let Err(update_err) = transaction_repo
            .partial_update(transaction_id.clone(), update_request)
            .await
        {
            error!(
                tx_id = %transaction_id,
                error = %update_err,
                "failed to update transaction status to Failed"
            );
        } else {
            debug!(
                tx_id = %transaction_id,
                "successfully marked transaction as Failed"
            );
        }

        Err(Error::Abort(Arc::new(
            "Failed to handle transaction request".into(),
        )))?
    }

    Err(Error::Failed(Arc::new(
        "Failed to handle transaction request. Retrying".into(),
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
