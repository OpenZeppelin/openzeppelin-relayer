//! # Workers
//! Initialise and starts the workers for the application

use actix_web::web::ThinData;
use apalis::{layers::ErrorHandlingLayer, prelude::*};
use eyre::Result;
use log::{error, info};
use std::time::Duration;
use tokio::signal::unix::SignalKind;

use crate::{
    jobs::{
        notification_handler, transaction_request_handler, transaction_status_handler,
        transaction_submission_handler, BackoffRetryPolicy,
    },
    AppState,
};

// Review and fine tune configuration for the workers
const DEFAULT_CONCURRENCY: usize = 2;
const DEFAULT_RATE_LIMIT: u64 = 20;
const DEFAULT_RATE_LIMIT_DURATION: Duration = Duration::from_secs(1);

const TRANSACTION_REQUEST: &str = "transaction_request";
const TRANSACTION_SENDER: &str = "transaction_sender";
const TRANSACTION_STATUS_CHECKER: &str = "transaction_status_checker";
const NOTIFICATION_SENDER: &str = "notification_sender";

pub async fn initialize_workers(app_state: ThinData<AppState>) -> Result<()> {
    let queue = app_state.job_producer.get_queue().await?;

    let transaction_request_queue_worker = WorkerBuilder::new(TRANSACTION_REQUEST)
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .rate_limit(DEFAULT_RATE_LIMIT, DEFAULT_RATE_LIMIT_DURATION)
        .retry(BackoffRetryPolicy::default())
        .concurrency(DEFAULT_CONCURRENCY)
        .data(app_state.clone())
        .backend(queue.transaction_request_queue.clone())
        .build_fn(transaction_request_handler);

    let transaction_submission_queue_worker = WorkerBuilder::new(TRANSACTION_SENDER)
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .rate_limit(DEFAULT_RATE_LIMIT, DEFAULT_RATE_LIMIT_DURATION)
        .retry(BackoffRetryPolicy::default())
        .concurrency(DEFAULT_CONCURRENCY)
        .data(app_state.clone())
        .backend(queue.transaction_submission_queue.clone())
        .build_fn(transaction_submission_handler);

    let transaction_status_queue_worker = WorkerBuilder::new(TRANSACTION_STATUS_CHECKER)
        .layer(ErrorHandlingLayer::new())
        .catch_panic()
        .enable_tracing()
        .rate_limit(DEFAULT_RATE_LIMIT, DEFAULT_RATE_LIMIT_DURATION)
        .retry(BackoffRetryPolicy::default())
        .concurrency(DEFAULT_CONCURRENCY)
        .data(app_state.clone())
        .backend(queue.transaction_status_queue.clone())
        .build_fn(transaction_status_handler);

    let notification_queue_worker = WorkerBuilder::new(NOTIFICATION_SENDER)
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .rate_limit(DEFAULT_RATE_LIMIT, DEFAULT_RATE_LIMIT_DURATION)
        .retry(BackoffRetryPolicy::default())
        .concurrency(DEFAULT_CONCURRENCY)
        .data(app_state.clone())
        .backend(queue.notification_queue.clone())
        .build_fn(notification_handler);

    let monitor_future = Monitor::new()
        .register(transaction_request_queue_worker)
        .register(transaction_submission_queue_worker)
        .register(transaction_status_queue_worker)
        .register(notification_queue_worker)
        .on_event(monitor_handle_event)
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

fn monitor_handle_event(e: Worker<Event>) {
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
}
