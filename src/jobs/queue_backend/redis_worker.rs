//! Redis/Apalis worker initialization.
//!
//! This module contains all Apalis-specific worker creation logic for the Redis
//! queue backend, including WorkerBuilder configurations, Monitor setup,
//! backoff strategies, and token swap cron workers.

use crate::{
    config::ServerConfig,
    constants::{
        DEFAULT_CONCURRENCY_HEALTH_CHECK, DEFAULT_CONCURRENCY_NOTIFICATION,
        DEFAULT_CONCURRENCY_STATUS_CHECKER, DEFAULT_CONCURRENCY_STATUS_CHECKER_EVM,
        DEFAULT_CONCURRENCY_STATUS_CHECKER_STELLAR, DEFAULT_CONCURRENCY_TOKEN_SWAP,
        DEFAULT_CONCURRENCY_TRANSACTION_REQUEST, DEFAULT_CONCURRENCY_TRANSACTION_SENDER,
        WORKER_NOTIFICATION_SENDER_RETRIES, WORKER_RELAYER_HEALTH_CHECK_RETRIES,
        WORKER_SYSTEM_CLEANUP_RETRIES, WORKER_TOKEN_SWAP_REQUEST_RETRIES,
        WORKER_TRANSACTION_CLEANUP_RETRIES, WORKER_TRANSACTION_REQUEST_RETRIES,
        WORKER_TRANSACTION_STATUS_CHECKER_RETRIES, WORKER_TRANSACTION_SUBMIT_RETRIES,
    },
    jobs::{
        notification_handler, relayer_health_check_handler, system_cleanup_handler,
        token_swap_cron_handler, token_swap_request_handler, transaction_cleanup_handler,
        transaction_request_handler, transaction_status_handler, transaction_submission_handler,
        JobProducerTrait,
    },
    models::{
        NetworkRepoModel, NotificationRepoModel, RelayerNetworkPolicy, RelayerRepoModel,
        SignerRepoModel, ThinDataAppState, TransactionRepoModel,
    },
    repositories::{
        ApiKeyRepositoryTrait, NetworkRepository, PluginRepositoryTrait, RelayerRepository,
        Repository, TransactionCounterTrait, TransactionRepository,
    },
};
use apalis::prelude::*;

use apalis::layers::retry::backoff::MakeBackoff;
use apalis::layers::retry::{backoff::ExponentialBackoffMaker, RetryPolicy};
use apalis::layers::ErrorHandlingLayer;

/// Re-exports from [`tower::util`]
pub use tower::util::rng::HasherRng;

use apalis_cron::CronStream;
use eyre::Result;
use std::{str::FromStr, time::Duration};
use tokio::signal::unix::SignalKind;
use tracing::{debug, error, info};

use super::types::filter_relayers_for_swap;

const TRANSACTION_REQUEST: &str = "transaction_request";
const TRANSACTION_SENDER: &str = "transaction_sender";
// Generic transaction status checker
const TRANSACTION_STATUS_CHECKER: &str = "transaction_status_checker";
// Network specific status checkers
const TRANSACTION_STATUS_CHECKER_EVM: &str = "transaction_status_checker_evm";
const TRANSACTION_STATUS_CHECKER_STELLAR: &str = "transaction_status_checker_stellar";
const NOTIFICATION_SENDER: &str = "notification_sender";
const TOKEN_SWAP_REQUEST: &str = "token_swap_request";
const TRANSACTION_CLEANUP: &str = "transaction_cleanup";
const RELAYER_HEALTH_CHECK: &str = "relayer_health_check";
const SYSTEM_CLEANUP: &str = "system_cleanup";

/// Creates an exponential backoff with configurable parameters
///
/// # Arguments
/// * `initial_ms` - Initial delay in milliseconds (e.g., 200)
/// * `max_ms` - Maximum delay in milliseconds (e.g., 5000)
/// * `jitter` - Jitter factor 0.0-1.0 (e.g., 0.99 for high jitter)
///
/// # Returns
/// A configured backoff instance ready for use with RetryPolicy
fn create_backoff(initial_ms: u64, max_ms: u64, jitter: f64) -> Result<ExponentialBackoffMaker> {
    let maker = ExponentialBackoffMaker::new(
        Duration::from_millis(initial_ms),
        Duration::from_millis(max_ms),
        jitter,
        HasherRng::default(),
    )?;

    Ok(maker)
}

