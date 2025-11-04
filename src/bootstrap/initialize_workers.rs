//! Worker initialization
//!
//! This module contains functions for initializing background workers,
//! including job processors and other long-running tasks.
use crate::{
    config::ServerConfig,
    constants::{
        DEFAULT_CONCURRENCY_HEALTH_CHECK, DEFAULT_CONCURRENCY_NOTIFICATION,
        DEFAULT_CONCURRENCY_SOLANA_SWAP, DEFAULT_CONCURRENCY_STATUS_CHECKER,
        DEFAULT_CONCURRENCY_STATUS_CHECKER_EVM, DEFAULT_CONCURRENCY_STATUS_CHECKER_STELLAR,
        DEFAULT_CONCURRENCY_TRANSACTION_REQUEST, DEFAULT_CONCURRENCY_TRANSACTION_SENDER,
        WORKER_NOTIFICATION_SENDER_RETRIES, WORKER_RELAYER_HEALTH_CHECK_RETRIES,
        WORKER_SOLANA_TOKEN_SWAP_REQUEST_RETRIES, WORKER_TRANSACTION_CLEANUP_RETRIES,
        WORKER_TRANSACTION_REQUEST_RETRIES, WORKER_TRANSACTION_STATUS_CHECKER_RETRIES,
        WORKER_TRANSACTION_SUBMIT_RETRIES,
    },
    jobs::{
        notification_handler, relayer_health_check_handler, solana_token_swap_cron_handler,
        solana_token_swap_request_handler, transaction_cleanup_handler,
        transaction_request_handler, transaction_status_handler, transaction_submission_handler,
        JobProducerTrait,
    },
    models::{
        NetworkRepoModel, NotificationRepoModel, RelayerRepoModel, SignerRepoModel,
        ThinDataAppState, TransactionRepoModel,
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

const TRANSACTION_REQUEST: &str = "transaction_request";
const TRANSACTION_SENDER: &str = "transaction_sender";
// Generic transaction status checker
const TRANSACTION_STATUS_CHECKER: &str = "transaction_status_checker";
// Network specific status checkers
const TRANSACTION_STATUS_CHECKER_EVM: &str = "transaction_status_checker_evm";
const TRANSACTION_STATUS_CHECKER_STELLAR: &str = "transaction_status_checker_stellar";
const NOTIFICATION_SENDER: &str = "notification_sender";
const SOLANA_TOKEN_SWAP_REQUEST: &str = "solana_token_swap_request";
const TRANSACTION_CLEANUP: &str = "transaction_cleanup";
const RELAYER_HEALTH_CHECK: &str = "relayer_health_check";

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

pub async fn initialize_workers<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
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

    let solana_token_swap_request_queue_worker = WorkerBuilder::new(SOLANA_TOKEN_SWAP_REQUEST)
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .retry(
            RetryPolicy::retries(WORKER_SOLANA_TOKEN_SWAP_REQUEST_RETRIES)
                .with_backoff(create_backoff(5000, 20000, 0.99)?.make_backoff()),
        )
        .concurrency(ServerConfig::get_worker_concurrency(
            SOLANA_TOKEN_SWAP_REQUEST,
            DEFAULT_CONCURRENCY_SOLANA_SWAP,
        ))
        .data(app_state.clone())
        .backend(queue.solana_token_swap_request_queue.clone())
        .build_fn(solana_token_swap_request_handler);

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
            // every 30 minutes
            apalis_cron::Schedule::from_str("0 */30 * * * *")?,
        ))
        .build_fn(transaction_cleanup_handler);

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
        .register(solana_token_swap_request_queue_worker)
        .register(transaction_cleanup_queue_worker)
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

/// Filters relayers to find those eligible for swap workers
/// Returns relayers that have:
/// 1. Solana network type
/// 2. Swap configuration
/// 3. Cron schedule defined
fn filter_relayers_for_swap(relayers: Vec<RelayerRepoModel>) -> Vec<RelayerRepoModel> {
    relayers
        .into_iter()
        .filter(|relayer| {
            let policy = relayer.policies.get_solana_policy();
            let swap_config = match policy.get_swap_config() {
                Some(config) => config,
                None => {
                    debug!(relayer_id = %relayer.id, "No swap configuration specified; skipping");
                    return false;
                }
            };

            if swap_config.cron_schedule.is_none() {
                debug!(relayer_id = %relayer.id, "No cron schedule specified; skipping");
                return false;
            }
            true
        })
        .collect()
}

