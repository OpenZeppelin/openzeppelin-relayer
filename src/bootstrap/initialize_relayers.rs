//! Relayer initialization
//!
//! This module contains functions for initializing relayers, ensuring they are
//! properly configured and ready for operation.
//!
//! ## Distributed Locking
//!
//! When multiple instances of the relayer service start simultaneously with
//! `DISTRIBUTED_MODE` enabled, this module uses distributed locking to coordinate
//! initialization and prevent duplicate work:
//!
//! - **Global lock**: A single lock is used for the entire initialization process,
//!   ensuring only one instance initializes relayers at a time.
//! - **Recent completion check**: Skips initialization if it was recently completed
//!   (within the staleness threshold) to handle rolling restarts efficiently.
//! - **Wait for completion**: Instances that don't acquire the lock wait for the
//!   initializing instance to complete, then proceed without re-initializing.
//! - **Single-instance mode**: When `DISTRIBUTED_MODE` is disabled (default) or using
//!   in-memory storage, locking is skipped since coordination is not needed.
use crate::{
    config::ServerConfig,
    domain::{get_network_relayer, Relayer},
    jobs::JobProducerTrait,
    models::{
        NetworkRepoModel, NotificationRepoModel, RelayerRepoModel, SignerRepoModel,
        ThinDataAppState, TransactionRepoModel,
    },
    repositories::{
        ApiKeyRepositoryTrait, NetworkRepository, PluginRepositoryTrait, RelayerRepository,
        Repository, TransactionCounterTrait, TransactionRepository,
    },
    utils::{
        is_global_init_recently_completed, poll_until, set_global_init_completed, DistributedLock,
        BOOTSTRAP_LOCK_TTL_SECS, LOCK_POLL_INTERVAL_MS, LOCK_WAIT_MAX_SECS,
    },
};
use color_eyre::{eyre::WrapErr, Result};
use deadpool_redis::Pool;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Staleness threshold in seconds. Initialization completed within this time is skipped.
/// Set to 5 minutes to prevent redundant initialization on rolling restarts.
const INIT_STALENESS_THRESHOLD_SECS: u64 = 300;

/// Lock name for global initialization lock.
const GLOBAL_INIT_LOCK_NAME: &str = "relayer_init_global";

/// Internal function for initializing a relayer using a provided relayer service.
/// This allows for easier testing with mocked relayers.
/// Uses generics for static dispatch instead of dynamic dispatch.
async fn initialize_relayer_with_service<R>(relayer_id: &str, relayer_service: &R) -> Result<()>
where
    R: Relayer,
{
    debug!(relayer_id = %relayer_id, "initializing relayer");

    relayer_service
        .initialize_relayer()
        .await
        .wrap_err_with(|| format!("Failed to initialize relayer: {relayer_id}"))?;

    Ok(())
}

pub async fn initialize_relayer<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    relayer_id: String,
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
    let relayer_service = get_network_relayer(relayer_id.clone(), &app_state).await?;

    initialize_relayer_with_service(&relayer_id, &relayer_service).await
}

/// Collects relayer IDs that need initialization
pub fn get_relayer_ids_to_initialize(relayers: &[RelayerRepoModel]) -> Vec<String> {
    relayers.iter().map(|r| r.id.clone()).collect()
}

pub async fn initialize_relayers<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
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
    let relayers = app_state.relayer_repository.list_all().await?;

    // Early return for empty list - no work to do
    if relayers.is_empty() {
        info!("No relayers to initialize");
        return Ok(());
    }

    info!(count = relayers.len(), "Initializing relayers");

    // Check if using persistent storage with distributed coordination
    let use_lock = ServerConfig::get_distributed_mode();
    let connection_info = app_state.relayer_repository.connection_info();

    match (use_lock, connection_info) {
        (true, Some((conn, prefix))) => {
            // Distributed mode: use locking to coordinate across instances
            coordinate_with_distributed_lock(&relayers, &app_state, &conn, &prefix).await
        }
        _ => {
            // Single-instance mode or in-memory storage: skip locking
            info!("Initializing relayers without distributed locking");
            run_initialization_batch(&relayers, &app_state).await
        }
    }
}