pub async fn initialize_redis_workers<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    app_state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
) -> Result<()>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
    AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
{
    let queue = app_state.job_producer.get_queue().await?;

    let transaction_request_queue_worker = WorkerBuilder::new(TRANSACTION_REQUEST)
        .layer(ErrorHandlingLayer::new())
        .retry(
            RetryPolicy::retries(WORKER_TRANSACTION_REQUEST_RETRIES)
                .with_backoff(create_backoff(500, 5000, 0.99)?.make_backoff()),
        )
        .enable_tracing()
        .catch_panic()
        .concurrency(ServerConfig::get_worker_concurrency(
            TRANSACTION_REQUEST,
            DEFAULT_CONCURRENCY_TRANSACTION_REQUEST,
        ))
        .data(app_state.clone())
        .backend(queue.transaction_request_queue.clone())
        .build_fn(transaction_request_handler);

    let transaction_submission_queue_worker = WorkerBuilder::new(TRANSACTION_SENDER)
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .retry(
            RetryPolicy::retries(WORKER_TRANSACTION_SUBMIT_RETRIES)
                .with_backoff(create_backoff(500, 2000, 0.99)?.make_backoff()),
        )
        .concurrency(ServerConfig::get_worker_concurrency(
            TRANSACTION_SENDER,
            DEFAULT_CONCURRENCY_TRANSACTION_SENDER,
        ))
        .data(app_state.clone())
        .backend(queue.transaction_submission_queue.clone())
        .build_fn(transaction_submission_handler);

    // Generic status checker
    // Uses medium settings that work reasonably for most chains
    let transaction_status_queue_worker = WorkerBuilder::new(TRANSACTION_STATUS_CHECKER)
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .retry(
            RetryPolicy::retries(WORKER_TRANSACTION_STATUS_CHECKER_RETRIES)
                .with_backoff(create_backoff(5000, 8000, 0.99)?.make_backoff()),
        )
        .concurrency(ServerConfig::get_worker_concurrency(
            TRANSACTION_STATUS_CHECKER,
            DEFAULT_CONCURRENCY_STATUS_CHECKER,
        ))
        .data(app_state.clone())
        .backend(queue.transaction_status_queue.clone())
        .build_fn(transaction_status_handler);

    // EVM status checker - slower retries to avoid premature resubmission
    // EVM has longer block times (~12s) and needs time for resubmission logic
    let transaction_status_queue_worker_evm = WorkerBuilder::new(TRANSACTION_STATUS_CHECKER_EVM)
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .retry(
            RetryPolicy::retries(WORKER_TRANSACTION_STATUS_CHECKER_RETRIES)
                .with_backoff(create_backoff(8000, 12000, 0.99)?.make_backoff()),
        )
        .concurrency(ServerConfig::get_worker_concurrency(
            TRANSACTION_STATUS_CHECKER_EVM,
            DEFAULT_CONCURRENCY_STATUS_CHECKER_EVM,
        ))
        .data(app_state.clone())
        .backend(queue.transaction_status_queue_evm.clone())
        .build_fn(transaction_status_handler);

    // Stellar status checker - fast retries for fast finality
    // Stellar has sub-second finality, needs more frequent status checks
    let transaction_status_queue_worker_stellar =
        WorkerBuilder::new(TRANSACTION_STATUS_CHECKER_STELLAR)
            .layer(ErrorHandlingLayer::new())
            .enable_tracing()
            .catch_panic()
            .retry(
                RetryPolicy::retries(WORKER_TRANSACTION_STATUS_CHECKER_RETRIES)
                    .with_backoff(create_backoff(2000, 3000, 0.99)?.make_backoff()),
            )
            .concurrency(ServerConfig::get_worker_concurrency(
                TRANSACTION_STATUS_CHECKER_STELLAR,
                DEFAULT_CONCURRENCY_STATUS_CHECKER_STELLAR,
            ))
            .data(app_state.clone())
            .backend(queue.transaction_status_queue_stellar.clone())
            .build_fn(transaction_status_handler);

    let notification_queue_worker = WorkerBuilder::new(NOTIFICATION_SENDER)
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .retry(
            RetryPolicy::retries(WORKER_NOTIFICATION_SENDER_RETRIES)
                .with_backoff(create_backoff(2000, 8000, 0.99)?.make_backoff()),
        )
        .concurrency(ServerConfig::get_worker_concurrency(
            NOTIFICATION_SENDER,
            DEFAULT_CONCURRENCY_NOTIFICATION,
        ))
        .data(app_state.clone())
        .backend(queue.notification_queue.clone())
        .build_fn(notification_handler);

    let token_swap_request_queue_worker = WorkerBuilder::new(TOKEN_SWAP_REQUEST)
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .retry(
            RetryPolicy::retries(WORKER_TOKEN_SWAP_REQUEST_RETRIES)
                .with_backoff(create_backoff(5000, 20000, 0.99)?.make_backoff()),
        )
        .concurrency(ServerConfig::get_worker_concurrency(
            TOKEN_SWAP_REQUEST,
            DEFAULT_CONCURRENCY_TOKEN_SWAP,
        ))
        .data(app_state.clone())
        .backend(queue.token_swap_request_queue.clone())
        .build_fn(token_swap_request_handler);

    let transaction_cleanup_queue_worker = WorkerBuilder::new(TRANSACTION_CLEANUP)
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .retry(
            RetryPolicy::retries(WORKER_TRANSACTION_CLEANUP_RETRIES)
                .with_backoff(create_backoff(5000, 20000, 0.99)?.make_backoff()),
        )
        .concurrency(ServerConfig::get_worker_concurrency(TRANSACTION_CLEANUP, 1)) // Default to 1 to avoid DB conflicts
        .data(app_state.clone())
        .backend(CronStream::new(
            // every 10 minutes
            apalis_cron::Schedule::from_str("0 */10 * * * *")?,
        ))
        .build_fn(transaction_cleanup_handler);

    let system_cleanup_queue_worker = WorkerBuilder::new(SYSTEM_CLEANUP)
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .retry(
            RetryPolicy::retries(WORKER_SYSTEM_CLEANUP_RETRIES)
                .with_backoff(create_backoff(5000, 20000, 0.99)?.make_backoff()),
        )
        .concurrency(1)
        .data(app_state.clone())
        .backend(CronStream::new(
            // Runs at the start of every hour
            apalis_cron::Schedule::from_str("0 0 * * * *")?,
        ))
        .build_fn(system_cleanup_handler);

    let relayer_health_check_worker = WorkerBuilder::new(RELAYER_HEALTH_CHECK)
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .retry(
            RetryPolicy::retries(WORKER_RELAYER_HEALTH_CHECK_RETRIES)
                .with_backoff(create_backoff(2000, 10000, 0.99)?.make_backoff()),
        )
        .concurrency(ServerConfig::get_worker_concurrency(
            RELAYER_HEALTH_CHECK,
            DEFAULT_CONCURRENCY_HEALTH_CHECK,
        ))
        .data(app_state.clone())
        .backend(queue.relayer_health_check_queue.clone())
        .build_fn(relayer_health_check_handler);

    let monitor = Monitor::new()
        .register(transaction_request_queue_worker)
        .register(transaction_submission_queue_worker)
        .register(transaction_status_queue_worker)
        .register(transaction_status_queue_worker_evm)
        .register(transaction_status_queue_worker_stellar)
        .register(notification_queue_worker)
        .register(token_swap_request_queue_worker)
        .register(transaction_cleanup_queue_worker)
        .register(system_cleanup_queue_worker)
        .register(relayer_health_check_worker)
        .on_event(monitor_handle_event)
        .shutdown_timeout(Duration::from_millis(5000));

    let monitor_future = monitor.run_with_signal(async {
        let mut sigint = tokio::signal::unix::signal(SignalKind::interrupt())
            .expect("Failed to create SIGINT signal");
        let mut sigterm = tokio::signal::unix::signal(SignalKind::terminate())
            .expect("Failed to create SIGTERM signal");

        debug!("Workers monitor started");

        tokio::select! {
            _ = sigint.recv() => debug!("Received SIGINT."),
            _ = sigterm.recv() => debug!("Received SIGTERM."),
        };

        debug!("Workers monitor shutting down");

        Ok(())
    });
    tokio::spawn(async move {
        if let Err(e) = monitor_future.await {
            error!(error = %e, "monitor error");
        }
    });
    debug!("Workers monitor shutdown complete");

    Ok(())
}