/// Initializes the Solana swap workers
/// This function creates and registers workers for Solana relayers that have swap enabled and cron schedule set.
pub async fn initialize_solana_swap_workers<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
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
    let solena_relayers_with_swap_enabled = filter_relayers_for_swap(active_relayers);

    if solena_relayers_with_swap_enabled.is_empty() {
        debug!("No solana relayers with swap enabled");
        return Ok(());
    }
    info!(
        "Found {} solana relayers with swap enabled",
        solena_relayers_with_swap_enabled.len()
    );

    let mut workers = Vec::new();

    let swap_backoff = create_backoff(2000, 5000, 0.99)?.make_backoff();

    for relayer in solena_relayers_with_swap_enabled {
        debug!(relayer = ?relayer, "found solana relayer with swap enabled");

        let policy = relayer.policies.get_solana_policy();
        let swap_config = match policy.get_swap_config() {
            Some(config) => config,
            None => {
                debug!("No swap configuration specified; skipping validation.");
                continue;
            }
        };

        let calendar_schedule = match swap_config.cron_schedule {
            Some(schedule) => apalis_cron::Schedule::from_str(&schedule).unwrap(),
            None => {
                debug!(relayer = ?relayer, "no swap cron schedule found for relayer");
                continue;
            }
        };

        // Create worker and add to the workers vector
        let worker = WorkerBuilder::new(format!("solana-swap-schedule-{}", relayer.id.clone()))
            .layer(ErrorHandlingLayer::new())
            .enable_tracing()
            .catch_panic()
            .retry(
                RetryPolicy::retries(WORKER_SOLANA_TOKEN_SWAP_REQUEST_RETRIES)
                    .with_backoff(swap_backoff.clone()),
            )
            .concurrency(1)
            .data(relayer.id.clone())
            .data(app_state.clone())
            .backend(CronStream::new(calendar_schedule))
            .build_fn(solana_token_swap_cron_handler);

        workers.push(worker);
        debug!(
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

        debug!("Solana Swap Monitor started");

        tokio::select! {
            _ = sigint.recv() => debug!("Received SIGINT."),
            _ = sigterm.recv() => debug!("Received SIGTERM."),
        };

        debug!("Solana Swap Monitor shutting down");

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
    use crate::models::{
        NetworkType, RelayerEvmPolicy, RelayerNetworkPolicy, RelayerRepoModel, RelayerSolanaPolicy,
        RelayerSolanaSwapConfig,
    };

    fn create_test_evm_relayer(id: &str) -> RelayerRepoModel {
        RelayerRepoModel {
            id: id.to_string(),
            name: format!("EVM Relayer {}", id),
            network: "sepolia".to_string(),
            paused: false,
            network_type: NetworkType::Evm,
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
            signer_id: "test-signer".to_string(),
            address: "0x742d35Cc6634C0532925a3b8D8C2e48a73F6ba2E".to_string(),
            system_disabled: false,
            ..Default::default()
        }
    }

    fn create_test_solana_relayer_with_swap(
        id: &str,
        cron_schedule: Option<String>,
    ) -> RelayerRepoModel {
        RelayerRepoModel {
            id: id.to_string(),
            name: format!("Solana Relayer {}", id),
            network: "mainnet-beta".to_string(),
            paused: false,
            network_type: NetworkType::Solana,
            policies: RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
                min_balance: Some(1000000000),
                allowed_tokens: None,
                allowed_programs: None,
                max_signatures: None,
                max_tx_data_size: None,
                fee_payment_strategy: None,
                fee_margin_percentage: None,
                allowed_accounts: None,
                disallowed_accounts: None,
                max_allowed_fee_lamports: None,
                swap_config: Some(RelayerSolanaSwapConfig {
                    strategy: None,
                    cron_schedule,
                    min_balance_threshold: Some(5000000000),
                    jupiter_swap_options: None,
                }),
            }),
            signer_id: "test-signer".to_string(),
            address: "5zWma6gn4QxRfC6xZk6KfpXWXXgV3Xt6VzPpXMKCMYW5".to_string(),
            system_disabled: false,
            ..Default::default()
        }
    }

    #[test]
    fn test_filter_relayers_for_swap_with_empty_list() {
        let relayers = vec![];
        let filtered = filter_relayers_for_swap(relayers);

        assert_eq!(
            filtered.len(),
            0,
            "Should return empty list when no relayers provided"
        );
    }

    #[test]
    fn test_filter_relayers_for_swap_filters_non_solana() {
        let relayers = vec![
            create_test_evm_relayer("evm-1"),
            create_test_evm_relayer("evm-2"),
        ];

        let filtered = filter_relayers_for_swap(relayers);

        assert_eq!(
            filtered.len(),
            0,
            "Should filter out all non-Solana relayers"
        );
    }

    #[test]
    fn test_filter_relayers_for_swap_filters_no_cron_schedule() {
        let relayers = vec![
            create_test_solana_relayer_with_swap("solana-1", None),
            create_test_solana_relayer_with_swap("solana-2", None),
        ];

        let filtered = filter_relayers_for_swap(relayers);

        assert_eq!(
            filtered.len(),
            0,
            "Should filter out Solana relayers without cron schedule"
        );
    }

    #[test]
    fn test_filter_relayers_for_swap_includes_valid_relayers() {
        let relayers = vec![
            create_test_solana_relayer_with_swap("solana-1", Some("0 0 * * * *".to_string())),
            create_test_solana_relayer_with_swap("solana-2", Some("0 */2 * * * *".to_string())),
        ];

        let filtered = filter_relayers_for_swap(relayers);

        assert_eq!(
            filtered.len(),
            2,
            "Should include all Solana relayers with cron schedule"
        );
        assert_eq!(filtered[0].id, "solana-1");
        assert_eq!(filtered[1].id, "solana-2");
    }

    #[test]
    fn test_filter_relayers_for_swap_with_mixed_relayers() {
        let relayers = vec![
            create_test_evm_relayer("evm-1"),
            create_test_solana_relayer_with_swap("solana-no-cron", None),
            create_test_solana_relayer_with_swap(
                "solana-with-cron-1",
                Some("0 0 * * * *".to_string()),
            ),
            create_test_evm_relayer("evm-2"),
            create_test_solana_relayer_with_swap(
                "solana-with-cron-2",
                Some("0 */3 * * * *".to_string()),
            ),
        ];

        let filtered = filter_relayers_for_swap(relayers);

        assert_eq!(
            filtered.len(),
            2,
            "Should only include Solana relayers with cron schedule"
        );

        // Verify the correct relayers were included
        let ids: Vec<&str> = filtered.iter().map(|r| r.id.as_str()).collect();
        assert!(
            ids.contains(&"solana-with-cron-1"),
            "Should include solana-with-cron-1"
        );
        assert!(
            ids.contains(&"solana-with-cron-2"),
            "Should include solana-with-cron-2"
        );
        assert!(!ids.contains(&"evm-1"), "Should not include EVM relayers");
        assert!(
            !ids.contains(&"solana-no-cron"),
            "Should not include Solana without cron"
        );
    }

    #[test]
    fn test_filter_relayers_for_swap_preserves_relayer_data() {
        let cron = "0 1 * * * *".to_string();
        let relayers = vec![create_test_solana_relayer_with_swap(
            "test-relayer",
            Some(cron.clone()),
        )];

        let filtered = filter_relayers_for_swap(relayers);

        assert_eq!(filtered.len(), 1);

        let relayer = &filtered[0];
        assert_eq!(relayer.id, "test-relayer");
        assert_eq!(relayer.name, "Solana Relayer test-relayer");
        assert_eq!(relayer.network_type, NetworkType::Solana);

        // Verify swap config is preserved
        let policy = relayer.policies.get_solana_policy();
        let swap_config = policy.get_swap_config().expect("Should have swap config");
        assert_eq!(swap_config.cron_schedule.as_ref(), Some(&cron));
    }
}
