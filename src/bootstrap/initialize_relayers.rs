//! Relayer initialization
//!
//! This module contains functions for initializing relayers, ensuring they are
//! properly configured and ready for operation.
//!
//! ## Distributed Locking
//!
//! When multiple instances of the relayer service start simultaneously, this module
//! uses distributed locking to coordinate initialization and prevent duplicate work:
//!
//! - **Per-relayer locks**: Each relayer is locked independently to allow parallel
//!   initialization of different relayers across instances.
//! - **Recent sync check**: Skips initialization for relayers that were recently
//!   synced (within the staleness threshold) to handle rolling restarts efficiently.
//! - **In-memory fallback**: When using in-memory storage, locking is skipped since
//!   coordination across instances is not possible or needed.
use crate::{
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
    utils::{is_relayer_recently_synced, set_relayer_last_sync, DistributedLock},
};
use color_eyre::{eyre::WrapErr, Result};
use redis::aio::ConnectionManager;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

/// TTL for the per-relayer initialization lock in seconds.
/// Set to 2 minutes (2x worst case init time) as a safety net for crashes.
const RELAYER_INIT_LOCK_TTL_SECS: u64 = 120;

/// Staleness threshold in seconds. Relayers synced within this time are skipped.
/// Set to 5 minutes to prevent redundant initialization on rolling restarts.
const RELAYER_SYNC_STALENESS_THRESHOLD_SECS: u64 = 300;

/// Lock name prefix for relayer initialization locks.
const RELAYER_INIT_LOCK_PREFIX: &str = "relayer_init";

/// Result of attempting to initialize a single relayer.
#[derive(Debug)]
enum RelayerInitResult {
    /// Successfully initialized the relayer.
    Initialized,
    /// Skipped because another instance recently synced this relayer.
    SkippedRecentSync,
    /// Skipped because another instance currently holds the lock.
    SkippedLockHeld,
    /// Initialization failed with an error.
    Failed(String),
}

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
        debug!("No relayers to initialize");
        return Ok(());
    }

    debug!(count = relayers.len(), "Initializing relayers concurrently");

    // Check if using persistent storage for distributed coordination
    let connection_info = app_state.relayer_repository.connection_info();

    // Initialize relayers with appropriate locking strategy
    let results = if let Some((conn, prefix)) = connection_info {
        // Persistent storage mode: use distributed locking
        initialize_relayers_with_locking(&relayers, &app_state, &conn, &prefix).await
    } else {
        // In-memory mode: skip locking, initialize all relayers directly
        debug!("In-memory storage detected, skipping distributed locking");
        initialize_relayers_without_locking(&relayers, &app_state).await
    };

    // Log summary
    let (initialized, skipped_recent, skipped_lock, failed) = count_results(&results);
    info!(
        initialized = initialized,
        skipped_recent_sync = skipped_recent,
        skipped_lock_held = skipped_lock,
        failed = failed,
        "Relayer initialization completed"
    );

    // Fail if any initialization failed
    if failed > 0 {
        let failure_messages: Vec<String> = results
            .into_iter()
            .filter_map(|(id, result)| {
                if let RelayerInitResult::Failed(msg) = result {
                    Some(format!("{id}: {msg}"))
                } else {
                    None
                }
            })
            .collect();

        return Err(eyre::eyre!(
            "Failed to initialize {} relayer(s): {}",
            failed,
            failure_messages.join("; ")
        ));
    }

    Ok(())
}

/// Initializes relayers with distributed locking for coordination across instances.
///
/// For each relayer:
/// 1. Checks if recently synced (skip if yes)
/// 2. Attempts to acquire a per-relayer lock
/// 3. If lock acquired: initializes and records sync time
/// 4. If lock held: skips (another instance is handling it)
async fn initialize_relayers_with_locking<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    relayers: &[RelayerRepoModel],
    app_state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    conn: &Arc<ConnectionManager>,
    prefix: &str,
) -> Vec<(String, RelayerInitResult)>
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
        let conn = conn.clone();
        let prefix = prefix.to_string();
        let relayer_id = relayer.id.clone();

        async move {
            let result =
                initialize_single_relayer_with_lock(&relayer_id, &app_state, &conn, &prefix).await;
            (relayer_id, result)
        }
    });

    futures::future::join_all(futures).await
}

