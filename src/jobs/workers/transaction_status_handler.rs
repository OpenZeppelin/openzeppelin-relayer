use actix_web::web::ThinData;
use apalis::prelude::{Data, Error};
use log::info;

use crate::{
    jobs::{Job, TransactionStatusCheck},
    AppState,
};

pub async fn transaction_status_handler(
    job: Job<TransactionStatusCheck>,
    context: Data<ThinData<AppState>>,
) -> Result<(), Error> {
    info!("Handling transaction status job: {:?}", job.data);

    Ok(())
}
