//! # Workers Module
//! Handles background job processing for the relayer service.

use std::time::Duration;

use actix_web::web::ThinData;
use apalis::{layers::ErrorHandlingLayer, prelude::*};
use eyre::Result;
use log::{error, info};
use tokio::signal::unix::SignalKind;

use crate::AppState;

mod transaction_queue_worker_handler;
pub use transaction_queue_worker_handler::*;

mod submission_queue_worker_handler;
pub use submission_queue_worker_handler::*;

mod notification_queue_worker_handler;
pub use notification_queue_worker_handler::*;

mod status_queue_worker_handler;
pub use status_queue_worker_handler::*;

use super::Queue;

const DEFAULT_CONCURRENCY: usize = 2;
const DEFAULT_RATE_LIMIT: u64 = 20;
const DEFAULT_RATE_LIMIT_DURATION: Duration = Duration::from_secs(1);

pub async fn setup_workers(queue: Queue, app_state: ThinData<AppState>) -> Result<()> {
    let transaction_queue_worker = WorkerBuilder::new("transaction_handler")
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .rate_limit(DEFAULT_RATE_LIMIT, DEFAULT_RATE_LIMIT_DURATION)
        .concurrency(DEFAULT_CONCURRENCY)
        .data(app_state.clone())
        .backend(queue.transaction_queue.clone())
        .build_fn(transaction_queue_worker_handler);

    let submission_queue_worker = WorkerBuilder::new("transaction_sender")
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .rate_limit(DEFAULT_RATE_LIMIT, DEFAULT_RATE_LIMIT_DURATION)
        .concurrency(DEFAULT_CONCURRENCY)
        .data(app_state.clone())
        .backend(queue.submission_queue.clone())
        .build_fn(submission_queue_worker_handler);

    let status_queue_worker = WorkerBuilder::new("transaction_status_checker")
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .concurrency(DEFAULT_CONCURRENCY)
        .rate_limit(DEFAULT_RATE_LIMIT, DEFAULT_RATE_LIMIT_DURATION)
        .data(app_state.clone())
        .backend(queue.status_queue.clone())
        .build_fn(status_queue_worker_handler);

    let notification_queue_worker = WorkerBuilder::new("notification_sender")
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .rate_limit(DEFAULT_RATE_LIMIT, DEFAULT_RATE_LIMIT_DURATION)
        .concurrency(DEFAULT_CONCURRENCY)
        .data(app_state.clone())
        .backend(queue.notification_queue.clone())
        .build_fn(notification_queue_worker_handler);

    let monitor_future = Monitor::new()
        .register(transaction_queue_worker)
        .register(submission_queue_worker)
        .register(status_queue_worker)
        .register(notification_queue_worker)
        .on_event(|e| {
            let worker_id = e.id();
            match e.inner() {
                Event::Engage(task_id) => {
                    info!("Worker [{worker_id}] got a job with id: {task_id}");
                }
                Event::Error(e) => {
                    error!("Worker [{worker_id}] encountered an error: {e}");
                }

                Event::Exit => {
                    info!("Worker [{worker_id}] exited");
                }
                Event::Idle => {
                    info!("Worker [{worker_id}] is idle");
                }
                Event::Start => {
                    info!("Worker [{worker_id}] started");
                }
                Event::Stop => {
                    info!("Worker [{worker_id}] stopped");
                }
                _ => {}
            }
        })
        .shutdown_timeout(Duration::from_millis(5000))
        .run_with_signal(async {
            let mut sigint = tokio::signal::unix::signal(SignalKind::interrupt())
                .expect("Failed to create SIGINT signal");
            let mut sigterm = tokio::signal::unix::signal(SignalKind::terminate())
                .expect("Failed to create SIGTERM signal");

            info!("Monitor started");

            tokio::select! {
                _ = sigint.recv() => info!("Received SIGINT."),
                _ = sigterm.recv() => info!("Received SIGTERM."),
            };

            info!("Monitor shutting down");

            Ok(())
        });
    tokio::spawn(async move {
        if let Err(e) = monitor_future.await {
            error!("Monitor error: {}", e);
        }
    });
    info!("Monitor shutdown complete");
    Ok(())
}
