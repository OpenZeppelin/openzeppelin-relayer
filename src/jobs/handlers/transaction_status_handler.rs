//! Transaction status monitoring handler.
//!
//! Monitors the status of submitted transactions by:
//! - Checking transaction status on the network
//! - Updating transaction status in storage
//! - Triggering notifications on status changes
use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Data, *};

use eyre::Result;
use log::info;

use crate::{
    constants::WORKER_DEFAULT_MAXIMUM_RETRIES,
    domain::{get_relayer_transaction, get_transaction_by_id, Transaction},
    jobs::{handle_result, Job, TransactionStatusCheck},
    models::DefaultAppState,
};

pub async fn transaction_status_handler(
    job: Job<TransactionStatusCheck>,
    state: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
) -> Result<(), Error> {
    info!("Handling transaction status job: {:?}", job.data);

    let result = handle_request(job.data, state).await;

    handle_result(
        result,
        attempt,
        "Transaction Status",
        WORKER_DEFAULT_MAXIMUM_RETRIES,
    )
}

async fn handle_request(
    status_request: TransactionStatusCheck,
    state: Data<ThinData<DefaultAppState>>,
) -> Result<()> {
    let relayer_transaction =
        get_relayer_transaction(status_request.relayer_id.clone(), &state).await?;

    let transaction = get_transaction_by_id(status_request.transaction_id, &state).await?;

    relayer_transaction
        .handle_transaction_status(transaction)
        .await?;

    info!("Status check handled successfully");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use apalis::prelude::Attempt;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_status_check_job_validation() {
        // Create a basic status check job
        let check_job = TransactionStatusCheck::new("tx123", "relayer-1");
        let job = Job::new(crate::jobs::JobType::TransactionStatusCheck, check_job);

        // Validate the job data
        assert_eq!(job.data.transaction_id, "tx123");
        assert_eq!(job.data.relayer_id, "relayer-1");
        assert!(job.data.metadata.is_none());
    }

    #[tokio::test]
    async fn test_status_check_with_metadata() {
        // Create a job with retry metadata
        let mut metadata = HashMap::new();
        metadata.insert("retry_count".to_string(), "2".to_string());
        metadata.insert("last_status".to_string(), "pending".to_string());

        let check_job =
            TransactionStatusCheck::new("tx123", "relayer-1").with_metadata(metadata.clone());

        // Validate the metadata
        assert!(check_job.metadata.is_some());
        let job_metadata = check_job.metadata.unwrap();
        assert_eq!(job_metadata.get("retry_count").unwrap(), "2");
        assert_eq!(job_metadata.get("last_status").unwrap(), "pending");
    }

    #[tokio::test]
    async fn test_status_handler_attempt_tracking() {
        // Create attempts with different retry counts
        let first_attempt = Attempt::default();
        assert_eq!(first_attempt.current(), 0);

        let second_attempt = Attempt::default();
        second_attempt.increment();
        assert_eq!(second_attempt.current(), 1);

        let final_attempt = Attempt::default();
        for _ in 0..WORKER_DEFAULT_MAXIMUM_RETRIES {
            final_attempt.increment();
        }
        assert_eq!(final_attempt.current(), WORKER_DEFAULT_MAXIMUM_RETRIES);
    }
}
