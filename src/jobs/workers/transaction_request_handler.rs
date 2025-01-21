use std::sync::Arc;

use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Context, Data, TaskId, Worker, *};
use apalis_redis::RedisContext;
use eyre::Result;
use log::info;

use crate::{
    domain::{get_relayer_transaction, get_transaction_by_id, Transaction},
    jobs::{Job, TransactionRequest},
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

    match result {
        Ok(_) => {
            info!("Transaction request handled successfully");
            Ok(())
        }
        Err(e) => {
            info!("Transaction request failed: {:?}", e);
            Err(Error::Failed(Arc::new(
                "Failed to handle transaction request".into(),
            )))
        }
    }
}

pub async fn handle_request(
    request: TransactionRequest,
    state: Data<ThinData<AppState>>,
) -> Result<()> {
    let relayer_transaction =
        get_relayer_transaction(request.relayer_id.to_string(), &state).await?;

    let transaction = get_transaction_by_id(request.transaction_id, &state).await?;

    relayer_transaction.prepare_transaction(transaction).await?;

    info!("Transaction request handled successfully");

    Ok(())
}
