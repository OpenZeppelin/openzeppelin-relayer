//! Transaction submission handler for processing submission jobs.
//!
//! Handles the submission of prepared transactions to networks:
//! - Submits transactions to appropriate networks
//! - Handles different submission commands (Submit, Cancel, Resubmit)
//! - Updates transaction status after submission
//! - Enqueues status monitoring jobs
use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Data, *};
use eyre::Result;
use log::info;

use crate::{
    domain::{get_relayer_transaction, get_transaction_by_id, Transaction},
    jobs::{handle_result, Job, TransactionCommand, TransactionSend, DEFAULT_MAXIMUM_RETRIES},
    AppState,
};

pub async fn transaction_submission_handler(
    job: Job<TransactionSend>,
    state: Data<ThinData<AppState>>,
    attempt: Attempt,
) -> Result<(), Error> {
    info!("handling transaction submission: {:?}", job.data);

    let result = handle_request(job.data, state).await;

    handle_result(
        result,
        attempt,
        "Transaction Sender",
        DEFAULT_MAXIMUM_RETRIES,
    )
}

pub async fn handle_request(
    status_request: TransactionSend,
    state: Data<ThinData<AppState>>,
) -> Result<()> {
    let relayer_transaction =
        get_relayer_transaction(status_request.relayer_id.clone(), &state).await?;

    let transaction = get_transaction_by_id(status_request.transaction_id, &state).await?;

    match status_request.command {
        TransactionCommand::Submit => {
            relayer_transaction.submit_transaction(transaction).await?;
        }
        TransactionCommand::Cancel { reason } => {
            info!("Cancelling transaction: {:?}", reason);
            relayer_transaction.submit_transaction(transaction).await?;
        }
        TransactionCommand::Resubmit => {
            info!("Resubmitting transaction");
            relayer_transaction.submit_transaction(transaction).await?;
        }
        TransactionCommand::Resend => {
            info!("Resending transaction");
            relayer_transaction.submit_transaction(transaction).await?;
        }
    };

    info!("Transaction handled successfully");

    Ok(())
}
