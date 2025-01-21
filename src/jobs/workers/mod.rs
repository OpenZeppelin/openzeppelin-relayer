//! # Workers Module
//! Handles background job processing for the relayer service.

use actix_web::web::ThinData;
use apalis::{layers::ErrorHandlingLayer, prelude::*};
use eyre::Result;
use log::{error, info};
use std::time::Duration;
use tokio::signal::unix::SignalKind;

use crate::AppState;

mod transaction_request_handler;
pub use transaction_request_handler::*;

mod transaction_submission_handler;
pub use transaction_submission_handler::*;

mod notification_handler;
pub use notification_handler::*;

mod transaction_status_handler;
pub use transaction_status_handler::*;

mod handler_error;
pub use handler_error::*;

mod retry_backoff;

const DEFAULT_CONCURRENCY: usize = 2;
const DEFAULT_RATE_LIMIT: u64 = 20;
const DEFAULT_RATE_LIMIT_DURATION: Duration = Duration::from_secs(1);

pub async fn setup_workers(app_state: ThinData<AppState>) -> Result<()> {
    let queue = app_state.job_producer.get_queue().await?;
    let transaction_request_queue_worker = WorkerBuilder::new("transaction_request_handler")
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .rate_limit(DEFAULT_RATE_LIMIT, DEFAULT_RATE_LIMIT_DURATION)
        .concurrency(DEFAULT_CONCURRENCY)
        .data(app_state.clone())
        .backend(queue.transaction_request_queue.clone())
        .build_fn(transaction_request_handler);

    let transaction_submission_queue_worker = WorkerBuilder::new("transaction_sender")
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .rate_limit(DEFAULT_RATE_LIMIT, DEFAULT_RATE_LIMIT_DURATION)
        .concurrency(DEFAULT_CONCURRENCY)
        .data(app_state.clone())
        .backend(queue.transaction_submission_queue.clone())
        .build_fn(transaction_submission_handler);

    let transaction_status_queue_worker = WorkerBuilder::new("transaction_status_checker")
        .layer(ErrorHandlingLayer::new())
        .catch_panic()
        .enable_tracing()
        .concurrency(DEFAULT_CONCURRENCY)
        .rate_limit(DEFAULT_RATE_LIMIT, DEFAULT_RATE_LIMIT_DURATION)
        .data(app_state.clone())
        .backend(queue.transaction_status_queue.clone())
        .build_fn(transaction_status_handler);

    let notification_queue_worker = WorkerBuilder::new("notification_sender")
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .rate_limit(DEFAULT_RATE_LIMIT, DEFAULT_RATE_LIMIT_DURATION)
        .concurrency(DEFAULT_CONCURRENCY)
        .data(app_state.clone())
        .backend(queue.notification_queue.clone())
        .build_fn(notification_handler);

    let monitor_future = Monitor::new()
        .register(transaction_request_queue_worker)
        .register(transaction_submission_queue_worker)
        .register(transaction_status_queue_worker)
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
