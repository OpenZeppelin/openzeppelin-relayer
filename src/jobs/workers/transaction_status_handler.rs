use actix_web::web::ThinData;
use apalis::prelude::Data;
use log::info;

use crate::{
    domain::{get_relayer_transaction, get_transaction_by_id, Transaction},
    jobs::{Job, TransactionStatusCheck},
    AppState,
};

use super::HandlerError;

pub async fn transaction_status_handler(
    job: Job<TransactionStatusCheck>,
    state: Data<ThinData<AppState>>,
) -> Result<(), HandlerError> {
    info!("Handling transaction status job: {:?}", job.data);

    let relayer_transaction = get_relayer_transaction(job.data.relayer_id.clone(), &state).await?;

    let transaction = get_transaction_by_id(job.data.transaction_id, &state).await?;

    relayer_transaction
        .handle_transaction_status(transaction)
        .await?;
    Ok(())
}
