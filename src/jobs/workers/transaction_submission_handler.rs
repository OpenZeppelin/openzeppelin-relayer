use actix_web::web::ThinData;
use apalis::prelude::Data;
use log::info;

use crate::{
    domain::{get_relayer_transaction, get_transaction_by_id, Transaction},
    jobs::{Job, TransactionSubmit},
    AppState,
};

use super::HandlerError;

pub async fn transaction_submission_handler(
    job: Job<TransactionSubmit>,
    state: Data<ThinData<AppState>>,
) -> Result<(), HandlerError> {
    info!("handling transaction submission: {:?}", job.data);

    let relayer_transaction = get_relayer_transaction(job.data.relayer_id.clone(), &state).await?;

    let transaction = get_transaction_by_id(job.data.transaction_id, &state).await?;

    relayer_transaction.submit_transaction(transaction).await?;
    Ok(())
}
