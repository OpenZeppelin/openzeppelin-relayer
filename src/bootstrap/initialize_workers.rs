//! Worker initialization
//!
//! This module contains functions for initializing background workers,
//! including job processors and other long-running tasks.
use crate::{
    jobs::{
        notification_handler, relayer_health_check_handler, solana_token_swap_cron_handler,
        solana_token_swap_request_handler, transaction_cleanup_handler,
        transaction_request_handler, transaction_status_handler, transaction_submission_handler,
        BackoffRetryPolicy, JobProducerTrait,
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
use apalis::{layers::ErrorHandlingLayer, prelude::*};
use apalis_cron::CronStream;
use eyre::Result;
use std::{str::FromStr, time::Duration};
use tokio::signal::unix::SignalKind;
use tracing::{debug, error, info};

// Worker configuration constants
const DEFAULT_CONCURRENCY: usize = 10;
const DEFAULT_RATE_LIMIT: u64 = 100;
const DEFAULT_RATE_LIMIT_DURATION: Duration = Duration::from_secs(1);

const TRANSACTION_REQUEST: &str = "transaction_request";
const TRANSACTION_SENDER: &str = "transaction_sender";
const TRANSACTION_STATUS_CHECKER: &str = "transaction_status_checker";
const NOTIFICATION_SENDER: &str = "notification_sender";
const SOLANA_TOKEN_SWAP_REQUEST: &str = "solana_token_swap_request";
const TRANSACTION_CLEANUP: &str = "transaction_cleanup";
const RELAYER_HEALTH_CHECK: &str = "relayer_health_check";

/// Configuration for a worker
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub name: &'static str,
    pub concurrency: usize,
    pub rate_limit: u64,
    pub rate_limit_duration: Duration,
    /// Optional cron schedule for periodic workers (e.g., cleanup tasks)
    pub cron_schedule: Option<&'static str>,
}

impl WorkerConfig {
    pub const fn new(name: &'static str, concurrency: usize) -> Self {
        Self {
            name,
            concurrency,
            rate_limit: DEFAULT_RATE_LIMIT,
            rate_limit_duration: DEFAULT_RATE_LIMIT_DURATION,
            cron_schedule: None,
        }
    }

    pub const fn with_cron(name: &'static str, concurrency: usize, cron: &'static str) -> Self {
        Self {
            name,
            concurrency,
            rate_limit: DEFAULT_RATE_LIMIT,
            rate_limit_duration: DEFAULT_RATE_LIMIT_DURATION,
            cron_schedule: Some(cron),
        }
    }
}

/// Get worker configurations for all standard workers
///
/// Concurrency reasoning:
/// - Transaction processing (request/sender/status): I/O-bound blockchain calls → high concurrency
/// - Notifications: I/O-bound webhook calls → high concurrency
/// - Solana swap: DEX operations with multiple tokens → very high concurrency
/// - Cleanup: Resource-intensive DB operations → low concurrency to avoid conflicts
/// - Health checks: Multiple relayers checked independently → moderate-high concurrency
pub fn get_worker_configs() -> Vec<WorkerConfig> {
    vec![
        // High concurrency for I/O-bound blockchain operations
        WorkerConfig::new(TRANSACTION_REQUEST, DEFAULT_CONCURRENCY), // 10: Validate/parse requests
        WorkerConfig::new(TRANSACTION_SENDER, DEFAULT_CONCURRENCY),  // 10: Submit to blockchains
        WorkerConfig::new(TRANSACTION_STATUS_CHECKER, 20), // 20: Poll RPC endpoints (read-only)
        // High concurrency for webhook notifications (I/O-bound HTTP calls)
        WorkerConfig::new(NOTIFICATION_SENDER, DEFAULT_CONCURRENCY), // 10: Send webhooks
        // Very high concurrency for DEX operations (multiple tokens/swaps)
        WorkerConfig::new(SOLANA_TOKEN_SWAP_REQUEST, 25), // 25: Handle swap requests
        // Low concurrency for resource-intensive periodic cleanup (cron-based)
        WorkerConfig::with_cron(
            TRANSACTION_CLEANUP,
            1,                // 1: Avoid DB conflicts
            "0 */30 * * * *", // Every 30 minutes
        ),
        // Moderate-high concurrency for independent health checks
        WorkerConfig::new(RELAYER_HEALTH_CHECK, 10), // 10: Check multiple relayers
    ]
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
    let queue = app_state.job_producer().get_queue().await?;
    let configs = get_worker_configs();

    // Helper function to find config by name
    let get_config = |name: &str| -> &WorkerConfig {
        configs
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("Worker config for '{}' should exist", name))
    };

    let tx_req_config = get_config(TRANSACTION_REQUEST);
    let transaction_request_queue_worker = WorkerBuilder::new(tx_req_config.name)
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .rate_limit(tx_req_config.rate_limit, tx_req_config.rate_limit_duration)
        .retry(BackoffRetryPolicy::default())
        .concurrency(tx_req_config.concurrency)
        .data(app_state.clone())
        .backend(queue.transaction_request_queue.clone())
        .build_fn(transaction_request_handler);

    let tx_sub_config = get_config(TRANSACTION_SENDER);
    let transaction_submission_queue_worker = WorkerBuilder::new(tx_sub_config.name)
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .rate_limit(tx_sub_config.rate_limit, tx_sub_config.rate_limit_duration)
        .retry(BackoffRetryPolicy::default())
        .concurrency(tx_sub_config.concurrency)
        .data(app_state.clone())
        .backend(queue.transaction_submission_queue.clone())
        .build_fn(transaction_submission_handler);

    let tx_status_config = get_config(TRANSACTION_STATUS_CHECKER);
    let transaction_status_queue_worker = WorkerBuilder::new(tx_status_config.name)
        .layer(ErrorHandlingLayer::new())
        .catch_panic()
        .enable_tracing()
        .rate_limit(
            tx_status_config.rate_limit,
            tx_status_config.rate_limit_duration,
        )
        .retry(BackoffRetryPolicy::default())
        .concurrency(tx_status_config.concurrency)
        .data(app_state.clone())
        .backend(queue.transaction_status_queue.clone())
        .build_fn(transaction_status_handler);

    let notif_config = get_config(NOTIFICATION_SENDER);
    let notification_queue_worker = WorkerBuilder::new(notif_config.name)
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .rate_limit(notif_config.rate_limit, notif_config.rate_limit_duration)
        .retry(BackoffRetryPolicy::default())
        .concurrency(notif_config.concurrency)
        .data(app_state.clone())
        .backend(queue.notification_queue.clone())
        .build_fn(notification_handler);

    let swap_config = get_config(SOLANA_TOKEN_SWAP_REQUEST);
    let solana_token_swap_request_queue_worker = WorkerBuilder::new(swap_config.name)
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .rate_limit(swap_config.rate_limit, swap_config.rate_limit_duration)
        .retry(BackoffRetryPolicy::default())
        .concurrency(swap_config.concurrency)
        .data(app_state.clone())
        .backend(queue.solana_token_swap_request_queue.clone())
        .build_fn(solana_token_swap_request_handler);

    let cleanup_config = get_config(TRANSACTION_CLEANUP);
    let transaction_cleanup_queue_worker = WorkerBuilder::new(cleanup_config.name)
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .rate_limit(
            cleanup_config.rate_limit,
            cleanup_config.rate_limit_duration,
        )
        .retry(BackoffRetryPolicy::default())
        .concurrency(cleanup_config.concurrency)
        .data(app_state.clone())
        .backend(CronStream::new(
            apalis_cron::Schedule::from_str(
                cleanup_config
                    .cron_schedule
                    .expect("TRANSACTION_CLEANUP should have cron schedule"),
            )
            .expect("Valid cron schedule"),
        ))
        .build_fn(transaction_cleanup_handler);

    let health_config = get_config(RELAYER_HEALTH_CHECK);
    let relayer_health_check_worker = WorkerBuilder::new(health_config.name)
        .layer(ErrorHandlingLayer::new())
        .enable_tracing()
        .catch_panic()
        .rate_limit(health_config.rate_limit, health_config.rate_limit_duration)
        .retry(BackoffRetryPolicy::default())
        .concurrency(health_config.concurrency)
        .data(app_state.clone())
        .backend(queue.relayer_health_check_queue.clone())
        .build_fn(relayer_health_check_handler);

    let monitor = Monitor::new()
        .register(transaction_request_queue_worker)
        .register(transaction_submission_queue_worker)
        .register(transaction_status_queue_worker)
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
            error!(error = %e, "monitor error");
        }
    });
    info!("Monitor shutdown complete");
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
        info!("No solana relayers with swap enabled");
        return Ok(());
    }
    info!(
        "Found {} solana relayers with swap enabled",
        solena_relayers_with_swap_enabled.len()
    );

    let mut workers = Vec::new();

    for relayer in solena_relayers_with_swap_enabled {
        debug!(relayer = ?relayer, "found solana relayer with swap enabled");

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
                debug!(relayer = ?relayer, "no swap cron schedule found for relayer");
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

    // ===== Worker Configuration Tests =====

    #[test]
    fn test_get_worker_configs_returns_all_workers() {
        let configs = get_worker_configs();

        assert_eq!(configs.len(), 7, "Should have 7 standard workers");
    }

    #[test]
    fn test_worker_configs_have_correct_names() {
        let configs = get_worker_configs();
        let names: Vec<&str> = configs.iter().map(|c| c.name).collect();

        // Verify all expected workers are present
        assert!(
            names.contains(&TRANSACTION_REQUEST),
            "Should include transaction_request"
        );
        assert!(
            names.contains(&TRANSACTION_SENDER),
            "Should include transaction_sender"
        );
        assert!(
            names.contains(&TRANSACTION_STATUS_CHECKER),
            "Should include transaction_status_checker"
        );
        assert!(
            names.contains(&NOTIFICATION_SENDER),
            "Should include notification_sender"
        );
        assert!(
            names.contains(&SOLANA_TOKEN_SWAP_REQUEST),
            "Should include solana_token_swap_request"
        );
        assert!(
            names.contains(&TRANSACTION_CLEANUP),
            "Should include transaction_cleanup"
        );
        assert!(
            names.contains(&RELAYER_HEALTH_CHECK),
            "Should include relayer_health_check"
        );
    }

    #[test]
    fn test_worker_configs_have_correct_concurrency() {
        let configs = get_worker_configs();

        for config in configs {
            match config.name {
                TRANSACTION_REQUEST => assert_eq!(
                    config.concurrency, 10,
                    "transaction_request should have concurrency 10 (I/O-bound)"
                ),
                TRANSACTION_SENDER => assert_eq!(
                    config.concurrency, 10,
                    "transaction_sender should have concurrency 10 (I/O-bound)"
                ),
                TRANSACTION_STATUS_CHECKER => assert_eq!(
                    config.concurrency, 20,
                    "transaction_status_checker should have concurrency 20 (read-only RPC)"
                ),
                NOTIFICATION_SENDER => assert_eq!(
                    config.concurrency, 10,
                    "notification_sender should have concurrency 10 (I/O-bound)"
                ),
                SOLANA_TOKEN_SWAP_REQUEST => assert_eq!(
                    config.concurrency, 25,
                    "solana_token_swap_request should have concurrency 25 (DEX operations)"
                ),
                TRANSACTION_CLEANUP => assert_eq!(
                    config.concurrency, 1,
                    "transaction_cleanup should have concurrency 1 (DB conflicts)"
                ),
                RELAYER_HEALTH_CHECK => assert_eq!(
                    config.concurrency, 10,
                    "relayer_health_check should have concurrency 10 (independent checks)"
                ),
                _ => panic!("Unexpected worker name: {}", config.name),
            }
        }
    }

    #[test]
    fn test_worker_configs_have_default_rate_limits() {
        let configs = get_worker_configs();

        for config in configs {
            assert_eq!(
                config.rate_limit, DEFAULT_RATE_LIMIT,
                "Worker {} should have default rate limit",
                config.name
            );
            assert_eq!(
                config.rate_limit_duration, DEFAULT_RATE_LIMIT_DURATION,
                "Worker {} should have default rate limit duration",
                config.name
            );
        }
    }

    #[test]
    fn test_worker_config_constants_are_unique() {
        let configs = get_worker_configs();
        let names: Vec<&str> = configs.iter().map(|c| c.name).collect();
        let mut unique_names = names.clone();
        unique_names.sort();
        unique_names.dedup();

        assert_eq!(
            names.len(),
            unique_names.len(),
            "All worker names should be unique"
        );
    }

    #[test]
    fn test_worker_constants_match_strings() {
        // Verify constants are set correctly
        assert_eq!(TRANSACTION_REQUEST, "transaction_request");
        assert_eq!(TRANSACTION_SENDER, "transaction_sender");
        assert_eq!(TRANSACTION_STATUS_CHECKER, "transaction_status_checker");
        assert_eq!(NOTIFICATION_SENDER, "notification_sender");
        assert_eq!(SOLANA_TOKEN_SWAP_REQUEST, "solana_token_swap_request");
        assert_eq!(TRANSACTION_CLEANUP, "transaction_cleanup");
        assert_eq!(RELAYER_HEALTH_CHECK, "relayer_health_check");
    }

    #[test]
    fn test_default_configuration_constants() {
        // Verify defaults are sensible for I/O-bound operations
        assert_eq!(
            DEFAULT_CONCURRENCY, 10,
            "Default concurrency should be 10 for I/O-bound ops"
        );
        assert_eq!(
            DEFAULT_RATE_LIMIT, 100,
            "Default rate limit should be 100 for higher throughput"
        );
        assert_eq!(
            DEFAULT_RATE_LIMIT_DURATION,
            Duration::from_secs(1),
            "Default rate limit duration should be 1 second"
        );
    }

    #[test]
    fn test_specialized_worker_concurrency() {
        let configs = get_worker_configs();

        // Very high concurrency for DEX swap operations
        let swap_worker = configs
            .iter()
            .find(|c| c.name == SOLANA_TOKEN_SWAP_REQUEST)
            .expect("Should have solana swap worker");
        assert_eq!(
            swap_worker.concurrency, 25,
            "Solana swap should handle 25 concurrent requests (DEX operations)"
        );

        // Highest concurrency for read-only status checks
        let status_worker = configs
            .iter()
            .find(|c| c.name == TRANSACTION_STATUS_CHECKER)
            .expect("Should have status checker");
        assert_eq!(
            status_worker.concurrency, 20,
            "Status checker should have high concurrency 20 (read-only RPC calls)"
        );

        // Low concurrency for cleanup (resource-intensive, avoid DB conflicts)
        let cleanup_worker = configs
            .iter()
            .find(|c| c.name == TRANSACTION_CLEANUP)
            .expect("Should have cleanup worker");
        assert_eq!(
            cleanup_worker.concurrency, 1,
            "Cleanup should run single-threaded to avoid DB conflicts"
        );

        // High concurrency for independent health checks
        let health_worker = configs
            .iter()
            .find(|c| c.name == RELAYER_HEALTH_CHECK)
            .expect("Should have health check worker");
        assert_eq!(
            health_worker.concurrency, 10,
            "Health checks should handle 10 concurrent relayers"
        );
    }

    #[test]
    fn test_cron_scheduled_workers() {
        let configs = get_worker_configs();

        // Find cleanup worker which should have a cron schedule
        let cleanup_worker = configs
            .iter()
            .find(|c| c.name == TRANSACTION_CLEANUP)
            .expect("Should have cleanup worker");

        assert!(
            cleanup_worker.cron_schedule.is_some(),
            "Cleanup worker should have a cron schedule"
        );

        assert_eq!(
            cleanup_worker.cron_schedule,
            Some("0 */30 * * * *"),
            "Cleanup should run every 30 minutes"
        );

        // Verify the cron schedule is valid
        let schedule_str = cleanup_worker.cron_schedule.unwrap();
        let schedule = apalis_cron::Schedule::from_str(schedule_str);
        assert!(
            schedule.is_ok(),
            "Cron schedule should be valid: {}",
            schedule_str
        );
    }

    #[test]
    fn test_non_cron_workers_have_no_schedule() {
        let configs = get_worker_configs();

        // Regular queue-based workers should not have cron schedules
        let queue_workers = vec![
            TRANSACTION_REQUEST,
            TRANSACTION_SENDER,
            TRANSACTION_STATUS_CHECKER,
            NOTIFICATION_SENDER,
            SOLANA_TOKEN_SWAP_REQUEST,
            RELAYER_HEALTH_CHECK,
        ];

        for worker_name in queue_workers {
            let worker = configs
                .iter()
                .find(|c| c.name == worker_name)
                .unwrap_or_else(|| panic!("Should have worker: {}", worker_name));

            assert!(
                worker.cron_schedule.is_none(),
                "Queue-based worker {} should not have cron schedule",
                worker_name
            );
        }
    }
}
