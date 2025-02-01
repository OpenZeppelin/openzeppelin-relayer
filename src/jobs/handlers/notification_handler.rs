//! Notification handling worker implementation.
//!
//! This module implements the notification handling worker that processes
//! notification jobs from the queue.

use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Data, *};

use eyre::Result;
use log::info;

use crate::{
    jobs::{handle_result, Job, NotificationSend, DEFAULT_MAXIMUM_RETRIES},
    AppState,
};

/// Handles incoming notification jobs from the queue.
///
/// # Arguments
/// * `job` - The notification job containing recipient and message details
/// * `context` - Application state containing notification services
///
/// # Returns
/// * `Result<(), Error>` - Success or failure of notification processing
pub async fn notification_handler(
    job: Job<NotificationSend>,
    _context: Data<ThinData<AppState>>,
    attempt: Attempt,
) -> Result<(), Error> {
    info!("handling notification: {:?}", job.data);

    let result = handle_request(job.data, _context).await;

    handle_result(result, attempt, "Notification", DEFAULT_MAXIMUM_RETRIES)
}

pub async fn handle_request(
    _request: NotificationSend,
    _state: Data<ThinData<AppState>>,
) -> Result<()> {
    // handle notification

    Ok(())
}
