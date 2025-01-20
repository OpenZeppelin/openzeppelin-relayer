use actix_web::web::ThinData;
use apalis::prelude::Data;
use eyre::Result;
use log::info;

use crate::{
    domain::{get_relayer_transaction, get_transaction_by_id, Transaction},
    jobs::{Job, TransactionRequest},
    AppState,
};

use super::HandlerError;

pub async fn transaction_request_handler(
    job: Job<TransactionRequest>,
    state: Data<ThinData<AppState>>,
) -> Result<(), HandlerError> {
    info!("Handling transaction request: {:?}", job.data);

    let relayer_transaction = get_relayer_transaction(job.data.relayer_id.clone(), &state).await?;

    let transaction = get_transaction_by_id(job.data.transaction_id, &state).await?;

    relayer_transaction.prepare_transaction(transaction).await?;

    info!("Transaction request handled successfully");

    Ok(())
}
