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
    domain::{get_relayer_transaction, get_transaction_by_id, Transaction},
    jobs::{handle_result, Job, TransactionRequest, DEFAULT_MAXIMUM_RETRIES},
    AppState,
};

pub async fn transaction_request_handler(
    job: Job<TransactionRequest>,
    state: Data<ThinData<AppState>>,
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
        DEFAULT_MAXIMUM_RETRIES,
    )
}

pub async fn handle_request(
    request: TransactionRequest,
    state: Data<ThinData<AppState>>,
) -> Result<()> {
    let relayer_transaction = get_relayer_transaction("sdsd".to_owned(), &state).await?;

    let transaction = get_transaction_by_id(request.transaction_id, &state).await?;

    relayer_transaction.prepare_transaction(transaction).await?;

    info!("Transaction request handled successfully");

    Ok(())
}
