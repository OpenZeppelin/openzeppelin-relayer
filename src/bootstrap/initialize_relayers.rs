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
}
