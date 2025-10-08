//! Transaction request handler for processing incoming transaction jobs.
//!
//! Handles the validation and preparation of transactions before they are
//! submitted to the network
use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Context, Data, TaskId, Worker, *};
use apalis_redis::RedisContext;
use eyre::Result;
use tracing::{debug, instrument};

use crate::{
    constants::WORKER_TRANSACTION_REQUEST_RETRIES,
    domain::{get_relayer_transaction, get_transaction_by_id, Transaction},
    jobs::{handle_transaction_result, Job, TransactionRequest},
    models::DefaultAppState,
    observability::request_id::set_request_id,
};

#[instrument(
    level = "info",
    skip(job, state, _worker, _ctx),
    fields(
        request_id = ?job.request_id,
        job_id = %job.message_id,
        job_type = %job.job_type.to_string(),
        attempt = %attempt.current(),
        tx_id = %job.data.transaction_id,
        relayer_id = %job.data.relayer_id,
        task_id = %task_id.to_string(),
    )
)]
pub async fn transaction_request_handler(
    job: Job<TransactionRequest>,
    state: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
    _worker: Worker<Context>,
    task_id: TaskId,
    _ctx: RedisContext,
) -> Result<(), Error> {
    if let Some(request_id) = job.request_id.clone() {
        set_request_id(request_id);
    }

    debug!("handling transaction request {}", job.data.transaction_id);

    let transaction_id = job.data.transaction_id.clone();
    let result = handle_request(job.data, state.clone()).await;

    // Handle transaction result by setting transaction status to Failed when max attempts are reached
    handle_transaction_result(
        result,
        attempt,
        "Transaction Request",
        WORKER_TRANSACTION_REQUEST_RETRIES,
        transaction_id,
        state.transaction_repository(),
    )
    .await
}

async fn handle_request(
    request: TransactionRequest,
    state: Data<ThinData<DefaultAppState>>,
) -> Result<()> {
    let relayer_transaction = get_relayer_transaction(request.relayer_id, &state).await?;

    let transaction = get_transaction_by_id(request.transaction_id.clone(), &state).await?;

    relayer_transaction.prepare_transaction(transaction).await?;

    debug!(
        "transaction request handled successfully {}",
        request.transaction_id.clone()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use apalis::prelude::Attempt;

    #[tokio::test]
    async fn test_handler_result_processing() {
        // This test focuses only on the interaction with handle_result
        // which we can test without mocking the entire state

        // Create a minimal job
        let request = TransactionRequest::new("tx123", "relayer-1");
        let job = Job::new(crate::jobs::JobType::TransactionRequest, request);

        // Create a test attempt
        let attempt = Attempt::default();

        // We cannot fully test the transaction_request_handler without extensive mocking
        // of the domain layer, but we can verify our test setup is correct
        assert_eq!(job.data.transaction_id, "tx123");
        assert_eq!(job.data.relayer_id, "relayer-1");
        assert_eq!(attempt.current(), 0);
    }

    // Note: Fully testing the functionality would require either:
    // 1. Dependency injection for all external dependencies
    // 2. Feature flags to enable mock implementations
    // 3. Integration tests with a real or test database

    // For now, these tests serve as placeholders to be expanded
    // when the appropriate testing infrastructure is in place.
}
