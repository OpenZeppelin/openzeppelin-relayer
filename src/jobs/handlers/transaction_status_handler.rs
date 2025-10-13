//! Transaction status monitoring handler.
//!
//! Monitors the status of submitted transactions by:
//! - Checking transaction status on the network
//! - Updating transaction status in storage
//! - Triggering notifications on status changes
use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Data, *};
use eyre::Result;
use tracing::{debug, instrument};

use std::sync::Arc;

use crate::{
    domain::{get_relayer_transaction, get_transaction_by_id, is_final_state, Transaction},
    jobs::{Job, TransactionStatusCheck},
    models::{DefaultAppState, TransactionRepoModel},
    observability::request_id::set_request_id,
};

#[cfg(test)]
use crate::models::NetworkType;

#[instrument(
    level = "debug",
    skip(job, state),
    fields(
        request_id = ?job.request_id,
        job_id = %job.message_id,
        job_type = %job.job_type.to_string(),
        attempt = %attempt.current(),
        tx_id = %job.data.transaction_id,
        relayer_id = %job.data.relayer_id,
    )
)]
pub async fn transaction_status_handler(
    job: Job<TransactionStatusCheck>,
    state: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
) -> Result<(), Error> {
    if let Some(request_id) = job.request_id.clone() {
        set_request_id(request_id);
    }

    debug!(
        "handling transaction status check for tx_id {}",
        job.data.transaction_id
    );

    let result = handle_request(job.data, state).await;

    handle_status_check_result(result)
}

/// Handles status check results with special retry logic.
///
/// # Retry Strategy
/// - If transaction is in final state → Job completes successfully
/// - If error occurred → Retry (let handle_result decide)
/// - If transaction still not final → Retry to keep checking
fn handle_status_check_result(result: Result<TransactionRepoModel>) -> Result<(), Error> {
    match result {
        Ok(updated_tx) => {
            // Check if transaction reached final state
            if is_final_state(&updated_tx.status) {
                debug!(
                    tx_id = %updated_tx.id,
                    status = ?updated_tx.status,
                    "transaction reached final state, status check complete"
                );
                Ok(())
            } else {
                // Transaction still processing, retry status check
                debug!(
                    tx_id = %updated_tx.id,
                    status = ?updated_tx.status,
                    "transaction status: {:?} - not in final state, retrying status check",
                    updated_tx.status
                );
                Err(Error::Failed(Arc::new(
                    format!(
                        "transaction status: {:?} - not in final state, retrying status check",
                        updated_tx.status
                    )
                    .into(),
                )))?
            }
        }
        Err(e) => {
            // Error occurred, retry
            Err(Error::Failed(Arc::new(format!("{}", e).into())))?
        }
    }
}

async fn handle_request(
    status_request: TransactionStatusCheck,
    state: Data<ThinData<DefaultAppState>>,
) -> Result<TransactionRepoModel> {
    let relayer_transaction =
        get_relayer_transaction(status_request.relayer_id.clone(), &state).await?;

    let transaction = get_transaction_by_id(status_request.transaction_id.clone(), &state).await?;

    let updated_transaction = relayer_transaction
        .handle_transaction_status(transaction)
        .await?;

    debug!(
        "status check handled successfully for tx_id {}",
        status_request.transaction_id
    );

    Ok(updated_transaction)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_status_check_job_validation() {
        // Create a basic status check job
        let check_job = TransactionStatusCheck::new("tx123", "relayer-1", NetworkType::Evm);
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

        let check_job = TransactionStatusCheck::new("tx123", "relayer-1", NetworkType::Evm)
            .with_metadata(metadata.clone());

        // Validate the metadata
        assert!(check_job.metadata.is_some());
        let job_metadata = check_job.metadata.unwrap();
        assert_eq!(job_metadata.get("retry_count").unwrap(), "2");
        assert_eq!(job_metadata.get("last_status").unwrap(), "pending");
    }
}