/// Coordinates relayer initialization with a distributed lock for multi-instance deployments.
///
/// This function handles the coordination logic for distributed initialization:
/// 1. Check if initialization was recently completed (skip if yes)
/// 2. Try to acquire global lock
/// 3. If lock acquired: initialize all relayers and record completion time
/// 4. If lock held by another instance: wait for completion
/// 5. If wait times out: recheck state and attempt recovery (lock holder may have crashed)
/// 6. If lock error: proceed without coordination (graceful degradation)
async fn coordinate_with_distributed_lock<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    relayers: &[RelayerRepoModel],
    app_state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    conn: &Arc<Pool>,
    prefix: &str,
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
    // Step 1: Check if recently completed
    match is_global_init_recently_completed(conn, prefix, INIT_STALENESS_THRESHOLD_SECS).await {
        Ok(true) => {
            info!("Initialization recently completed by another instance, skipping");
            return Ok(());
        }
        Ok(false) => {}
        Err(e) => {
            warn!(
                error = %e,
                "Failed to check recent initialization status, proceeding with initialization"
            );
        }
    }

    // Step 2: Try to acquire global lock
    let lock_key = format!("{prefix}:lock:{GLOBAL_INIT_LOCK_NAME}");
    let lock = DistributedLock::new(
        conn.clone(),
        &lock_key,
        Duration::from_secs(BOOTSTRAP_LOCK_TTL_SECS),
    );

    let lock_result = lock.try_acquire().await;

    // Handle lock held by another instance
    if matches!(&lock_result, Ok(None)) {
        info!("Another instance is initializing relayers, waiting for completion");
        let completed = wait_for_initialization_complete(conn, prefix).await?;

        if completed {
            return Ok(());
        }

        // Timeout reached — the lock holder may have crashed without completing.
        // Recheck: was initialization completed in the final moments?
        warn!("Timeout waiting for initialization, rechecking state");

        if is_global_init_recently_completed(conn, prefix, INIT_STALENESS_THRESHOLD_SECS)
            .await
            .unwrap_or(false)
        {
            info!("Initialization completed during timeout window");
            return Ok(());
        }

        // Not completed. Try to acquire the lock and take over initialization.
        return recover_after_timeout(relayers, app_state, conn, prefix).await;
    }

    // Handle lock error - graceful degradation (early return)
    let guard = match lock_result {
        Ok(Some(g)) => g,
        Err(e) => {
            warn!(
                error = %e,
                "Failed to acquire distributed lock, proceeding without coordination"
            );
            return run_initialization_batch(relayers, app_state).await;
        }
        Ok(None) => unreachable!(), // Already handled above
    };

    // Lock acquired - proceed with initialization
    info!(
        count = relayers.len(),
        "Acquired initialization lock, initializing relayers"
    );

    let result = run_initialization_batch(relayers, app_state).await;

    // Record completion time only on success
    if result.is_ok() {
        if let Err(e) = set_global_init_completed(conn, prefix).await {
            warn!(error = %e, "Failed to record initialization completion time");
        }
    }

    drop(guard);
    result
}

/// Waits for another instance to complete initialization.
///
/// Polls periodically until:
/// - Initialization is completed (detected via recent completion timestamp) → returns `Ok(true)`
/// - Timeout is reached without completion detected → returns `Ok(false)`
async fn wait_for_initialization_complete(conn: &Arc<Pool>, prefix: &str) -> Result<bool> {
    let max_wait = Duration::from_secs(LOCK_WAIT_MAX_SECS);
    let poll_interval = Duration::from_millis(LOCK_POLL_INTERVAL_MS);

    // Clone values for the closure
    let conn = conn.clone();
    let prefix = prefix.to_string();

    poll_until(
        || is_global_init_recently_completed(&conn, &prefix, INIT_STALENESS_THRESHOLD_SECS),
        max_wait,
        poll_interval,
        "initialization",
    )
    .await
}

