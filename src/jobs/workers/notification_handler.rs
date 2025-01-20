use actix_web::web::ThinData;
use apalis::prelude::Data;
use log::info;

use crate::{
    jobs::{Job, NotificationSend},
    AppState,
};

use super::HandlerError;

pub async fn notification_handler(
    job: Job<NotificationSend>,
    _context: Data<ThinData<AppState>>,
) -> Result<(), HandlerError> {
    info!("handling notification: {:?}", job.data);

    Ok(())
}
