use actix_web::web::ThinData;
use apalis::prelude::{Data, Error};
use log::info;

use crate::{
    jobs::{Job, NotificationSend},
    AppState,
};

pub async fn notification_handler(
    job: Job<NotificationSend>,
    context: Data<ThinData<AppState>>,
) -> Result<(), Error> {
    info!("handling notification: {:?}", job.data);

    Ok(())
}