/// Attempts to recover after a wait timeout by acquiring the lock and initializing.
///
/// This handles the case where the lock holder crashed or errored without completing
/// initialization. After timeout:
/// - If the lock can be acquired (previous holder's TTL expired): take over and initialize
/// - If the lock is still held (another instance is legitimately running): wait one more
///   bounded period for completion, then initialize as last resort
/// - If Redis errors: graceful degradation (initialize without coordination)
async fn recover_after_timeout<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    relayers: &[RelayerRepoModel],
    app_state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    conn: &Arc<Pool>,
    prefix: &str,
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
    let lock_key = format!("{prefix}:lock:{GLOBAL_INIT_LOCK_NAME}");
    let recovery_lock = DistributedLock::new(
        conn.clone(),
        &lock_key,
        Duration::from_secs(BOOTSTRAP_LOCK_TTL_SECS),
    );

    match recovery_lock.try_acquire().await {
        Ok(Some(guard)) => {
            // Lock expired (holder crashed) — we take over
            warn!(
                count = relayers.len(),
                "Previous lock holder appears to have crashed, taking over initialization"
            );
            let result = run_initialization_batch(relayers, app_state).await;
            if result.is_ok() {
                if let Err(e) = set_global_init_completed(conn, prefix).await {
                    warn!(error = %e, "Failed to record initialization completion time");
                }
            }
            drop(guard);
            result
        }
        Ok(None) => {
            // Lock still held — another instance is legitimately running.
            // Wait one more bounded period for completion instead of failing.
            warn!("Lock still held by another instance after timeout, waiting for completion");
            let completed = wait_for_initialization_complete(conn, prefix).await?;

            if completed {
                info!("Initialization completed by another instance during extended wait");
                Ok(())
            } else {
                // Extended wait also timed out (~260s total). Proceed with initialization
                // as a last resort rather than failing — duplicate side effects (notifications,
                // jobs) are a minor cost compared to no instance initializing at all.
                warn!("Extended wait also timed out, proceeding with initialization");
                let result = run_initialization_batch(relayers, app_state).await;
                if result.is_ok() {
                    if let Err(e) = set_global_init_completed(conn, prefix).await {
                        warn!(error = %e, "Failed to record initialization completion time");
                    }
                }
                result
            }
        }
        Err(e) => {
            // Redis error — graceful degradation
            warn!(
                error = %e,
                "Failed to check lock after timeout, attempting initialization without coordination"
            );
            run_initialization_batch(relayers, app_state).await
        }
    }
}