/// Initializes a single relayer with distributed locking.
async fn initialize_single_relayer_with_lock<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    relayer_id: &str,
    app_state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    conn: &Arc<ConnectionManager>,
    prefix: &str,
) -> RelayerInitResult
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
    // Step 1: Check if recently synced
    match is_relayer_recently_synced(
        conn,
        prefix,
        relayer_id,
        RELAYER_SYNC_STALENESS_THRESHOLD_SECS,
    )
    .await
    {
        Ok(true) => {
            debug!(relayer_id = %relayer_id, "skipping initialization - recently synced");
            return RelayerInitResult::SkippedRecentSync;
        }
        Ok(false) => {}
        Err(e) => {
            // Log warning but proceed with initialization (graceful degradation)
            warn!(
                relayer_id = %relayer_id,
                error = %e,
                "failed to check recent sync status, proceeding with initialization"
            );
        }
    }

    // Step 2: Attempt to acquire per-relayer lock
    let lock_key = format!("{prefix}:lock:{RELAYER_INIT_LOCK_PREFIX}:{relayer_id}");
    let lock = DistributedLock::new(
        conn.clone(),
        &lock_key,
        Duration::from_secs(RELAYER_INIT_LOCK_TTL_SECS),
    );

    let lock_guard = match lock.try_acquire().await {
        Ok(Some(guard)) => {
            debug!(relayer_id = %relayer_id, lock_key = %lock_key, "acquired initialization lock");
            Some(guard)
        }
        Ok(None) => {
            debug!(
                relayer_id = %relayer_id,
                lock_key = %lock_key,
                "initialization skipped - lock held by another instance"
            );
            return RelayerInitResult::SkippedLockHeld;
        }
        Err(e) => {
            // Log warning but proceed without lock (graceful degradation)
            warn!(
                relayer_id = %relayer_id,
                error = %e,
                lock_key = %lock_key,
                "failed to acquire lock, proceeding with initialization anyway"
            );
            None
        }
    };

    // Step 3: Initialize the relayer
    let init_result = initialize_relayer(relayer_id.to_string(), app_state.clone()).await;

    // Step 4: Record sync time on success
    match &init_result {
        Ok(()) => {
            if let Err(e) = set_relayer_last_sync(conn, prefix, relayer_id).await {
                warn!(
                    relayer_id = %relayer_id,
                    error = %e,
                    "failed to record last sync time"
                );
            }
        }
        Err(e) => {
            debug!(relayer_id = %relayer_id, error = %e, "initialization failed");
        }
    }

    // Lock guard is automatically released when dropped
    drop(lock_guard);

    match init_result {
        Ok(()) => RelayerInitResult::Initialized,
        Err(e) => RelayerInitResult::Failed(e.to_string()),
    }
}

/// Initializes relayers without distributed locking (for in-memory storage).
async fn initialize_relayers_without_locking<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    relayers: &[RelayerRepoModel],
    app_state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
) -> Vec<(String, RelayerInitResult)>
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
            let result = match initialize_relayer(relayer_id.clone(), app_state).await {
                Ok(()) => RelayerInitResult::Initialized,
                Err(e) => RelayerInitResult::Failed(e.to_string()),
            };
            (relayer_id, result)
        }
    });

    futures::future::join_all(futures).await
}

