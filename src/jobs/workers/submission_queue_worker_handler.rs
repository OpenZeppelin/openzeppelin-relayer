use actix_web::web::ThinData;
use apalis::prelude::{Data, Error};
use log::info;

use crate::{
    jobs::{Job, TransactionSubmit},
    AppState,
};

pub async fn submission_queue_worker_handler(
    job: Job<TransactionSubmit>,
    context: Data<ThinData<AppState>>,
) -> Result<(), Error> {
    handle_transaction().await;

    Ok(())
}

pub async fn handle_transaction() {
    info!("handling transaction");
}
