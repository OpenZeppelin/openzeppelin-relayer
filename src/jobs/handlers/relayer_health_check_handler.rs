//! Relayer Health Check Handler
//!
//! This module implements an event-driven job handler that attempts to re-enable
//! disabled relayers by running health checks with exponential backoff.

use crate::{
    constants::WORKER_DEFAULT_MAXIMUM_RETRIES,
    domain::{get_network_relayer, Relayer},
    jobs::{handle_result, Job, JobProducerTrait, RelayerHealthCheck},
    models::{
        produce_relayer_enabled_payload, DefaultAppState, DisabledReason, NetworkRepoModel,
        NotificationRepoModel, RelayerRepoModel, SignerRepoModel, ThinDataAppState,
        TransactionRepoModel,
    },
    repositories::{
        ApiKeyRepositoryTrait, NetworkRepository, PluginRepositoryTrait, RelayerRepository,
        Repository, TransactionCounterTrait, TransactionRepository,
    },
    utils::calculate_scheduled_timestamp,
};
use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Data, *};
use eyre::Result;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Handler for relayer health check jobs.
///
/// This event-driven handler:
/// 1. Checks if the relayer is disabled (exits early if not)
/// 2. Runs stateless health checks on the disabled relayer
/// 3. On success: Re-enables the relayer and sends a recovery notification
/// 4. On failure: Updates the disabled reason (if changed) and schedules retry with exponential backoff
///
/// # Behavior
///
/// - **Early exit**: If relayer is not disabled, exits without running checks
/// - **Recovery notification**: Unlike bootstrap (silent), sends notification when re-enabled
/// - **Reason tracking**: Updates disabled_reason if the failure reason changes between retries
/// - **No redundant updates**: Only updates reason when it actually changes
///
/// # Arguments
///
/// * `job` - The job containing relayer health check data
/// * `app_state` - Application state with repositories and services
/// * `attempt` - Current attempt number for retry logic
///
/// # Returns
///
/// Returns `Ok(())` if the health check completes successfully, or an error otherwise.
pub async fn relayer_health_check_handler(
    job: Job<RelayerHealthCheck>,
    app_state: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
) -> Result<(), Error> {
    relayer_health_check_handler_impl(job, app_state, attempt).await
}

/// Generic implementation of the health check handler
#[allow(clippy::type_complexity)]
async fn relayer_health_check_handler_impl<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    job: Job<RelayerHealthCheck>,
    app_state: Data<ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
    attempt: Attempt,
) -> Result<(), Error>
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
    let result = check_and_reenable_relayer(job.data, &app_state).await;
    handle_result(
        result,
        attempt,
        "relayer_health_check",
        WORKER_DEFAULT_MAXIMUM_RETRIES,
    )
}

