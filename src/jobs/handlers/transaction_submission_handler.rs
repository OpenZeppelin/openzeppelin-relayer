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
use log::info;

use crate::{
    constants::WORKER_DEFAULT_MAXIMUM_RETRIES,
    domain::{get_relayer_transaction, get_transaction_by_id, Transaction},
    jobs::{handle_result, Job, TransactionCommand, TransactionSend},
    models::DefaultAppState,
};

pub async fn transaction_submission_handler(
    job: Job<TransactionSend>,
    state: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
) -> Result<(), Error> {
    info!("handling transaction submission: {:?}", job.data);

    let result = handle_request(job.data, state).await;

    handle_result(
        result,
        attempt,
        "Transaction Sender",
        WORKER_DEFAULT_MAXIMUM_RETRIES,
    )
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
            info!("Cancelling transaction: {:?}", reason);
            relayer_transaction.submit_transaction(transaction).await?;
        }
        TransactionCommand::Resubmit => {
            info!("Resubmitting transaction with updated parameters");
            relayer_transaction
                .resubmit_transaction(transaction)
                .await?;
        }
        TransactionCommand::Resend => {
            info!("Resending transaction");
            relayer_transaction.submit_transaction(transaction).await?;
        }
    };

    info!("Transaction handled successfully");

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
