use actix_web::web::ThinData;
use apalis::prelude::{Data, Error};
use log::info;

use crate::{
    jobs::{Job, TransactionStatusCheck},
    AppState,
};

pub async fn status_queue_worker_handler(
    job: Job<TransactionStatusCheck>,
    context: Data<ThinData<AppState>>,
) -> Result<(), Error> {
    handle_transaction().await;

    Ok(())
}

pub async fn handle_transaction() {
    info!("handling transaction");
}
