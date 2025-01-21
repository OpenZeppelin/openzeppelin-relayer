use actix_web::web::ThinData;
use apalis::prelude::{Data, *};
use eyre::Result;
use log::info;
use std::sync::Arc;

use crate::{
    domain::{get_relayer_transaction, get_transaction_by_id, Transaction},
    jobs::{Job, TransactionStatusCheck},
    AppState,
};

pub async fn transaction_status_handler(
    job: Job<TransactionStatusCheck>,
    state: Data<ThinData<AppState>>,
) -> Result<(), Error> {
    info!("Handling transaction status job: {:?}", job.data);

    let result = handle_request(job.data, state).await;

    match result {
        Ok(_) => {
            info!("Transaction request handled successfully");
            #[allow(clippy::needless_return)]
            return Ok(());
        }
        Err(e) => {
            info!("Transaction request failed: {:?}", e);
            #[allow(clippy::needless_return)]
            return Err(Error::Failed(Arc::new(
                "Failed to handle transaction request".into(),
            )));
        }
    }
}

pub async fn handle_request(
    status_request: TransactionStatusCheck,
    state: Data<ThinData<AppState>>,
) -> Result<()> {
    let relayer_transaction =
        get_relayer_transaction(status_request.relayer_id.clone(), &state).await?;

    let transaction = get_transaction_by_id(status_request.transaction_id, &state).await?;

    relayer_transaction
        .handle_transaction_status(transaction)
        .await?;

    info!("Status check handled successfully");

    Ok(())
}