async fn check_and_reenable_relayer<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    data: RelayerHealthCheck,
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
    let relayer_id = data.relayer_id.clone();

    debug!(
        relayer_id = %relayer_id,
        retry_count = data.retry_count,
        "Running health check on disabled relayer"
    );

    // Check if relayer is actually disabled
    let relayer = app_state
        .relayer_repository
        .get_by_id(relayer_id.clone())
        .await
        .map_err(|e| eyre::eyre!("Failed to get relayer: {}", e))?;

    if !relayer.system_disabled {
        info!(
            relayer_id = %relayer_id,
            "Relayer is not disabled, skipping health check"
        );
        return Ok(());
    }

    // Get the network relayer instance
    let relayer_service = get_network_relayer(relayer_id.clone(), app_state)
        .await
        .map_err(|e| eyre::eyre!("Failed to get relayer: {}", e))?;

    // Run stateless health check
    match relayer_service.check_health().await {
        Ok(_) => {
            // Health checks passed - re-enable and notify (this is a recovery event!)
            info!(
                relayer_id = %relayer_id,
                retry_count = data.retry_count,
                "Health checks passed, re-enabling relayer"
            );

            // Enable the relayer in the repository
            let enabled_relayer = app_state
                .relayer_repository
                .enable_relayer(relayer_id.clone())
                .await
                .map_err(|e| eyre::eyre!("Failed to enable relayer: {}", e))?;

            // Send recovery notification (unlike bootstrap, this is an event worth notifying about!)
            if let Some(notification_id) = &enabled_relayer.notification_id {
                app_state
                    .job_producer
                    .produce_send_notification_job(
                        produce_relayer_enabled_payload(
                            notification_id,
                            &enabled_relayer,
                            data.retry_count,
                        ),
                        None,
                    )
                    .await
                    .map_err(|e| eyre::eyre!("Failed to send notification: {}", e))?;

                info!(
                    relayer_id = %relayer_id,
                    notification_id = %notification_id,
                    "Sent relayer recovery notification"
                );
            }

            Ok(())
        }
        Err(failures) => {
            // Health checks failed - update reason if changed, then schedule retry
            let reason = DisabledReason::from_health_failures(failures).unwrap_or_else(|| {
                DisabledReason::RpcValidationFailed("Unknown error".to_string())
            });

            warn!(
                relayer_id = %relayer_id,
                retry_count = data.retry_count,
                reason = %reason,
                "Health checks failed, scheduling retry"
            );

            // Update the disabled reason if the type of failure has changed
            // We use same_variant() to avoid updating when only error details differ
            let should_update = match &relayer.disabled_reason {
                Some(old_reason) => !old_reason.same_variant(&reason),
                None => true, // Always update if there's no existing reason
            };

            if should_update {
                debug!(
                    relayer_id = %relayer_id,
                    old_reason = ?relayer.disabled_reason,
                    new_reason = %reason,
                    "Disabled reason variant has changed, updating"
                );

                app_state
                    .relayer_repository
                    .disable_relayer(relayer_id.clone(), reason.clone())
                    .await
                    .map_err(|e| eyre::eyre!("Failed to update disabled reason: {}", e))?;
            } else {
                debug!(
                    relayer_id = %relayer_id,
                    reason = %reason,
                    "Disabled reason variant unchanged, skipping update"
                );
            }

            // Calculate exponential backoff delay
            let delay = calculate_backoff_delay(data.retry_count);

            debug!(
                relayer_id = %relayer_id,
                next_retry = data.retry_count + 1,
                delay_seconds = delay.as_secs(),
                "Scheduling next health check attempt"
            );

            // Schedule retry with exponential backoff
            app_state
                .job_producer
                .produce_relayer_health_check_job(
                    RelayerHealthCheck::with_retry_count(relayer_id, data.retry_count + 1),
                    Some(calculate_scheduled_timestamp(delay.as_secs() as i64)),
                )
                .await
                .map_err(|e| eyre::eyre!("Failed to schedule retry: {}", e))?;

            Ok(())
        }
    }
}