/// Runs the batch initialization of all relayers concurrently.
async fn run_initialization_batch<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    relayers: &[RelayerRepoModel],
    app_state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
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
    let futures = relayers.iter().map(|relayer| {
        let app_state = app_state.clone();
        let relayer_id = relayer.id.clone();

        async move {
            let result = initialize_relayer(relayer_id.clone(), app_state).await;
            (relayer_id, result)
        }
    });

    let results = futures::future::join_all(futures).await;

    // Count and report results
    let succeeded = results.iter().filter(|(_, r)| r.is_ok()).count();
    let failed = results.iter().filter(|(_, r)| r.is_err()).count();

    info!(
        succeeded = succeeded,
        failed = failed,
        "Relayer initialization completed"
    );

    // Collect failures and return error if any
    if failed > 0 {
        let failures: Vec<String> = results
            .into_iter()
            .filter_map(|(id, r)| r.err().map(|e| format!("{id}: {e}")))
            .collect();

        return Err(eyre::eyre!(
            "Failed to initialize {} relayer(s): {}",
            failed,
            failures.join("; ")
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::mocks::mockutils::create_mock_relayer;

    #[test]
    fn test_get_relayer_ids_with_empty_list() {
        let relayers: Vec<RelayerRepoModel> = vec![];
        let ids = get_relayer_ids_to_initialize(&relayers);

        assert_eq!(ids.len(), 0, "Should return empty list for no relayers");
    }

    #[test]
    fn test_get_relayer_ids_with_single_relayer() {
        let relayers = vec![create_mock_relayer("relayer-1".to_string(), false)];

        let ids = get_relayer_ids_to_initialize(&relayers);

        assert_eq!(ids.len(), 1, "Should return one ID");
        assert_eq!(ids[0], "relayer-1");
    }

    #[test]
    fn test_get_relayer_ids_with_multiple_relayers() {
        let relayers = vec![
            create_mock_relayer("evm-relayer".to_string(), false),
            create_mock_relayer("solana-relayer".to_string(), false),
            create_mock_relayer("stellar-relayer".to_string(), false),
        ];

        let ids = get_relayer_ids_to_initialize(&relayers);

        assert_eq!(ids.len(), 3, "Should return three IDs");
        assert_eq!(ids[0], "evm-relayer");
        assert_eq!(ids[1], "solana-relayer");
        assert_eq!(ids[2], "stellar-relayer");
    }

    #[test]
    fn test_get_relayer_ids_with_mixed_states() {
        let mut relayers = vec![
            create_mock_relayer("active-relayer".to_string(), false),
            create_mock_relayer("paused-relayer".to_string(), false),
            create_mock_relayer("disabled-relayer".to_string(), false),
        ];

        // Modify states
        relayers[1].paused = true;
        relayers[2].system_disabled = true;

        let ids = get_relayer_ids_to_initialize(&relayers);

        // Should include ALL relayers regardless of state (initialization handles state)
        assert_eq!(
            ids.len(),
            3,
            "Should include all relayers regardless of state"
        );
        assert!(ids.contains(&"active-relayer".to_string()));
        assert!(ids.contains(&"paused-relayer".to_string()));
        assert!(ids.contains(&"disabled-relayer".to_string()));
    }

    #[test]
    fn test_get_relayer_ids_with_different_network_types() {
        let relayers = vec![
            create_mock_relayer("evm-1".to_string(), false),
            create_mock_relayer("evm-2".to_string(), false),
            create_mock_relayer("solana-1".to_string(), false),
            create_mock_relayer("stellar-1".to_string(), false),
        ];

        let ids = get_relayer_ids_to_initialize(&relayers);

        assert_eq!(ids.len(), 4, "Should include all network types");

        // Verify all network types are included
        assert!(ids.iter().any(|id| id.starts_with("evm-")));
        assert!(ids.iter().any(|id| id.starts_with("solana-")));
        assert!(ids.iter().any(|id| id.starts_with("stellar-")));
    }

    #[test]
    fn test_concurrent_initialization_count() {
        // This test verifies the number of concurrent initializations
        // that would be triggered for different relayer counts

        let test_cases = vec![
            (0, 0),   // No relayers = no initializations
            (1, 1),   // One relayer = one initialization
            (5, 5),   // Five relayers = five concurrent initializations
            (10, 10), // Ten relayers = ten concurrent initializations
        ];

        for (relayer_count, expected_init_count) in test_cases {
            let relayers: Vec<RelayerRepoModel> = (0..relayer_count)
                .map(|i| create_mock_relayer(format!("relayer-{i}"), false))
                .collect();

            let ids = get_relayer_ids_to_initialize(&relayers);

            assert_eq!(
                ids.len(),
                expected_init_count,
                "Should create {expected_init_count} initializations for {relayer_count} relayers"
            );
        }
    }

    #[tokio::test]
    async fn test_initialize_relayer_with_service_success() {
        use crate::domain::MockRelayer;

        let mut mock_relayer = MockRelayer::new();
        mock_relayer
            .expect_initialize_relayer()
            .times(1)
            .returning(|| Box::pin(async { Ok(()) }));

        let result = initialize_relayer_with_service("test-relayer", &mock_relayer).await;

        assert!(result.is_ok(), "Should successfully initialize relayer");
    }

    #[tokio::test]
    async fn test_initialize_relayer_with_service_failure() {
        use crate::domain::MockRelayer;
        use crate::models::RelayerError;

        let mut mock_relayer = MockRelayer::new();
        mock_relayer
            .expect_initialize_relayer()
            .times(1)
            .returning(|| {
                Box::pin(async {
                    Err(RelayerError::ProviderError(
                        "RPC connection failed".to_string(),
                    ))
                })
            });

        let result = initialize_relayer_with_service("test-relayer", &mock_relayer).await;

        assert!(
            result.is_err(),
            "Should fail when initialize_relayer returns error"
        );
        let err = result.unwrap_err();
        assert!(err
            .to_string()
            .contains("Failed to initialize relayer: test-relayer"));
    }

    #[tokio::test]
    async fn test_initialize_relayer_with_service_called_once() {
        use crate::domain::MockRelayer;

        let mut mock_relayer = MockRelayer::new();
        // Verify that initialize_relayer is called exactly once
        mock_relayer
            .expect_initialize_relayer()
            .times(1)
            .returning(|| Box::pin(async { Ok(()) }));

        let _ = initialize_relayer_with_service("relayer-123", &mock_relayer).await;

        // Mock will panic if expectations aren't met (called more/less than once)
    }

    #[tokio::test]
    async fn test_initialize_relayer_with_service_multiple_relayers() {
        use crate::domain::MockRelayer;

        // Test that we can call initialize_relayer_with_service multiple times
        let mut mock_relayer_1 = MockRelayer::new();
        mock_relayer_1
            .expect_initialize_relayer()
            .times(1)
            .returning(|| Box::pin(async { Ok(()) }));

        let mut mock_relayer_2 = MockRelayer::new();
        mock_relayer_2
            .expect_initialize_relayer()
            .times(1)
            .returning(|| Box::pin(async { Ok(()) }));

        let result1 = initialize_relayer_with_service("relayer-1", &mock_relayer_1).await;
        let result2 = initialize_relayer_with_service("relayer-2", &mock_relayer_2).await;

        assert!(
            result1.is_ok(),
            "First relayer should initialize successfully"
        );
        assert!(
            result2.is_ok(),
            "Second relayer should initialize successfully"
        );
    }

    // Tests for constants
    #[test]
    fn test_lock_ttl_is_reasonable() {
        // Lock TTL should be at least 60 seconds to handle slow initializations
        assert!(
            BOOTSTRAP_LOCK_TTL_SECS >= 60,
            "Lock TTL should be at least 60 seconds"
        );
        // But not too long (more than 10 minutes would be excessive)
        assert!(
            BOOTSTRAP_LOCK_TTL_SECS <= 600,
            "Lock TTL should not exceed 10 minutes"
        );
    }

    #[test]
    fn test_staleness_threshold_is_reasonable() {
        // Staleness threshold should be at least 60 seconds
        assert!(
            INIT_STALENESS_THRESHOLD_SECS >= 60,
            "Staleness threshold should be at least 60 seconds"
        );
        // But not too long (more than 1 hour would be excessive)
        assert!(
            INIT_STALENESS_THRESHOLD_SECS <= 3600,
            "Staleness threshold should not exceed 1 hour"
        );
    }

    #[test]
    fn test_wait_max_duration_exceeds_lock_ttl() {
        // Wait duration should be longer than lock TTL to handle edge cases
        assert!(
            LOCK_WAIT_MAX_SECS > BOOTSTRAP_LOCK_TTL_SECS,
            "Wait duration should exceed lock TTL"
        );
    }

    #[test]
    fn test_poll_interval_is_reasonable() {
        // Poll interval should be at least 100ms to avoid excessive polling
        assert!(
            LOCK_POLL_INTERVAL_MS >= 100,
            "Poll interval should be at least 100ms"
        );
        // But not too long (more than 5 seconds would be slow)
        assert!(
            LOCK_POLL_INTERVAL_MS <= 5000,
            "Poll interval should not exceed 5 seconds"
        );
    }

    // Tests for get_relayer_ids_to_initialize edge cases
    #[test]
    fn test_get_relayer_ids_preserves_order() {
        let relayers = vec![
            create_mock_relayer("z-relayer".to_string(), false),
            create_mock_relayer("a-relayer".to_string(), false),
            create_mock_relayer("m-relayer".to_string(), false),
        ];

        let ids = get_relayer_ids_to_initialize(&relayers);

        // Should preserve insertion order, not sort
        assert_eq!(ids[0], "z-relayer");
        assert_eq!(ids[1], "a-relayer");
        assert_eq!(ids[2], "m-relayer");
    }

    #[test]
    fn test_get_relayer_ids_with_special_characters() {
        let relayers = vec![
            create_mock_relayer("relayer-with-dashes".to_string(), false),
            create_mock_relayer("relayer_with_underscores".to_string(), false),
            create_mock_relayer("relayer.with.dots".to_string(), false),
        ];

        let ids = get_relayer_ids_to_initialize(&relayers);

        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&"relayer-with-dashes".to_string()));
        assert!(ids.contains(&"relayer_with_underscores".to_string()));
        assert!(ids.contains(&"relayer.with.dots".to_string()));
    }

    #[test]
    fn test_get_relayer_ids_with_large_list() {
        let relayers: Vec<RelayerRepoModel> = (0..100)
            .map(|i| create_mock_relayer(format!("relayer-{:03}", i), false))
            .collect();

        let ids = get_relayer_ids_to_initialize(&relayers);

        assert_eq!(ids.len(), 100);
        assert_eq!(ids[0], "relayer-000");
        assert_eq!(ids[99], "relayer-099");
    }

    // Test error message formatting
    #[tokio::test]
    async fn test_initialize_relayer_with_service_error_includes_relayer_id() {
        use crate::domain::MockRelayer;
        use crate::models::RelayerError;

        let mut mock_relayer = MockRelayer::new();
        mock_relayer
            .expect_initialize_relayer()
            .times(1)
            .returning(|| {
                Box::pin(async {
                    Err(RelayerError::NetworkConfiguration("bad config".to_string()))
                })
            });

        let result = initialize_relayer_with_service("my-special-relayer-id", &mock_relayer).await;

        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("my-special-relayer-id"),
            "Error should contain relayer ID, got: {}",
            err_str
        );
    }

    #[tokio::test]
    async fn test_initialize_relayer_with_service_provider_error() {
        use crate::domain::MockRelayer;
        use crate::models::RelayerError;

        let mut mock_relayer = MockRelayer::new();
        mock_relayer
            .expect_initialize_relayer()
            .times(1)
            .returning(|| {
                Box::pin(async { Err(RelayerError::ProviderError("provider failed".to_string())) })
            });

        let result = initialize_relayer_with_service("test-relayer", &mock_relayer).await;
        assert!(result.is_err(), "Should fail for ProviderError");
    }

    #[tokio::test]
    async fn test_initialize_relayer_with_service_network_config_error() {
        use crate::domain::MockRelayer;
        use crate::models::RelayerError;

        let mut mock_relayer = MockRelayer::new();
        mock_relayer
            .expect_initialize_relayer()
            .times(1)
            .returning(|| {
                Box::pin(async {
                    Err(RelayerError::NetworkConfiguration(
                        "network config error".to_string(),
                    ))
                })
            });

        let result = initialize_relayer_with_service("test-relayer", &mock_relayer).await;
        assert!(
            result.is_err(),
            "Should fail for NetworkConfiguration error"
        );
    }

    // ============================================================================
    // Integration tests for run_initialization_batch, coordinate_with_distributed_lock,
    // wait_for_initialization_complete, and initialize_relayers
    // ============================================================================

    use crate::utils::mocks::mockutils::create_mock_app_state;
    use actix_web::web::ThinData;

    /// Helper to create a Redis connection pool for integration tests.
    async fn create_test_redis_pool() -> Option<Arc<Pool>> {
        let cfg = deadpool_redis::Config::from_url("redis://127.0.0.1:6379");
        let pool = cfg
            .builder()
            .ok()?
            .max_size(16)
            .runtime(deadpool_redis::Runtime::Tokio1)
            .build()
            .ok()?;
        Some(Arc::new(pool))
    }

    // --- Tests for run_initialization_batch ---

    #[tokio::test]
    async fn test_run_initialization_batch_empty_list() {
        let app_state = create_mock_app_state(None, None, None, None, None, None).await;
        let thin_state = ThinData(app_state);

        let relayers: Vec<RelayerRepoModel> = vec![];
        let result = run_initialization_batch(&relayers, &thin_state).await;

        assert!(
            result.is_ok(),
            "Should succeed with empty relayer list: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_run_initialization_batch_handles_failures() {
        let relayers = vec![create_mock_relayer("failing-relayer".to_string(), false)];

        let app_state =
            create_mock_app_state(None, Some(relayers.clone()), None, None, None, None).await;
        let thin_state = ThinData(app_state);

        let result = run_initialization_batch(&relayers, &thin_state).await;

        assert!(result.is_err(), "Should fail due to missing signer");
    }

    #[tokio::test]
    async fn test_run_initialization_batch_concurrent_execution() {
        let relayers: Vec<RelayerRepoModel> = (0..5)
            .map(|i| create_mock_relayer(format!("concurrent-relayer-{}", i), false))
            .collect();

        let app_state =
            create_mock_app_state(None, Some(relayers.clone()), None, None, None, None).await;
        let thin_state = ThinData(app_state);

        // This will fail because signers aren't configured, but it tests concurrent execution
        let result = run_initialization_batch(&relayers, &thin_state).await;

        assert!(result.is_err(), "Should fail due to missing signers");
        // The error message should mention the failed relayers
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("Failed to initialize"),
            "Error should mention initialization failure"
        );
    }

    // --- Tests for coordinate_with_distributed_lock (requires Redis) ---

    #[tokio::test]
    #[ignore] // Requires running Redis instance
    async fn test_coordinate_with_distributed_lock_skips_when_recently_completed() {
        let conn = create_test_redis_pool()
            .await
            .expect("Redis connection required");

        let relayers = vec![create_mock_relayer(
            "global-lock-relayer".to_string(),
            false,
        )];
        let app_state =
            create_mock_app_state(None, Some(relayers.clone()), None, None, None, None).await;
        let thin_state = ThinData(app_state);

        let prefix = "test_global_skip_recent";

        // Set completion time to simulate recent initialization
        set_global_init_completed(&conn, prefix)
            .await
            .expect("Should set completion time");

        let result = coordinate_with_distributed_lock(&relayers, &thin_state, &conn, prefix).await;

        // Should succeed because it skips (recently completed)
        assert!(
            result.is_ok(),
            "Should skip initialization when recently completed: {:?}",
            result
        );

        // Cleanup
        let mut conn_clone = conn.get().await.expect("Failed to get connection");
        let hash_key = format!("{}:relayer_sync_meta", prefix);
        let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &hash_key).await;
    }

    #[tokio::test]
    #[ignore] // Requires running Redis instance
    async fn test_coordinate_with_distributed_lock_acquires_lock() {
        let conn = create_test_redis_pool()
            .await
            .expect("Redis connection required");

        let relayers = vec![create_mock_relayer(
            "lock-acquire-relayer".to_string(),
            false,
        )];
        let app_state =
            create_mock_app_state(None, Some(relayers.clone()), None, None, None, None).await;
        let thin_state = ThinData(app_state);

        let prefix = "test_global_acquire_lock";

        // Clear any existing state
        {
            let mut conn_clone = conn.get().await.expect("Failed to get connection");
            let hash_key = format!("{}:relayer_sync_meta", prefix);
            let lock_key = format!("{}:lock:{}", prefix, GLOBAL_INIT_LOCK_NAME);
            let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &hash_key).await;
            let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &lock_key).await;
        }

        let result = coordinate_with_distributed_lock(&relayers, &thin_state, &conn, prefix).await;

        // Will fail because signer isn't configured, but lock should have been acquired
        assert!(
            result.is_err(),
            "Should fail due to missing signer configuration"
        );

        // Verify completion time was NOT set (because initialization failed)
        let is_recent = is_global_init_recently_completed(&conn, prefix, 300)
            .await
            .expect("Should check completion");
        assert!(!is_recent, "Should NOT record completion time on failure");

        // Cleanup
        {
            let mut conn_clone = conn.get().await.expect("Failed to get connection");
            let hash_key = format!("{}:relayer_sync_meta", prefix);
            let lock_key = format!("{}:lock:{}", prefix, GLOBAL_INIT_LOCK_NAME);
            let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &hash_key).await;
            let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &lock_key).await;
        }
    }

    #[tokio::test]
    #[ignore] // Requires running Redis instance
    async fn test_coordinate_with_distributed_lock_waits_when_lock_held() {
        let conn = create_test_redis_pool()
            .await
            .expect("Redis connection required");

        let relayers = vec![create_mock_relayer("wait-relayer".to_string(), false)];
        let app_state =
            create_mock_app_state(None, Some(relayers.clone()), None, None, None, None).await;
        let thin_state = ThinData(app_state);

        let prefix = "test_global_wait_lock";
        let hash_key = format!("{}:relayer_sync_meta", prefix);
        let lock_key = format!("{}:lock:{}", prefix, GLOBAL_INIT_LOCK_NAME);

        // Clear any existing state
        {
            let mut conn_clone = conn.get().await.expect("Failed to get connection");
            let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &hash_key).await;
            let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &lock_key).await;
        }

        // Acquire lock to simulate another instance initializing
        let lock = DistributedLock::new(conn.clone(), &lock_key, Duration::from_secs(5));
        let guard = lock
            .try_acquire()
            .await
            .expect("Should acquire lock")
            .expect("Lock should be available");

        // Spawn task to release lock and set completion after a short delay
        let conn_for_task = conn.clone();
        let prefix_for_task = prefix.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            set_global_init_completed(&conn_for_task, &prefix_for_task)
                .await
                .expect("Should set completion");
            guard.release().await.expect("Should release lock");
        });

        // This should wait and then succeed (because completion will be set)
        let result = coordinate_with_distributed_lock(&relayers, &thin_state, &conn, prefix).await;

        assert!(
            result.is_ok(),
            "Should succeed after waiting for completion: {:?}",
            result
        );

        // Cleanup
        {
            let mut conn_clone = conn.get().await.expect("Failed to get connection");
            let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &hash_key).await;
            let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &lock_key).await;
        }
    }

    // --- Tests for initialize_relayers main function ---

    #[tokio::test]
    async fn test_initialize_relayers_empty_list() {
        let app_state = create_mock_app_state(None, None, None, None, None, None).await;
        let thin_state = ThinData(app_state);

        let result = initialize_relayers(thin_state).await;

        assert!(
            result.is_ok(),
            "Should succeed with empty relayer list: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_initialize_relayers_uses_in_memory_path() {
        let relayers = vec![create_mock_relayer("inmem-relayer".to_string(), false)];
        let app_state =
            create_mock_app_state(None, Some(relayers.clone()), None, None, None, None).await;
        let thin_state = ThinData(app_state);

        let result = initialize_relayers(thin_state).await;

        assert!(
            result.is_err(),
            "Should fail due to missing signer configuration"
        );
    }
}