/// Counts the results of initialization attempts.
fn count_results(results: &[(String, RelayerInitResult)]) -> (usize, usize, usize, usize) {
    let mut initialized = 0;
    let mut skipped_recent = 0;
    let mut skipped_lock = 0;
    let mut failed = 0;

    for (_, result) in results {
        match result {
            RelayerInitResult::Initialized => initialized += 1,
            RelayerInitResult::SkippedRecentSync => skipped_recent += 1,
            RelayerInitResult::SkippedLockHeld => skipped_lock += 1,
            RelayerInitResult::Failed(_) => failed += 1,
        }
    }

    (initialized, skipped_recent, skipped_lock, failed)
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
                .map(|i| create_mock_relayer(format!("relayer-{}", i), false))
                .collect();

            let ids = get_relayer_ids_to_initialize(&relayers);

            assert_eq!(
                ids.len(),
                expected_init_count,
                "Should create {} initializations for {} relayers",
                expected_init_count,
                relayer_count
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

    // Tests for count_results function
    #[test]
    fn test_count_results_empty() {
        let results: Vec<(String, RelayerInitResult)> = vec![];
        let (initialized, skipped_recent, skipped_lock, failed) = count_results(&results);

        assert_eq!(initialized, 0);
        assert_eq!(skipped_recent, 0);
        assert_eq!(skipped_lock, 0);
        assert_eq!(failed, 0);
    }

    #[test]
    fn test_count_results_all_initialized() {
        let results = vec![
            ("relayer-1".to_string(), RelayerInitResult::Initialized),
            ("relayer-2".to_string(), RelayerInitResult::Initialized),
            ("relayer-3".to_string(), RelayerInitResult::Initialized),
        ];
        let (initialized, skipped_recent, skipped_lock, failed) = count_results(&results);

        assert_eq!(initialized, 3);
        assert_eq!(skipped_recent, 0);
        assert_eq!(skipped_lock, 0);
        assert_eq!(failed, 0);
    }

    #[test]
    fn test_count_results_all_skipped_recent_sync() {
        let results = vec![
            (
                "relayer-1".to_string(),
                RelayerInitResult::SkippedRecentSync,
            ),
            (
                "relayer-2".to_string(),
                RelayerInitResult::SkippedRecentSync,
            ),
        ];
        let (initialized, skipped_recent, skipped_lock, failed) = count_results(&results);

        assert_eq!(initialized, 0);
        assert_eq!(skipped_recent, 2);
        assert_eq!(skipped_lock, 0);
        assert_eq!(failed, 0);
    }

    #[test]
    fn test_count_results_all_skipped_lock_held() {
        let results = vec![
            ("relayer-1".to_string(), RelayerInitResult::SkippedLockHeld),
            ("relayer-2".to_string(), RelayerInitResult::SkippedLockHeld),
        ];
        let (initialized, skipped_recent, skipped_lock, failed) = count_results(&results);

        assert_eq!(initialized, 0);
        assert_eq!(skipped_recent, 0);
        assert_eq!(skipped_lock, 2);
        assert_eq!(failed, 0);
    }

    #[test]
    fn test_count_results_all_failed() {
        let results = vec![
            (
                "relayer-1".to_string(),
                RelayerInitResult::Failed("error 1".to_string()),
            ),
            (
                "relayer-2".to_string(),
                RelayerInitResult::Failed("error 2".to_string()),
            ),
        ];
        let (initialized, skipped_recent, skipped_lock, failed) = count_results(&results);

        assert_eq!(initialized, 0);
        assert_eq!(skipped_recent, 0);
        assert_eq!(skipped_lock, 0);
        assert_eq!(failed, 2);
    }

    #[test]
    fn test_count_results_mixed() {
        let results = vec![
            ("relayer-1".to_string(), RelayerInitResult::Initialized),
            (
                "relayer-2".to_string(),
                RelayerInitResult::SkippedRecentSync,
            ),
            ("relayer-3".to_string(), RelayerInitResult::SkippedLockHeld),
            (
                "relayer-4".to_string(),
                RelayerInitResult::Failed("connection error".to_string()),
            ),
            ("relayer-5".to_string(), RelayerInitResult::Initialized),
            (
                "relayer-6".to_string(),
                RelayerInitResult::SkippedRecentSync,
            ),
        ];
        let (initialized, skipped_recent, skipped_lock, failed) = count_results(&results);

        assert_eq!(initialized, 2);
        assert_eq!(skipped_recent, 2);
        assert_eq!(skipped_lock, 1);
        assert_eq!(failed, 1);
    }

    // Tests for RelayerInitResult enum
    #[test]
    fn test_relayer_init_result_debug() {
        // Test Debug trait implementation for all variants
        let initialized = RelayerInitResult::Initialized;
        let skipped_recent = RelayerInitResult::SkippedRecentSync;
        let skipped_lock = RelayerInitResult::SkippedLockHeld;
        let failed = RelayerInitResult::Failed("test error".to_string());

        assert_eq!(format!("{:?}", initialized), "Initialized");
        assert_eq!(format!("{:?}", skipped_recent), "SkippedRecentSync");
        assert_eq!(format!("{:?}", skipped_lock), "SkippedLockHeld");
        assert!(format!("{:?}", failed).contains("Failed"));
        assert!(format!("{:?}", failed).contains("test error"));
    }

    #[test]
    fn test_relayer_init_result_failed_preserves_message() {
        let error_msg = "RPC connection timeout after 30 seconds".to_string();
        let result = RelayerInitResult::Failed(error_msg.clone());

        if let RelayerInitResult::Failed(msg) = result {
            assert_eq!(msg, error_msg);
        } else {
            panic!("Expected Failed variant");
        }
    }

    // Tests for constants
    #[test]
    fn test_lock_ttl_is_reasonable() {
        // Lock TTL should be at least 60 seconds to handle slow initializations
        assert!(
            RELAYER_INIT_LOCK_TTL_SECS >= 60,
            "Lock TTL should be at least 60 seconds"
        );
        // But not too long (more than 10 minutes would be excessive)
        assert!(
            RELAYER_INIT_LOCK_TTL_SECS <= 600,
            "Lock TTL should not exceed 10 minutes"
        );
    }

    #[test]
    fn test_staleness_threshold_is_reasonable() {
        // Staleness threshold should be at least 60 seconds
        assert!(
            RELAYER_SYNC_STALENESS_THRESHOLD_SECS >= 60,
            "Staleness threshold should be at least 60 seconds"
        );
        // But not too long (more than 1 hour would be excessive)
        assert!(
            RELAYER_SYNC_STALENESS_THRESHOLD_SECS <= 3600,
            "Staleness threshold should not exceed 1 hour"
        );
    }

    #[test]
    fn test_lock_prefix_is_valid() {
        assert!(!RELAYER_INIT_LOCK_PREFIX.is_empty());
        assert!(!RELAYER_INIT_LOCK_PREFIX.contains(':'));
        assert!(!RELAYER_INIT_LOCK_PREFIX.contains(' '));
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
    // Integration tests for initialize_relayers_without_locking,
    // initialize_relayers_with_locking, initialize_single_relayer_with_lock,
    // and initialize_relayers
    // ============================================================================

    use crate::utils::mocks::mockutils::create_mock_app_state;
    use actix_web::web::ThinData;
    use std::sync::Arc;
    use std::time::Duration;

    /// Helper to create a Redis connection for integration tests.
    async fn create_test_redis_connection() -> Option<Arc<redis::aio::ConnectionManager>> {
        let client = redis::Client::open("redis://127.0.0.1:6379").ok()?;
        let conn = redis::aio::ConnectionManager::new(client).await.ok()?;
        Some(Arc::new(conn))
    }

    // --- Tests for initialize_relayers_without_locking ---

    #[tokio::test]
    async fn test_initialize_relayers_without_locking_empty_list() {
        let app_state = create_mock_app_state(None, None, None, None, None, None).await;
        let thin_state = ThinData(app_state);

        let relayers: Vec<RelayerRepoModel> = vec![];
        let results = initialize_relayers_without_locking(&relayers, &thin_state).await;

        assert!(
            results.is_empty(),
            "Should return empty results for no relayers"
        );
    }

    #[tokio::test]
    async fn test_initialize_relayers_without_locking_returns_correct_count() {
        let relayers = vec![
            create_mock_relayer("relayer-1".to_string(), false),
            create_mock_relayer("relayer-2".to_string(), false),
            create_mock_relayer("relayer-3".to_string(), false),
        ];

        let app_state =
            create_mock_app_state(None, Some(relayers.clone()), None, None, None, None).await;
        let thin_state = ThinData(app_state);

        let results = initialize_relayers_without_locking(&relayers, &thin_state).await;

        assert_eq!(
            results.len(),
            3,
            "Should process all relayers and return 3 results"
        );

        let result_ids: Vec<&str> = results.iter().map(|(id, _)| id.as_str()).collect();
        assert!(result_ids.contains(&"relayer-1"));
        assert!(result_ids.contains(&"relayer-2"));
        assert!(result_ids.contains(&"relayer-3"));
    }

    #[tokio::test]
    async fn test_initialize_relayers_without_locking_handles_failures() {
        let relayers = vec![create_mock_relayer("failing-relayer".to_string(), false)];

        let app_state =
            create_mock_app_state(None, Some(relayers.clone()), None, None, None, None).await;
        let thin_state = ThinData(app_state);

        let results = initialize_relayers_without_locking(&relayers, &thin_state).await;

        assert_eq!(results.len(), 1);
        let (relayer_id, result) = &results[0];
        assert_eq!(relayer_id, "failing-relayer");
        assert!(matches!(result, RelayerInitResult::Failed(_)));
    }

    #[tokio::test]
    async fn test_initialize_relayers_without_locking_concurrent_execution() {
        let relayers: Vec<RelayerRepoModel> = (0..5)
            .map(|i| create_mock_relayer(format!("concurrent-relayer-{}", i), false))
            .collect();

        let app_state =
            create_mock_app_state(None, Some(relayers.clone()), None, None, None, None).await;
        let thin_state = ThinData(app_state);

        let results = initialize_relayers_without_locking(&relayers, &thin_state).await;

        assert_eq!(results.len(), 5);

        let mut ids: Vec<String> = results.iter().map(|(id, _)| id.clone()).collect();
        ids.sort();
        let expected: Vec<String> = (0..5)
            .map(|i| format!("concurrent-relayer-{}", i))
            .collect();
        assert_eq!(ids, expected);
    }

    // --- Tests for initialize_relayers_with_locking (requires Redis) ---

    #[tokio::test]
    #[ignore] // Requires running Redis instance
    async fn test_initialize_relayers_with_locking_empty_list() {
        let conn = create_test_redis_connection()
            .await
            .expect("Redis connection required");

        let app_state = create_mock_app_state(None, None, None, None, None, None).await;
        let thin_state = ThinData(app_state);

        let relayers: Vec<RelayerRepoModel> = vec![];
        let prefix = "test_init_empty";

        let results = initialize_relayers_with_locking(&relayers, &thin_state, &conn, prefix).await;

        assert!(
            results.is_empty(),
            "Should return empty results for no relayers"
        );
    }

    #[tokio::test]
    #[ignore] // Requires running Redis instance
    async fn test_initialize_relayers_with_locking_acquires_locks() {
        let conn = create_test_redis_connection()
            .await
            .expect("Redis connection required");

        let relayers = vec![
            create_mock_relayer("lock-test-relayer-1".to_string(), false),
            create_mock_relayer("lock-test-relayer-2".to_string(), false),
        ];

        let app_state =
            create_mock_app_state(None, Some(relayers.clone()), None, None, None, None).await;
        let thin_state = ThinData(app_state);

        let prefix = "test_init_locks";

        let results = initialize_relayers_with_locking(&relayers, &thin_state, &conn, prefix).await;

        assert_eq!(results.len(), 2);

        for (id, result) in &results {
            assert!(
                matches!(
                    result,
                    RelayerInitResult::Failed(_) | RelayerInitResult::Initialized
                ),
                "Relayer {} should have attempted initialization, got {:?}",
                id,
                result
            );
        }

        // Cleanup
        let mut conn_clone = (*conn).clone();
        let hash_key = format!("{}:relayer_sync_meta", prefix);
        let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &hash_key).await;
    }

    #[tokio::test]
    #[ignore] // Requires running Redis instance
    async fn test_initialize_relayers_with_locking_skips_recently_synced() {
        let conn = create_test_redis_connection()
            .await
            .expect("Redis connection required");

        let relayers = vec![create_mock_relayer(
            "recently-synced-relayer".to_string(),
            false,
        )];

        let app_state =
            create_mock_app_state(None, Some(relayers.clone()), None, None, None, None).await;
        let thin_state = ThinData(app_state);

        let prefix = "test_init_recent_sync";

        set_relayer_last_sync(&conn, prefix, "recently-synced-relayer")
            .await
            .expect("Should set sync time");

        let results = initialize_relayers_with_locking(&relayers, &thin_state, &conn, prefix).await;

        assert_eq!(results.len(), 1);
        let (id, result) = &results[0];
        assert_eq!(id, "recently-synced-relayer");
        assert!(
            matches!(result, RelayerInitResult::SkippedRecentSync),
            "Should skip recently synced relayer, got {:?}",
            result
        );

        // Cleanup
        let mut conn_clone = (*conn).clone();
        let hash_key = format!("{}:relayer_sync_meta", prefix);
        let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &hash_key).await;
    }

    #[tokio::test]
    #[ignore] // Requires running Redis instance
    async fn test_initialize_relayers_with_locking_mixed_results() {
        let conn = create_test_redis_connection()
            .await
            .expect("Redis connection required");

        let relayers = vec![
            create_mock_relayer("mixed-recent-relayer".to_string(), false),
            create_mock_relayer("mixed-init-relayer".to_string(), false),
        ];

        let app_state =
            create_mock_app_state(None, Some(relayers.clone()), None, None, None, None).await;
        let thin_state = ThinData(app_state);

        let prefix = "test_init_mixed";

        set_relayer_last_sync(&conn, prefix, "mixed-recent-relayer")
            .await
            .expect("Should set sync time");

        let results = initialize_relayers_with_locking(&relayers, &thin_state, &conn, prefix).await;

        assert_eq!(results.len(), 2);

        let recent_result = results
            .iter()
            .find(|(id, _)| id == "mixed-recent-relayer")
            .map(|(_, r)| r);
        let init_result = results
            .iter()
            .find(|(id, _)| id == "mixed-init-relayer")
            .map(|(_, r)| r);

        assert!(
            matches!(recent_result, Some(RelayerInitResult::SkippedRecentSync)),
            "First relayer should be skipped as recently synced"
        );
        assert!(
            matches!(
                init_result,
                Some(RelayerInitResult::Failed(_)) | Some(RelayerInitResult::Initialized)
            ),
            "Second relayer should attempt initialization"
        );

        // Cleanup
        let mut conn_clone = (*conn).clone();
        let hash_key = format!("{}:relayer_sync_meta", prefix);
        let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &hash_key).await;
    }

    // --- Tests for initialize_single_relayer_with_lock (requires Redis) ---

    #[tokio::test]
    #[ignore] // Requires running Redis instance
    async fn test_single_relayer_skips_when_recently_synced() {
        let conn = create_test_redis_connection()
            .await
            .expect("Redis connection required");

        let relayers = vec![create_mock_relayer(
            "single-recent-relayer".to_string(),
            false,
        )];
        let app_state =
            create_mock_app_state(None, Some(relayers.clone()), None, None, None, None).await;
        let thin_state = ThinData(app_state);

        let prefix = "test_single_recent";

        set_relayer_last_sync(&conn, prefix, "single-recent-relayer")
            .await
            .expect("Should set sync time");

        let result = initialize_single_relayer_with_lock(
            "single-recent-relayer",
            &thin_state,
            &conn,
            prefix,
        )
        .await;

        assert!(
            matches!(result, RelayerInitResult::SkippedRecentSync),
            "Should skip recently synced relayer, got {:?}",
            result
        );

        // Cleanup
        let mut conn_clone = (*conn).clone();
        let hash_key = format!("{}:relayer_sync_meta", prefix);
        let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &hash_key).await;
    }

    #[tokio::test]
    #[ignore] // Requires running Redis instance
    async fn test_single_relayer_skips_when_lock_held() {
        let conn = create_test_redis_connection()
            .await
            .expect("Redis connection required");

        let relayers = vec![create_mock_relayer(
            "locked-relayer-skip-test".to_string(),
            false,
        )];
        let app_state =
            create_mock_app_state(None, Some(relayers.clone()), None, None, None, None).await;
        let thin_state = ThinData(app_state);

        let prefix = "test_skip_lock_held_unique";

        // Pre-cleanup
        let lock_key = format!(
            "{}:lock:{}:{}",
            prefix, RELAYER_INIT_LOCK_PREFIX, "locked-relayer-skip-test"
        );
        let mut conn_clone = (*conn).clone();
        let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &lock_key).await;

        // Acquire lock (simulating another instance)
        let lock = DistributedLock::new(conn.clone(), &lock_key, Duration::from_secs(60));
        let guard = lock
            .try_acquire()
            .await
            .expect("Should acquire lock")
            .expect("Lock should be available after cleanup");

        let result = initialize_single_relayer_with_lock(
            "locked-relayer-skip-test",
            &thin_state,
            &conn,
            prefix,
        )
        .await;

        assert!(
            matches!(result, RelayerInitResult::SkippedLockHeld),
            "Should skip when lock is held, got {:?}",
            result
        );

        guard.release().await.expect("Should release lock");
    }

    #[tokio::test]
    #[ignore] // Requires running Redis instance
    async fn test_single_relayer_acquires_lock_and_initializes() {
        let conn = create_test_redis_connection()
            .await
            .expect("Redis connection required");

        let relayers = vec![create_mock_relayer(
            "single-init-relayer".to_string(),
            false,
        )];
        let app_state =
            create_mock_app_state(None, Some(relayers.clone()), None, None, None, None).await;
        let thin_state = ThinData(app_state);

        let prefix = "test_single_init";

        // Pre-cleanup
        let lock_key = format!(
            "{}:lock:{}:{}",
            prefix, RELAYER_INIT_LOCK_PREFIX, "single-init-relayer"
        );
        let mut conn_clone = (*conn).clone();
        let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &lock_key).await;

        let result =
            initialize_single_relayer_with_lock("single-init-relayer", &thin_state, &conn, prefix)
                .await;

        assert!(
            matches!(
                result,
                RelayerInitResult::Failed(_) | RelayerInitResult::Initialized
            ),
            "Should attempt initialization, got {:?}",
            result
        );

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Verify lock was released
        let lock = DistributedLock::new(conn.clone(), &lock_key, Duration::from_secs(60));
        let guard = lock.try_acquire().await.expect("Should not error");
        assert!(
            guard.is_some(),
            "Lock should be available after initialization"
        );

        // Cleanup
        if let Some(g) = guard {
            g.release().await.expect("Cleanup release");
        }
        let hash_key = format!("{}:relayer_sync_meta", prefix);
        let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &hash_key).await;
    }

    #[tokio::test]
    #[ignore] // Requires running Redis instance
    async fn test_single_relayer_does_not_set_sync_time_on_failure() {
        let conn = create_test_redis_connection()
            .await
            .expect("Redis connection required");

        let relayers = vec![create_mock_relayer(
            "no-sync-on-fail-relayer".to_string(),
            false,
        )];
        let app_state =
            create_mock_app_state(None, Some(relayers.clone()), None, None, None, None).await;
        let thin_state = ThinData(app_state);

        let prefix = "test_no_sync_on_fail";

        // Clear any existing sync time
        let mut conn_clone = (*conn).clone();
        let hash_key = format!("{}:relayer_sync_meta", prefix);
        let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &hash_key).await;

        let result = initialize_single_relayer_with_lock(
            "no-sync-on-fail-relayer",
            &thin_state,
            &conn,
            prefix,
        )
        .await;

        assert!(
            matches!(result, RelayerInitResult::Failed(_)),
            "Should fail without signer, got {:?}",
            result
        );

        let is_recent = is_relayer_recently_synced(&conn, prefix, "no-sync-on-fail-relayer", 300)
            .await
            .expect("Should check sync time");
        assert!(
            !is_recent,
            "Should NOT set sync time after failed initialization"
        );

        // Cleanup
        let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &hash_key).await;
    }

    #[tokio::test]
    #[ignore] // Requires running Redis instance
    async fn test_single_relayer_graceful_degradation_on_sync_check_error() {
        let conn = create_test_redis_connection()
            .await
            .expect("Redis connection required");

        let relayers = vec![create_mock_relayer(
            "degradation-relayer".to_string(),
            false,
        )];
        let app_state =
            create_mock_app_state(None, Some(relayers.clone()), None, None, None, None).await;
        let thin_state = ThinData(app_state);

        let prefix = "test_degradation";

        let result =
            initialize_single_relayer_with_lock("degradation-relayer", &thin_state, &conn, prefix)
                .await;

        assert!(
            matches!(
                result,
                RelayerInitResult::Failed(_) | RelayerInitResult::Initialized
            ),
            "Should attempt initialization even if checks have issues, got {:?}",
            result
        );

        // Cleanup
        let mut conn_clone = (*conn).clone();
        let hash_key = format!("{}:relayer_sync_meta", prefix);
        let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &hash_key).await;
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
