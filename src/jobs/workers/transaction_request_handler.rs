use actix_web::web::ThinData;
use apalis::prelude::{Data, Error};
use log::info;

use crate::{
    domain::{RelayerTransactionFactory, Transaction},
    jobs::{Job, TransactionRequest},
    repositories::Repository,
    AppState,
};

pub async fn transaction_request_handler(
    job: Job<TransactionRequest>,
    state: Data<ThinData<AppState>>,
) -> Result<(), Error> {
    info!("handling transaction: {:?}", job.data);

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
