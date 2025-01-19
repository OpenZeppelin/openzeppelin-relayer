use actix_web::web::{self, ThinData};
use apalis::prelude::{Data, Error};
use apalis_redis::RedisStorage;
use log::info;

use crate::{
    domain::{RelayerTransactionFactory, Transaction},
    jobs::{Job, TransactionProcess},
    repositories::Repository,
    AppState,
};

pub async fn transaction_queue_worker_handler(
    job: Job<TransactionProcess>,
    state: Data<ThinData<AppState>>,
) -> Result<(), Error> {
    info!("handling transaction");

    let relayer = state
        .relayer_repository
        .get_by_id(job.data.relayer_id)
        .await
        .unwrap();

    let transaction = state
        .transaction_repository
        .get_by_id(job.data.transaction_id)
        .await
        .unwrap();

    let relayer_transaction = RelayerTransactionFactory::create_transaction(
        relayer,
        state.relayer_repository(),
        state.transaction_repository(),
    )
    .unwrap();

    relayer_transaction
        .submit_transaction(transaction)
        .await
        .unwrap();

    Ok(())
}

pub async fn handle_transaction() {
    info!("handling transaction");
}
