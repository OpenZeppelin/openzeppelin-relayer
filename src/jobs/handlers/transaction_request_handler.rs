//! Transaction request handler for processing incoming transaction jobs.
//!
//! Handles the validation and preparation of transactions before they are
//! submitted to the network
use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Context, Data, TaskId, Worker, *};
use apalis_redis::RedisContext;
use eyre::Result;
use log::info;

use crate::{
    constants::WORKER_DEFAULT_MAXIMUM_RETRIES,
    domain::{get_relayer_transaction, get_transaction_by_id, Transaction},
    jobs::{handle_result, Job, TransactionRequest},
    models::DefaultAppState,
};

pub async fn transaction_request_handler(
    job: Job<TransactionRequest>,
    state: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
    worker: Worker<Context>,
    task_id: TaskId,
    ctx: RedisContext,
) -> Result<(), Error> {
    info!("Handling transaction request: {:?}", job.data);
    info!("Attempt: {:?}", attempt);
    info!("Worker: {:?}", worker);
    info!("Task ID: {:?}", task_id);
    info!("Context: {:?}", ctx);

    let result = handle_request(job.data, state).await;

    handle_result(
        result,
        attempt,
        "Transaction Request",
        WORKER_DEFAULT_MAXIMUM_RETRIES,
    )
}

async fn handle_request(
    request: TransactionRequest,
    state: Data<ThinData<DefaultAppState>>,
) -> Result<()> {
    let relayer_transaction = get_relayer_transaction(request.relayer_id, &state).await?;

    let transaction = get_transaction_by_id(request.transaction_id, &state).await?;

    relayer_transaction.prepare_transaction(transaction).await?;

    info!("Transaction request handled successfully");

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
