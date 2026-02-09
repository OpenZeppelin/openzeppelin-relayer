//! Transaction submission handler for processing submission jobs.
//!
//! Handles the submission of prepared transactions to networks:
//! - Submits transactions to appropriate networks
//! - Handles different submission commands (Submit, Cancel, Resubmit)
//! - Updates transaction status after submission
//! - Enqueues status monitoring jobs
use actix_web::web::ThinData;
use eyre::Result;
use tracing::{debug, info, instrument};

use crate::{
    constants::{
        WORKER_TRANSACTION_CANCEL_RETRIES, WORKER_TRANSACTION_RESEND_RETRIES,
        WORKER_TRANSACTION_RESUBMIT_RETRIES, WORKER_TRANSACTION_SUBMIT_RETRIES,
    },
    domain::{get_relayer_transaction, get_transaction_by_id, Transaction},
    jobs::{
        handle_result,
        queue_backend::types::{HandlerError, WorkerContext},
        Job, TransactionCommand, TransactionSend,
    },
    models::DefaultAppState,
    observability::request_id::set_request_id,
};

#[instrument(
    level = "info",
    skip(job, state, ctx),
    fields(
        request_id = ?job.request_id,
        job_id = %job.message_id,
        job_type = %job.job_type.to_string(),
        attempt = %ctx.attempt,
        tx_id = %job.data.transaction_id,
        relayer_id = %job.data.relayer_id,
        task_id = %ctx.task_id,
        command = ?job.data.command,
    )
)]
pub async fn transaction_submission_handler(
    job: Job<TransactionSend>,
    state: ThinData<DefaultAppState>,
    ctx: WorkerContext,
) -> Result<(), HandlerError> {
    if let Some(request_id) = job.request_id.clone() {
        set_request_id(request_id);
    }

    debug!(
        tx_id = %job.data.transaction_id,
        relayer_id = %job.data.relayer_id,
        "handling transaction submission"
    );

    let command = job.data.command.clone();
    let result = handle_request(job.data, &state).await;

    // Handle result with command-specific retry logic
    handle_result(
        result,
        &ctx,
        "Transaction Submission",
        get_max_retries(&command),
    )
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

async fn handle_request(
    status_request: TransactionSend,
    state: &ThinData<DefaultAppState>,
) -> Result<()> {
    let relayer_transaction =
        get_relayer_transaction(status_request.relayer_id.clone(), state).await?;

    let transaction = get_transaction_by_id(status_request.transaction_id, state).await?;

    // Capture transaction info for completion log
    let tx_id = transaction.id.clone();
    let relayer_id = transaction.relayer_id.clone();
    let command = status_request.command.clone();

    debug!(
        tx_id = %transaction.id,
        relayer_id = %transaction.relayer_id,
        status = ?transaction.status,
        "loaded transaction for submission"
    );

    match status_request.command {
        TransactionCommand::Submit => {
            relayer_transaction.submit_transaction(transaction).await?;
        }
        TransactionCommand::Cancel { reason } => {
            info!(
                tx_id = %transaction.id,
                relayer_id = %transaction.relayer_id,
                status = ?transaction.status,
                reason = %reason,
                "cancelling transaction"
            );
            relayer_transaction.submit_transaction(transaction).await?;
        }
        TransactionCommand::Resubmit => {
            debug!(
                tx_id = %transaction.id,
                relayer_id = %transaction.relayer_id,
                status = ?transaction.status,
                "resubmitting transaction with updated parameters"
            );
            relayer_transaction
                .resubmit_transaction(transaction)
                .await?;
        }
        TransactionCommand::Resend => {
            debug!(
                tx_id = %transaction.id,
                relayer_id = %transaction.relayer_id,
                status = ?transaction.status,
                "resending transaction"
            );
            relayer_transaction.submit_transaction(transaction).await?;
        }
    };

    debug!(
        tx_id = %tx_id,
        relayer_id = %relayer_id,
        command = ?command,
        "transaction submission completed"
    );

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

    mod get_max_retries_tests {
        use super::*;

        #[test]
        fn test_submit_command_retries() {
            let command = TransactionCommand::Submit;
            let retries = get_max_retries(&command);

            assert_eq!(
                retries, WORKER_TRANSACTION_SUBMIT_RETRIES,
                "Submit command should use WORKER_TRANSACTION_SUBMIT_RETRIES"
            );
        }

        #[test]
        fn test_resubmit_command_retries() {
            let command = TransactionCommand::Resubmit;
            let retries = get_max_retries(&command);

            assert_eq!(
                retries, WORKER_TRANSACTION_RESUBMIT_RETRIES,
                "Resubmit command should use WORKER_TRANSACTION_RESUBMIT_RETRIES"
            );
        }

        #[test]
        fn test_cancel_command_retries() {
            let command = TransactionCommand::Cancel {
                reason: "test cancel".to_string(),
            };
            let retries = get_max_retries(&command);

            assert_eq!(
                retries, WORKER_TRANSACTION_CANCEL_RETRIES,
                "Cancel command should use WORKER_TRANSACTION_CANCEL_RETRIES"
            );
        }

        #[test]
        fn test_resend_command_retries() {
            let command = TransactionCommand::Resend;
            let retries = get_max_retries(&command);

            assert_eq!(
                retries, WORKER_TRANSACTION_RESEND_RETRIES,
                "Resend command should use WORKER_TRANSACTION_RESEND_RETRIES"
            );
        }
    }
}
