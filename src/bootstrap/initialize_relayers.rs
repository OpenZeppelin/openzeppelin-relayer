//! Relayer initialization
//!
//! This module contains functions for initializing relayers, ensuring they are
//! properly configured and ready for operation.
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
};
use color_eyre::{eyre::WrapErr, Result};
use futures::future::try_join_all;
use tracing::debug;

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
        .wrap_err_with(|| format!("Failed to initialize relayer: {}", relayer_id))?;

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

    let relayer_futures = relayers.iter().map(|relayer| {
        let app_state = app_state.clone();
        async move { initialize_relayer(relayer.id.clone(), app_state).await }
    });

    try_join_all(relayer_futures)
        .await
        .wrap_err("Failed to initialize relayers")?;

    debug!(
        count = relayers.len(),
        "All relayers initialized successfully"
    );
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