/// Calculates exponential backoff delay based on retry count
///
/// Backoff schedule:
/// - Retry 0: 10 seconds
/// - Retry 1: 20 seconds
/// - Retry 2: 30 seconds
/// - Retry 3: 45 seconds
/// - Retry 4+: 60 seconds (max)
fn calculate_backoff_delay(retry_count: u32) -> Duration {
    let seconds = match retry_count {
        0 => 10,
        1 => 20,
        2 => 30,
        3 => 45,
        _ => 60, // Max 60 seconds
    };
    Duration::from_secs(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        DisabledReason, NetworkType, RelayerEvmPolicy, RelayerNetworkPolicy, RelayerRepoModel,
    };

    #[test]
    fn test_calculate_backoff_delay() {
        assert_eq!(calculate_backoff_delay(0), Duration::from_secs(10)); // 10 sec
        assert_eq!(calculate_backoff_delay(1), Duration::from_secs(20)); // 20 sec
        assert_eq!(calculate_backoff_delay(2), Duration::from_secs(30)); // 30 sec
        assert_eq!(calculate_backoff_delay(3), Duration::from_secs(45)); // 45 sec
        assert_eq!(calculate_backoff_delay(4), Duration::from_secs(60)); // 60 sec (max)
        assert_eq!(calculate_backoff_delay(10), Duration::from_secs(60)); // 60 sec (capped)
        assert_eq!(calculate_backoff_delay(100), Duration::from_secs(60)); // 60 sec (continues indefinitely)
    }

    #[test]
    fn test_relayer_health_check_creation() {
        let health_check = RelayerHealthCheck::new("test-relayer".to_string());
        assert_eq!(health_check.relayer_id, "test-relayer");
        assert_eq!(health_check.retry_count, 0);

        let health_check_with_retry =
            RelayerHealthCheck::with_retry_count("test-relayer".to_string(), 3);
        assert_eq!(health_check_with_retry.relayer_id, "test-relayer");
        assert_eq!(health_check_with_retry.retry_count, 3);
    }

    fn create_disabled_relayer(id: &str) -> RelayerRepoModel {
        RelayerRepoModel {
            id: id.to_string(),
            name: format!("Relayer {}", id),
            network: "sepolia".to_string(),
            paused: false,
            network_type: NetworkType::Evm,
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
                gas_price_cap: None,
                whitelist_receivers: None,
                eip1559_pricing: Some(false),
                private_transactions: Some(false),
                min_balance: Some(0),
                gas_limit_estimation: Some(false),
            }),
            signer_id: "test-signer".to_string(),
            address: "0x742d35Cc6634C0532925a3b8D8C2e48a73F6ba2E".to_string(),
            notification_id: Some("test-notification".to_string()),
            system_disabled: true,
            disabled_reason: Some(DisabledReason::RpcValidationFailed(
                "RPC unavailable".to_string(),
            )),
            custom_rpc_urls: None,
        }
    }

    #[tokio::test]
    async fn test_health_check_data_structure() {
        // Verify health check job structure
        let health_check = RelayerHealthCheck::new("test-relayer".to_string());
        assert_eq!(health_check.relayer_id, "test-relayer");
        assert_eq!(health_check.retry_count, 0);

        // Verify retry count increments correctly
        let health_check_retry =
            RelayerHealthCheck::with_retry_count("test-relayer".to_string(), 5);
        assert_eq!(health_check_retry.retry_count, 5);

        // Verify backoff calculation matches retry count
        let expected_delay = calculate_backoff_delay(5);
        assert_eq!(expected_delay, Duration::from_secs(60)); // Should be capped at 60s
    }

    /// Test that handler exits early if relayer is not disabled
    #[tokio::test]
    async fn test_relayer_health_check_handler_impl_exits_on_enabled() {
        use crate::jobs::MockJobProducerTrait;
        use crate::models::AppState;
        use crate::repositories::{
            ApiKeyRepositoryStorage, NetworkRepositoryStorage, NotificationRepositoryStorage,
            PluginRepositoryStorage, RelayerRepositoryStorage, Repository, SignerRepositoryStorage,
            TransactionCounterRepositoryStorage, TransactionRepositoryStorage,
        };
        use std::sync::Arc;

        // Create a mock job producer (should not be called)
        let mock_job_producer = MockJobProducerTrait::new();

        // Create repositories
        let relayer_repo = Arc::new(RelayerRepositoryStorage::new_in_memory());

        // Create a relayer that is NOT disabled
        let mut relayer = create_disabled_relayer("test-handler-enabled");
        relayer.system_disabled = false;
        relayer.disabled_reason = None;
        relayer_repo.create(relayer).await.unwrap();

        // Create app state
        let app_state = Data::new(actix_web::web::ThinData(AppState {
            relayer_repository: relayer_repo,
            transaction_repository: Arc::new(TransactionRepositoryStorage::new_in_memory()),
            signer_repository: Arc::new(SignerRepositoryStorage::new_in_memory()),
            notification_repository: Arc::new(NotificationRepositoryStorage::new_in_memory()),
            network_repository: Arc::new(NetworkRepositoryStorage::new_in_memory()),
            transaction_counter_store: Arc::new(
                TransactionCounterRepositoryStorage::new_in_memory(),
            ),
            job_producer: Arc::new(mock_job_producer),
            plugin_repository: Arc::new(PluginRepositoryStorage::new_in_memory()),
            api_key_repository: Arc::new(ApiKeyRepositoryStorage::new_in_memory()),
        }));

        // Create job
        let health_check = RelayerHealthCheck::new("test-handler-enabled".to_string());
        let job = Job::new(crate::jobs::JobType::RelayerHealthCheck, health_check);
        let attempt = Attempt::new_with_value(1);

        // Call the handler implementation - should exit early
        let result = relayer_health_check_handler_impl(job, app_state, attempt).await;

        // Should succeed (exits early)
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_relayer_health_check_backoff_progression() {
        // Test that backoff delays progress correctly through retries
        let delays: Vec<Duration> = (0..6).map(calculate_backoff_delay).collect();

        // Verify the progression
        assert_eq!(delays[0], Duration::from_secs(10)); // First retry: 10s
        assert_eq!(delays[1], Duration::from_secs(20)); // Second retry: 20s
        assert_eq!(delays[2], Duration::from_secs(30)); // Third retry: 30s
        assert_eq!(delays[3], Duration::from_secs(45)); // Fourth retry: 45s
        assert_eq!(delays[4], Duration::from_secs(60)); // Fifth retry: 60s (max)
        assert_eq!(delays[5], Duration::from_secs(60)); // Sixth retry: 60s (stays at max)

        // Verify that each delay (except the last) is increasing
        for i in 0..4 {
            assert!(
                delays[i] < delays[i + 1],
                "Delay should increase with retry count"
            );
        }

        // Verify that we cap at 60 seconds
        assert_eq!(delays[4], delays[5], "Delay should cap at 60 seconds");
    }

    #[tokio::test]
    async fn test_disabled_reason_is_preserved() {
        // Verify that disabled reasons are properly stored and retrieved
        use crate::repositories::RelayerRepositoryStorage;
        let repo = RelayerRepositoryStorage::new_in_memory();

        let relayer = create_disabled_relayer("test-relayer-2");
        let disabled_reason = relayer.disabled_reason.clone();

        repo.create(relayer).await.unwrap();

        // Retrieve the relayer and verify the disabled reason
        let retrieved = repo.get_by_id("test-relayer-2".to_string()).await.unwrap();

        assert!(retrieved.system_disabled);
        assert_eq!(retrieved.disabled_reason, disabled_reason);

        // Verify the reason describes the failure
        if let Some(reason) = &retrieved.disabled_reason {
            let description = reason.description();
            assert!(description.contains("RPC"));
        }
    }

    #[tokio::test]
    async fn test_check_and_reenable_relayer_exits_early_if_not_disabled() {
        use crate::jobs::MockJobProducerTrait;
        use crate::models::AppState;
        use crate::repositories::{
            ApiKeyRepositoryStorage, NetworkRepositoryStorage, NotificationRepositoryStorage,
            PluginRepositoryStorage, RelayerRepositoryStorage, Repository, SignerRepositoryStorage,
            TransactionCounterRepositoryStorage, TransactionRepositoryStorage,
        };
        use std::sync::Arc;

        // Create repositories
        let relayer_repo = Arc::new(RelayerRepositoryStorage::new_in_memory());

        // Create a relayer that is NOT disabled
        let mut relayer = create_disabled_relayer("test-check-enabled");
        relayer.system_disabled = false;
        relayer.disabled_reason = None;
        relayer_repo.create(relayer).await.unwrap();

        // Create a mock job producer (should not be called)
        let mock_job_producer = MockJobProducerTrait::new();

        // Create app state
        let app_state = AppState {
            relayer_repository: relayer_repo.clone(),
            transaction_repository: Arc::new(TransactionRepositoryStorage::new_in_memory()),
            signer_repository: Arc::new(SignerRepositoryStorage::new_in_memory()),
            notification_repository: Arc::new(NotificationRepositoryStorage::new_in_memory()),
            network_repository: Arc::new(NetworkRepositoryStorage::new_in_memory()),
            transaction_counter_store: Arc::new(
                TransactionCounterRepositoryStorage::new_in_memory(),
            ),
            job_producer: Arc::new(mock_job_producer),
            plugin_repository: Arc::new(PluginRepositoryStorage::new_in_memory()),
            api_key_repository: Arc::new(ApiKeyRepositoryStorage::new_in_memory()),
        };

        // Create health check data
        let health_check = RelayerHealthCheck::new("test-check-enabled".to_string());

        // Wrap in ThinData for the generic function signature
        let thin_app_state = actix_web::web::ThinData(app_state);

        // Call the function - should exit early without error
        let result = check_and_reenable_relayer(health_check, &thin_app_state).await;

        // Should succeed (exits early because relayer is not disabled)
        assert!(result.is_ok());

        // Verify relayer state hasn't changed
        let retrieved = relayer_repo
            .get_by_id("test-check-enabled".to_string())
            .await
            .unwrap();
        assert!(!retrieved.system_disabled);
        assert!(retrieved.disabled_reason.is_none());
    }

    #[tokio::test]
    async fn test_check_and_reenable_variant_comparison() {
        // Test the logic that only updates disabled_reason when the variant changes
        use crate::models::DisabledReason;

        // Same variant, different message - should NOT trigger update
        let reason1 = DisabledReason::RpcValidationFailed("Error A".to_string());
        let reason2 = DisabledReason::RpcValidationFailed("Error B".to_string());
        assert!(reason1.same_variant(&reason2));

        // Different variants - should trigger update
        let reason3 = DisabledReason::NonceSyncFailed("Error".to_string());
        assert!(!reason1.same_variant(&reason3));

        // Multiple reasons with same variants in same order
        let multi1 = DisabledReason::Multiple(vec![
            DisabledReason::RpcValidationFailed("A".to_string()),
            DisabledReason::NonceSyncFailed("B".to_string()),
        ]);
        let multi2 = DisabledReason::Multiple(vec![
            DisabledReason::RpcValidationFailed("C".to_string()),
            DisabledReason::NonceSyncFailed("D".to_string()),
        ]);
        assert!(multi1.same_variant(&multi2));

        // Multiple reasons with different variants - should not match
        let multi3 = DisabledReason::Multiple(vec![
            DisabledReason::RpcValidationFailed("A".to_string()),
            DisabledReason::BalanceCheckFailed("B".to_string()),
        ]);
        assert!(!multi1.same_variant(&multi3));
    }

    #[tokio::test]
    async fn test_backoff_delay_calculation_edge_cases() {
        // Test backoff calculation for edge cases

        // Test retry count 0 (first retry)
        let delay0 = calculate_backoff_delay(0);
        assert_eq!(delay0, Duration::from_secs(10));

        // Test very large retry count (should still cap at 60)
        let delay_large = calculate_backoff_delay(100);
        assert_eq!(delay_large, Duration::from_secs(60));

        // Test that each step increases until cap
        let mut prev_delay = Duration::from_secs(0);
        for retry in 0..10 {
            let delay = calculate_backoff_delay(retry);
            if delay < Duration::from_secs(60) {
                // Before cap, should increase
                assert!(delay > prev_delay, "Retry {}: delay should increase", retry);
            } else {
                // At or after cap, should stay at 60
                assert_eq!(
                    delay,
                    Duration::from_secs(60),
                    "Retry {}: should cap at 60s",
                    retry
                );
            }
            prev_delay = delay;
        }
    }

    #[tokio::test]
    async fn test_disabled_reason_from_health_failures() {
        use crate::models::{DisabledReason, HealthCheckFailure};

        // Test empty failures
        let empty_result = DisabledReason::from_health_failures(vec![]);
        assert!(empty_result.is_none());

        // Test single failure
        let single_failure = vec![HealthCheckFailure::RpcValidationFailed(
            "RPC down".to_string(),
        )];
        let single_result = DisabledReason::from_health_failures(single_failure);
        assert!(single_result.is_some());
        match single_result.unwrap() {
            DisabledReason::RpcValidationFailed(msg) => {
                assert_eq!(msg, "RPC down");
            }
            _ => panic!("Expected RpcValidationFailed variant"),
        }

        // Test multiple failures
        let multiple_failures = vec![
            HealthCheckFailure::RpcValidationFailed("RPC error".to_string()),
            HealthCheckFailure::NonceSyncFailed("Nonce error".to_string()),
        ];
        let multiple_result = DisabledReason::from_health_failures(multiple_failures);
        assert!(multiple_result.is_some());
        match multiple_result.unwrap() {
            DisabledReason::Multiple(reasons) => {
                assert_eq!(reasons.len(), 2);
                assert!(matches!(reasons[0], DisabledReason::RpcValidationFailed(_)));
                assert!(matches!(reasons[1], DisabledReason::NonceSyncFailed(_)));
            }
            _ => panic!("Expected Multiple variant"),
        }
    }

    #[tokio::test]
    async fn test_relayer_health_check_retry_count_increments() {
        // This test verifies that retry counts are properly incremented
        let retry_counts = vec![0, 1, 2, 5, 10];

        for retry_count in retry_counts {
            let health_check =
                RelayerHealthCheck::with_retry_count("test-relayer".to_string(), retry_count);

            // Verify the retry count is set correctly
            assert_eq!(health_check.retry_count, retry_count);

            // Verify the next retry would be incremented
            let next_health_check =
                RelayerHealthCheck::with_retry_count("test-relayer".to_string(), retry_count + 1);
            assert_eq!(next_health_check.retry_count, retry_count + 1);

            // Verify backoff increases with retry count (up to cap)
            let current_delay = calculate_backoff_delay(retry_count);
            let next_delay = calculate_backoff_delay(retry_count + 1);

            if current_delay < Duration::from_secs(60) {
                assert!(next_delay >= current_delay);
            } else {
                assert_eq!(next_delay, Duration::from_secs(60));
            }
        }
    }

    #[tokio::test]
    async fn test_repository_enable_disable_operations() {
        use crate::models::DisabledReason;
        use crate::repositories::{RelayerRepositoryStorage, Repository};

        let repo = RelayerRepositoryStorage::new_in_memory();

        // Create an initially enabled relayer
        let mut relayer = create_disabled_relayer("test-enable-disable");
        relayer.system_disabled = false;
        relayer.disabled_reason = None;
        repo.create(relayer).await.unwrap();

        // Disable the relayer
        let reason = DisabledReason::RpcValidationFailed("Test error".to_string());
        let disabled = repo
            .disable_relayer("test-enable-disable".to_string(), reason.clone())
            .await
            .unwrap();

        assert!(disabled.system_disabled);
        assert_eq!(disabled.disabled_reason, Some(reason));

        // Re-enable the relayer
        let enabled = repo
            .enable_relayer("test-enable-disable".to_string())
            .await
            .unwrap();

        assert!(!enabled.system_disabled);
        assert!(enabled.disabled_reason.is_none());

        // Verify state persists
        let retrieved = repo
            .get_by_id("test-enable-disable".to_string())
            .await
            .unwrap();
        assert!(!retrieved.system_disabled);
        assert!(retrieved.disabled_reason.is_none());
    }

    #[tokio::test]
    async fn test_disabled_reason_safe_description() {
        use crate::models::DisabledReason;

        // Test that safe_description doesn't leak sensitive information
        let reasons = vec![
            DisabledReason::NonceSyncFailed("Error with API key abc123".to_string()),
            DisabledReason::RpcValidationFailed(
                "RPC error: http://secret-rpc.com:8545".to_string(),
            ),
            DisabledReason::BalanceCheckFailed("Balance: 1.5 ETH at address 0x123...".to_string()),
        ];

        for reason in reasons {
            let safe_desc = reason.safe_description();

            // Safe description should not contain sensitive details
            assert!(!safe_desc.contains("abc123"));
            assert!(!safe_desc.contains("http://"));
            assert!(!safe_desc.contains("0x123"));
            assert!(!safe_desc.contains("1.5 ETH"));

            // But should be informative
            assert!(!safe_desc.is_empty());
        }

        // Test Multiple variant
        let multiple = DisabledReason::Multiple(vec![
            DisabledReason::RpcValidationFailed("Secret RPC info".to_string()),
            DisabledReason::NonceSyncFailed("Secret nonce info".to_string()),
        ]);

        let safe_desc = multiple.safe_description();
        assert!(!safe_desc.contains("Secret"));
        assert!(safe_desc.contains("RPC endpoint validation failed"));
        assert!(safe_desc.contains("Nonce synchronization failed"));
    }
}