/// Initializes swap workers for Solana and Stellar relayers.
/// This function creates and registers workers for relayers that have swap enabled and cron schedule set.
pub async fn initialize_redis_token_swap_workers<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    app_state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
) -> Result<()>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
    AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
{
    let active_relayers = app_state.relayer_repository.list_active().await?;
    let relayers_with_swap_enabled = filter_relayers_for_swap(active_relayers);

    if relayers_with_swap_enabled.is_empty() {
        debug!("No relayers with swap enabled");
        return Ok(());
    }
    info!(
        "Found {} relayers with swap enabled",
        relayers_with_swap_enabled.len()
    );

    let mut workers = Vec::new();

    let swap_backoff = create_backoff(2000, 5000, 0.99)?.make_backoff();

    for relayer in relayers_with_swap_enabled {
        debug!(relayer = ?relayer, "found relayer with swap enabled");

        let (cron_schedule, network_type) = match &relayer.policies {
            RelayerNetworkPolicy::Solana(policy) => match policy.get_swap_config() {
                Some(config) => match config.cron_schedule {
                    Some(schedule) => (schedule, "solana".to_string()),
                    None => {
                        debug!(relayer_id = %relayer.id, "No cron schedule specified for Solana relayer; skipping");
                        continue;
                    }
                },
                None => {
                    debug!(relayer_id = %relayer.id, "No swap configuration specified for Solana relayer; skipping");
                    continue;
                }
            },
            RelayerNetworkPolicy::Stellar(policy) => match policy.get_swap_config() {
                Some(config) => match config.cron_schedule {
                    Some(schedule) => (schedule, "stellar".to_string()),
                    None => {
                        debug!(relayer_id = %relayer.id, "No cron schedule specified for Stellar relayer; skipping");
                        continue;
                    }
                },
                None => {
                    debug!(relayer_id = %relayer.id, "No swap configuration specified for Stellar relayer; skipping");
                    continue;
                }
            },
            RelayerNetworkPolicy::Evm(_) => {
                debug!(relayer_id = %relayer.id, "EVM relayers do not support swap; skipping");
                continue;
            }
        };

        let calendar_schedule = match apalis_cron::Schedule::from_str(&cron_schedule) {
            Ok(schedule) => schedule,
            Err(e) => {
                error!(relayer_id = %relayer.id, error = %e, "Failed to parse cron schedule; skipping");
                continue;
            }
        };

        // Create worker and add to the workers vector
        let worker = WorkerBuilder::new(format!(
            "{}-swap-schedule-{}",
            network_type,
            relayer.id.clone()
        ))
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .retry(
            RetryPolicy::retries(WORKER_TOKEN_SWAP_REQUEST_RETRIES)
                .with_backoff(swap_backoff.clone()),
        )
        .concurrency(1)
        .data(relayer.id.clone())
        .data(app_state.clone())
        .backend(CronStream::new(calendar_schedule))
        .build_fn(token_swap_cron_handler);

        workers.push(worker);
        debug!(
            relayer_id = %relayer.id,
            network_type = %network_type,
            "Created worker for relayer with swap enabled"
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

        debug!("Swap Monitor started");

        tokio::select! {
            _ = sigint.recv() => debug!("Received SIGINT."),
            _ = sigterm.recv() => debug!("Received SIGTERM."),
        };

        debug!("Swap Monitor shutting down");

        Ok(())
    });
    tokio::spawn(async move {
        if let Err(e) = monitor_future.await {
            error!(error = %e, "monitor error");
        }
    });
    Ok(())
}

