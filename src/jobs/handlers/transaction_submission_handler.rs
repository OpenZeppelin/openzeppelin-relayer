//! Transaction submission handler for processing submission jobs.
//!
//! Handles the submission of prepared transactions to networks:
//! - Submits transactions to appropriate networks
//! - Handles different submission commands (Submit, Cancel, Resubmit)
//! - Updates transaction status after submission
//! - Enqueues status monitoring jobs
use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Data, *};
use eyre::Result;
use std::sync::Arc;
use tracing::{debug, error, info, instrument, warn};

use crate::{
    constants::{
        WORKER_TRANSACTION_CANCEL_RETRIES, WORKER_TRANSACTION_RESEND_RETRIES,
        WORKER_TRANSACTION_RESUBMIT_RETRIES, WORKER_TRANSACTION_SUBMIT_RETRIES,
    },
    domain::{get_relayer_transaction, get_transaction_by_id, Transaction},
    jobs::{Job, TransactionCommand, TransactionSend},
    models::{DefaultAppState, TransactionStatus, TransactionUpdateRequest},
    repositories::TransactionRepository,
    observability::request_id::set_request_id,
};

#[instrument(
    level = "info",
    skip(job, state),
    fields(
        request_id = ?job.request_id,
        job_id = %job.message_id,
        job_type = %job.job_type.to_string(),
        attempt = %attempt.current(),
        tx_id = %job.data.transaction_id,
        relayer_id = %job.data.relayer_id,
        command = ?job.data.command,
    )
)]
pub async fn transaction_submission_handler(
    job: Job<TransactionSend>,
    state: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
) -> Result<(), Error> {
    if let Some(request_id) = job.request_id.clone() {
        set_request_id(request_id);
    }

    debug!(
        "handling transaction submission {}",
        job.data.transaction_id
    );

    let transaction_id = job.data.transaction_id.clone();
    let command = job.data.command.clone();
    let result = handle_request(job.data, state.clone()).await;

    // Handle result with command-specific retry logic
    handle_submission_result(
        result,
        attempt,
        &command,
        transaction_id,
        state.transaction_repository(),
    )
    .await
}

/// Get max retry count based on command type
fn get_max_retries(command: &TransactionCommand) -> usize {
    match command {
        TransactionCommand::Submit => WORKER_TRANSACTION_SUBMIT_RETRIES,
        TransactionCommand::Resubmit => WORKER_TRANSACTION_RESUBMIT_RETRIES,
        TransactionCommand::Cancel { .. } => WORKER_TRANSACTION_CANCEL_RETRIES,
        TransactionCommand::Resend => WORKER_TRANSACTION_RESEND_RETRIES,
    }
}

/// Handle submission result with command-specific retry and failure logic
async fn handle_submission_result<TR>(
    result: Result<()>,
    attempt: Attempt,
    command: &TransactionCommand,
    transaction_id: String,
    transaction_repo: Arc<TR>,
) -> Result<(), Error>
where
    TR: TransactionRepository,
{
    let max_retries = get_max_retries(command);
    let command_name = format!("{:?}", command);

    match result {
        Ok(()) => Ok(()),
        Err(e) if attempt.current() < max_retries => {
            // Continue retrying
            debug!(
                tx_id = %transaction_id,
                command = %command_name,
                attempt = attempt.current(),
                max_retries,
                "submission failed, will retry"
            );
            Err(Error::Failed(Arc::new(e.to_string().into())))
        }
        Err(e) => {
            // Max retries exhausted - handle based on command type
            match command {
                TransactionCommand::Submit => {
                    // For fresh submissions, mark as Failed
                    warn!(
                        tx_id = %transaction_id,
                        command = %command_name,
                        attempts = attempt.current(),
                        "submission retries exhausted, marking transaction as Failed"
                    );

                    let update_request = TransactionUpdateRequest {
                        status: Some(TransactionStatus::Failed),
                        status_reason: Some(format!("Failed to submit after {} attempts: {}", attempt.current(), e)),
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
                    }

                    Err(Error::Abort(Arc::new(e.to_string().into())))
                }
                TransactionCommand::Resubmit | TransactionCommand::Cancel { .. } | TransactionCommand::Resend => {
                    // For resubmit/cancel/resend, don't mark as Failed
                    // Status checker is already running and will retry on next cycle
                    // DB state is source of truth - status checker will see the same conditions
                    // and attempt the operation again
                    warn!(
                        tx_id = %transaction_id,
                        command = %command_name,
                        attempts = attempt.current(),
                        "submission retries exhausted, status checker will retry on next cycle"
                    );

                    // Return Ok to stop job retries - status checker will handle it
                    Ok(())
                }
            }
        }
    }
}

async fn handle_request(
    status_request: TransactionSend,
    state: Data<ThinData<DefaultAppState>>,
) -> Result<()> {
    let relayer_transaction =
        get_relayer_transaction(status_request.relayer_id.clone(), &state).await?;

    let transaction = get_transaction_by_id(status_request.transaction_id, &state).await?;

    match status_request.command {
        TransactionCommand::Submit => {
            relayer_transaction.submit_transaction(transaction).await?;
        }
        TransactionCommand::Cancel { reason } => {
            info!(
                reason = %reason,
                "cancelling transaction {}", transaction.id
            );
            relayer_transaction.submit_transaction(transaction).await?;
        }
        TransactionCommand::Resubmit => {
            debug!(
                "resubmitting transaction with updated parameters {}",
                transaction.id
            );
            relayer_transaction
                .resubmit_transaction(transaction)
                .await?;
        }
        TransactionCommand::Resend => {
            debug!("resending transaction {}", transaction.id);
            relayer_transaction.submit_transaction(transaction).await?;
        }
    };

    debug!("transaction handled successfully");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_submission_handler_job_validation() {
        // Create a job with Submit command
        let submit_job = TransactionSend::submit("tx123", "relayer-1");
        let job = Job::new(crate::jobs::JobType::TransactionSend, submit_job);

        // Validate the job data
        match job.data.command {
            TransactionCommand::Submit => {}
            _ => panic!("Expected Submit command"),
        }
        assert_eq!(job.data.transaction_id, "tx123");
        assert_eq!(job.data.relayer_id, "relayer-1");
        assert!(job.data.metadata.is_none());

        // Create a job with Cancel command
        let cancel_job = TransactionSend::cancel("tx123", "relayer-1", "user requested");
        let job = Job::new(crate::jobs::JobType::TransactionSend, cancel_job);

        // Validate the job data
        match job.data.command {
            TransactionCommand::Cancel { reason } => {
                assert_eq!(reason, "user requested");
            }
            _ => panic!("Expected Cancel command"),
        }
    }

    #[tokio::test]
    async fn test_submission_job_with_metadata() {
        // Create a job with metadata
        let mut metadata = HashMap::new();
        metadata.insert("gas_price".to_string(), "20000000000".to_string());

        let submit_job =
            TransactionSend::submit("tx123", "relayer-1").with_metadata(metadata.clone());

        // Validate the metadata
        assert!(submit_job.metadata.is_some());
        let job_metadata = submit_job.metadata.unwrap();
        assert_eq!(job_metadata.get("gas_price").unwrap(), "20000000000");
    }

    // Note: As with the transaction_request_handler tests, full testing of the
    // handler functionality would require dependency injection or integration tests.
}
