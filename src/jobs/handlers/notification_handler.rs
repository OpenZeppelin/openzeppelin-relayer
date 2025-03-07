//! Notification handling worker implementation.
//!
//! This module implements the notification handling worker that processes
//! notification jobs from the queue.

use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Data, *};

use eyre::Result;
use log::info;

use crate::{
    constants::WORKER_DEFAULT_MAXIMUM_RETRIES,
    jobs::{handle_result, Job, NotificationSend},
    models::AppState,
    repositories::Repository,
    services::WebhookNotificationService,
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
    context: Data<ThinData<AppState>>,
    attempt: Attempt,
) -> Result<(), Error> {
    info!("handling notification: {:?}", job.data);

    let result = handle_request(job.data, context).await;

    handle_result(
        result,
        attempt,
        "Notification",
        WORKER_DEFAULT_MAXIMUM_RETRIES,
    )
}

async fn handle_request(
    request: NotificationSend,
    context: Data<ThinData<AppState>>,
) -> Result<()> {
    info!("sending notification: {:?}", request);
    let notification = context
        .notification_repository
        .get_by_id(request.notification_id)
        .await?;

    let notification_service =
        WebhookNotificationService::new(notification.url, notification.signing_key);

    notification_service
        .send_notification(request.notification)
        .await?;

    Ok(())
}
