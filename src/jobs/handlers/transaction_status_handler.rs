//! Transaction status monitoring handler.
//!
//! Monitors the status of submitted transactions by:
//! - Checking transaction status on the network
//! - Updating transaction status in storage
//! - Triggering notifications on status changes
use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Data, *};

use eyre::Result;
use log::info;

use crate::{
    domain::{get_relayer_transaction, get_transaction_by_id, Transaction},
    jobs::{handle_result, Job, TransactionStatusCheck, DEFAULT_MAXIMUM_RETRIES},
    AppState,
};

pub async fn transaction_status_handler(
    job: Job<TransactionStatusCheck>,
    state: Data<ThinData<AppState>>,
    attempt: Attempt,
) -> Result<(), Error> {
    info!("Handling transaction status job: {:?}", job.data);

    let result = handle_request(job.data, state).await;

    handle_result(
        result,
        attempt,
        "Transaction Status",
        DEFAULT_MAXIMUM_RETRIES,
    )
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
