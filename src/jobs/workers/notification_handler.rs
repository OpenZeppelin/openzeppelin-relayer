//! Notification handling worker implementation.
//!
//! This module implements the notification handling worker that processes
//! notification jobs from the queue. It handles webhook notification type.
//!
//! # Architecture
//!
//! The notification handler follows a two-step process:
//! 1. Receives notification job from queue
//! 2. Processes notification
//! ```
use std::sync::Arc;

use actix_web::web::ThinData;
use apalis::prelude::{Data, Error};

use eyre::Result;
use log::info;

use crate::{
    jobs::{Job, NotificationSend},
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
/// ```
pub async fn notification_handler(
    job: Job<NotificationSend>,
    _context: Data<ThinData<AppState>>,
) -> Result<(), Error> {
    info!("handling notification: {:?}", job.data);

    let result = handle_request(job.data, _context).await;

    match result {
        Ok(_) => {
            info!("Notification request handled successfully");
            Ok(())
        }
        Err(e) => {
            info!("Notification request failed: {:?}", e);
            Err(Error::Failed(Arc::new(
                "Failed to handle Notification request".into(),
            )))
        }
    }
}

pub async fn handle_request(
    _request: NotificationSend,
    _state: Data<ThinData<AppState>>,
) -> Result<()> {
    // handle notification

    Ok(())
}