fn monitor_handle_event(e: Worker<Event>) {
    let worker_id = e.id();
    match e.inner() {
        Event::Engage(task_id) => {
            debug!(worker_id = %worker_id, task_id = %task_id, "worker got a job");
        }
        Event::Error(e) => {
            error!(worker_id = %worker_id, error = %e, "worker encountered an error");
        }
        Event::Exit => {
            debug!(worker_id = %worker_id, "worker exited");
        }
        Event::Idle => {
            debug!(worker_id = %worker_id, "worker is idle");
        }
        Event::Start => {
            debug!(worker_id = %worker_id, "worker started");
        }
        Event::Stop => {
            debug!(worker_id = %worker_id, "worker stopped");
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_backoff_with_valid_parameters() {
        let result = create_backoff(200, 5000, 0.99);
        assert!(
            result.is_ok(),
            "Should create backoff with valid parameters"
        );
    }

    #[test]
    fn test_create_backoff_with_zero_initial() {
        let result = create_backoff(0, 5000, 0.99);
        assert!(
            result.is_ok(),
            "Should handle zero initial delay (edge case)"
        );
    }

    #[test]
    fn test_create_backoff_with_equal_initial_and_max() {
        let result = create_backoff(1000, 1000, 0.5);
        assert!(result.is_ok(), "Should handle equal initial and max delays");
    }

    #[test]
    fn test_create_backoff_with_zero_jitter() {
        let result = create_backoff(500, 5000, 0.0);
        assert!(result.is_ok(), "Should handle zero jitter");
    }

    #[test]
    fn test_create_backoff_with_max_jitter() {
        let result = create_backoff(500, 5000, 1.0);
        assert!(result.is_ok(), "Should handle maximum jitter (1.0)");
    }

    #[test]
    fn test_create_backoff_with_small_values() {
        let result = create_backoff(1, 10, 0.5);
        assert!(result.is_ok(), "Should handle very small delay values");
    }

    #[test]
    fn test_create_backoff_with_large_values() {
        let result = create_backoff(10000, 60000, 0.99);
        assert!(result.is_ok(), "Should handle large delay values");
    }
}
