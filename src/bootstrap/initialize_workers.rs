//! # Workers
//! Initialise and starts the workers for the application

use actix_web::web::ThinData;
use apalis::{layers::ErrorHandlingLayer, prelude::*};
use apalis_cron::CronStream;
use eyre::Result;
use log::{error, info};
use std::{str::FromStr, time::Duration};
use tokio::signal::unix::SignalKind;

use crate::{
    jobs::{
        notification_handler, solana_token_swap_cron_handler, solana_token_swap_request_handler,
        transaction_cleanup_handler, transaction_request_handler, transaction_status_handler,
        transaction_submission_handler, BackoffRetryPolicy,
    },
    models::DefaultAppState,
    repositories::RelayerRepository,
};

// Review and fine tune configuration for the workers
const DEFAULT_CONCURRENCY: usize = 2;
const DEFAULT_RATE_LIMIT: u64 = 20;
const DEFAULT_RATE_LIMIT_DURATION: Duration = Duration::from_secs(1);

const TRANSACTION_REQUEST: &str = "transaction_request";
const TRANSACTION_SENDER: &str = "transaction_sender";
const TRANSACTION_STATUS_CHECKER: &str = "transaction_status_checker";
const NOTIFICATION_SENDER: &str = "notification_sender";
const SOLANA_TOKEN_SWAP_REQUEST: &str = "solana_token_swap_request";
const TRANSACTION_CLEANUP: &str = "transaction_cleanup";

pub async fn initialize_workers(app_state: ThinData<DefaultAppState>) -> Result<()> {
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

    let solana_token_swap_request_queue_worker = WorkerBuilder::new(SOLANA_TOKEN_SWAP_REQUEST)
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .rate_limit(DEFAULT_RATE_LIMIT, DEFAULT_RATE_LIMIT_DURATION)
        .retry(BackoffRetryPolicy::default())
        .concurrency(10)
        .data(app_state.clone())
        .backend(queue.solana_token_swap_request_queue.clone())
        .build_fn(solana_token_swap_request_handler);

    let transaction_cleanup_queue_worker = WorkerBuilder::new(TRANSACTION_CLEANUP)
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .rate_limit(DEFAULT_RATE_LIMIT, DEFAULT_RATE_LIMIT_DURATION)
        .retry(BackoffRetryPolicy::default())
        .concurrency(1)
        .data(app_state.clone())
        .backend(CronStream::new(
            // every 30 minutes
            apalis_cron::Schedule::from_str("0 */30 * * * *").unwrap(),
        ))
        .build_fn(transaction_cleanup_handler);

    let monitor = Monitor::new()
        .register(transaction_request_queue_worker)
        .register(transaction_submission_queue_worker)
        .register(transaction_status_queue_worker)
        .register(notification_queue_worker)
        .register(solana_token_swap_request_queue_worker)
        .register(transaction_cleanup_queue_worker)
        .on_event(monitor_handle_event)
        .shutdown_timeout(Duration::from_millis(5000));

    let monitor_future = monitor.run_with_signal(async {
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

/// Initializes the Solana swap workers
/// This function creates and registers workers for Solana relayers that have swap enabled and cron schedule set.
pub async fn initialize_solana_swap_workers(app_state: ThinData<DefaultAppState>) -> Result<()> {
    let solena_relayers_with_swap_enabled = app_state
        .relayer_repository
        .list_active()
        .await?
        .into_iter()
        .filter(|relayer| {
            let policy = relayer.policies.get_solana_policy();
            let swap_config = match policy.get_swap_config() {
                Some(config) => config,
                None => {
                    info!("No swap configuration specified; skipping validation.");
                    return false;
                }
            };

            if swap_config.cron_schedule.is_none() {
                return false;
            }
            true
        })
        .collect::<Vec<_>>();

    if solena_relayers_with_swap_enabled.is_empty() {
        info!("No solana relayers with swap enabled");
        return Ok(());
    }
    info!(
        "Found {} solana relayers with swap enabled",
        solena_relayers_with_swap_enabled.len()
    );

    let mut workers = Vec::new();

    for relayer in solena_relayers_with_swap_enabled {
        info!("Found solana relayer with swap enabled: {:?}", relayer);

        let policy = relayer.policies.get_solana_policy();
        let swap_config = match policy.get_swap_config() {
            Some(config) => config,
            None => {
                info!("No swap configuration specified; skipping validation.");
                continue;
            }
        };

        let calendar_schedule = match swap_config.cron_schedule {
            Some(schedule) => apalis_cron::Schedule::from_str(&schedule).unwrap(),
            None => {
                info!("No swap cron schedule found for relayer: {:?}", relayer);
                continue;
            }
        };

        // Create worker and add to the workers vector
        let worker = WorkerBuilder::new(format!("solana-swap-schedule-{}", relayer.id.clone()))
            .layer(ErrorHandlingLayer::new())
            .enable_tracing()
            .catch_panic()
            .rate_limit(DEFAULT_RATE_LIMIT, DEFAULT_RATE_LIMIT_DURATION)
            .retry(BackoffRetryPolicy::default())
            .concurrency(1)
            .data(relayer.id.clone())
            .data(app_state.clone())
            .backend(CronStream::new(calendar_schedule))
            .build_fn(solana_token_swap_cron_handler);

        workers.push(worker);
        info!(
            "Created worker for solana relayer with swap enabled: {:?}",
            relayer
        );
    }

    let mut monitor = Monitor::new()
        .on_event(monitor_handle_event)
        .shutdown_timeout(Duration::from_millis(5000));

    // Register all workers with the monitor
    for worker in workers {
        monitor = monitor.register(worker);
    }

    let monitor_future = monitor.run_with_signal(async {
        let mut sigint = tokio::signal::unix::signal(SignalKind::interrupt())
            .expect("Failed to create SIGINT signal");
        let mut sigterm = tokio::signal::unix::signal(SignalKind::terminate())
            .expect("Failed to create SIGTERM signal");

        info!("Solana Swap Monitor started");

        tokio::select! {
            _ = sigint.recv() => info!("Received SIGINT."),
            _ = sigterm.recv() => info!("Received SIGTERM."),
        };

        info!("Solana Swap Monitor shutting down");

        Ok(())
    });
    tokio::spawn(async move {
        if let Err(e) = monitor_future.await {
            error!("Monitor error: {}", e);
        }
    });
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
