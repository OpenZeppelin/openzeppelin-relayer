use actix_web::web::ThinData;
use apalis::prelude::{Data, Error};
use log::info;

use crate::{
    jobs::{Job, TransactionSubmit},
    AppState,
};

pub async fn transaction_submission_handler(
    job: Job<TransactionSubmit>,
    context: Data<ThinData<AppState>>,
) -> Result<(), Error> {
    info!("handling transaction submission: {:?}", job.data);
    Ok(())
}
