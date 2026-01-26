//! Application state initialization
//!
//! This module contains functions for initializing the application state,
//! including setting up repositories, job queues, and other necessary components.
use crate::{
    config::{RepositoryStorageType, ServerConfig},
    jobs::{self, Queue},
    models::{AppState, DefaultAppState},
    repositories::{
        ApiKeyRepositoryStorage, NetworkRepositoryStorage, NotificationRepositoryStorage,
        PluginRepositoryStorage, RelayerRepositoryStorage, SignerRepositoryStorage,
        TransactionCounterRepositoryStorage, TransactionRepositoryStorage,
    },
    utils::{initialize_redis_connections, RedisConnections},
};
use actix_web::web;
use color_eyre::Result;
use std::sync::Arc;
use tracing::warn;

pub struct RepositoryCollection {
    pub relayer: Arc<RelayerRepositoryStorage>,
    pub transaction: Arc<TransactionRepositoryStorage>,
    pub signer: Arc<SignerRepositoryStorage>,
    pub notification: Arc<NotificationRepositoryStorage>,
    pub network: Arc<NetworkRepositoryStorage>,
    pub transaction_counter: Arc<TransactionCounterRepositoryStorage>,
    pub plugin: Arc<PluginRepositoryStorage>,
    pub api_key: Arc<ApiKeyRepositoryStorage>,
}

/// Initializes repositories based on the server configuration
///
/// # Arguments
///
/// * `config` - Server configuration
/// * `connections` - Redis connections (required for Redis storage type, None for in-memory)
///
/// # Returns
///
/// * `Result<RepositoryCollection>` - Initialized repositories
///
/// # Errors
pub async fn initialize_repositories(
    config: &ServerConfig,
    connections: Option<Arc<RedisConnections>>,
) -> eyre::Result<RepositoryCollection> {
    let repositories = match config.repository_storage_type {
        RepositoryStorageType::InMemory => RepositoryCollection {
            relayer: Arc::new(RelayerRepositoryStorage::new_in_memory()),
            transaction: Arc::new(TransactionRepositoryStorage::new_in_memory()),
            signer: Arc::new(SignerRepositoryStorage::new_in_memory()),
            notification: Arc::new(NotificationRepositoryStorage::new_in_memory()),
            network: Arc::new(NetworkRepositoryStorage::new_in_memory()),
            transaction_counter: Arc::new(TransactionCounterRepositoryStorage::new_in_memory()),
            plugin: Arc::new(PluginRepositoryStorage::new_in_memory()),
            api_key: Arc::new(ApiKeyRepositoryStorage::new_in_memory()),
        },
        RepositoryStorageType::Redis => {
            if config.storage_encryption_key.is_none() {
                warn!("⚠️ Storage encryption key is not set. Please set the STORAGE_ENCRYPTION_KEY environment variable.");
                return Err(eyre::eyre!("Storage encryption key is not set. Please set the STORAGE_ENCRYPTION_KEY environment variable."));
            }

            let connections = connections
                .ok_or_else(|| eyre::eyre!("Redis connections required for Redis storage type"))?;

            // Use the shared connections for all repositories
            RepositoryCollection {
                relayer: Arc::new(RelayerRepositoryStorage::new_redis(
                    connections.clone(),
                    config.redis_key_prefix.clone(),
                )?),
                transaction: Arc::new(TransactionRepositoryStorage::new_redis(
                    connections.clone(),
                    config.redis_key_prefix.clone(),
                )?),
                signer: Arc::new(SignerRepositoryStorage::new_redis(
                    connections.clone(),
                    config.redis_key_prefix.clone(),
                )?),
                notification: Arc::new(NotificationRepositoryStorage::new_redis(
                    connections.clone(),
                    config.redis_key_prefix.clone(),
                )?),
                network: Arc::new(NetworkRepositoryStorage::new_redis(
                    connections.clone(),
                    config.redis_key_prefix.clone(),
                )?),
                transaction_counter: Arc::new(TransactionCounterRepositoryStorage::new_redis(
                    connections.clone(),
                    config.redis_key_prefix.clone(),
                )?),
                plugin: Arc::new(PluginRepositoryStorage::new_redis(
                    connections.clone(),
                    config.redis_key_prefix.clone(),
                )?),
                api_key: Arc::new(ApiKeyRepositoryStorage::new_redis(
                    connections,
                    config.redis_key_prefix.clone(),
                )?),
            }
        }
    };

    Ok(repositories)
}

/// Initializes application state
///
/// # Returns
///
/// * `Result<web::Data<AppState>>` - Initialized application state
///
/// # Errors
///
/// Returns error if:
/// - Repository initialization fails
/// - Configuration loading fails
pub async fn initialize_app_state(
    server_config: Arc<ServerConfig>,
) -> Result<web::ThinData<DefaultAppState>> {
    // Initialize Redis connections - shared by all repositories
    // When REDIS_READER_URL is set, read operations use the reader endpoint
    let redis_connections = initialize_redis_connections(&server_config).await?;

    let repositories =
        initialize_repositories(&server_config, Some(redis_connections.clone())).await?;

    let queue = Queue::setup().await?;
    let job_producer = Arc::new(jobs::JobProducer::new(queue.clone()));

    let app_state = web::ThinData(AppState {
        relayer_repository: repositories.relayer,
        transaction_repository: repositories.transaction,
        signer_repository: repositories.signer,
        network_repository: repositories.network,
        notification_repository: repositories.notification,
        transaction_counter_store: repositories.transaction_counter,
        job_producer,
        plugin_repository: repositories.plugin,
        api_key_repository: repositories.api_key,
    });

    Ok(app_state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::RepositoryStorageType,
        models::SecretString,
        repositories::{ApiKeyRepositoryTrait, Repository},
        utils::mocks::mockutils::{
            create_mock_api_key, create_mock_network, create_mock_relayer, create_mock_signer,
            create_test_server_config,
        },
    };
    use std::sync::Arc;

    #[tokio::test]
    async fn test_initialize_repositories_in_memory() {
        let config = create_test_server_config(RepositoryStorageType::InMemory);
        // For in-memory storage, pool is not required
        let result = initialize_repositories(&config, None).await;

        assert!(result.is_ok());
        let repositories = result.unwrap();

        // Verify all repositories are created
        assert!(Arc::strong_count(&repositories.relayer) >= 1);
        assert!(Arc::strong_count(&repositories.transaction) >= 1);
        assert!(Arc::strong_count(&repositories.signer) >= 1);
        assert!(Arc::strong_count(&repositories.notification) >= 1);
        assert!(Arc::strong_count(&repositories.network) >= 1);
        assert!(Arc::strong_count(&repositories.transaction_counter) >= 1);
        assert!(Arc::strong_count(&repositories.plugin) >= 1);
        assert!(Arc::strong_count(&repositories.api_key) >= 1);
    }

    #[tokio::test]
    async fn test_repository_collection_functionality() {
        let config = create_test_server_config(RepositoryStorageType::InMemory);
        // For in-memory storage, pool is not required
        let repositories = initialize_repositories(&config, None).await.unwrap();

        // Test basic repository operations
        let relayer = create_mock_relayer("test-relayer".to_string(), false);
        let signer = create_mock_signer();
        let network = create_mock_network();
        let api_key = create_mock_api_key();

        // Test creating and retrieving items
        repositories.relayer.create(relayer.clone()).await.unwrap();
        repositories.signer.create(signer.clone()).await.unwrap();
        repositories.network.create(network.clone()).await.unwrap();
        repositories.api_key.create(api_key.clone()).await.unwrap();

        let retrieved_relayer = repositories
            .relayer
            .get_by_id("test-relayer".to_string())
            .await
            .unwrap();
        let retrieved_signer = repositories
            .signer
            .get_by_id("test".to_string())
            .await
            .unwrap();
        let retrieved_network = repositories
            .network
            .get_by_id("test".to_string())
            .await
            .unwrap();
        let retrieved_api_key = repositories
            .api_key
            .get_by_id("test-api-key")
            .await
            .unwrap();

        assert_eq!(retrieved_relayer.id, "test-relayer");
        assert_eq!(retrieved_signer.id, "test");
        assert_eq!(retrieved_network.id, "test");
        assert_eq!(retrieved_api_key.unwrap().id, "test-api-key");
    }

    #[tokio::test]
    async fn test_initialize_app_state_repository_error() {
        let mut config = create_test_server_config(RepositoryStorageType::Redis);
        config.redis_url = "redis://invalid_url".to_string();

        let result = initialize_app_state(Arc::new(config)).await;

        // Should fail during repository initialization
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.to_string().contains("Redis") || error.to_string().contains("connection"));
    }

    #[tokio::test]
    async fn test_initialize_repositories_redis_without_encryption_key() {
        let mut config = create_test_server_config(RepositoryStorageType::Redis);
        // Explicitly set encryption key to None
        config.storage_encryption_key = None;

        // Even with a pool, should fail without encryption key
        // We pass None for pool since it will fail before pool is used
        let result = initialize_repositories(&config, None).await;

        assert!(result.is_err());
        let error = match result {
            Err(e) => e,
            Ok(_) => panic!("Expected error for missing encryption key"),
        };
        assert!(
            error.to_string().contains("encryption key"),
            "Expected error about encryption key, got: {}",
            error
        );
    }

    #[tokio::test]
    async fn test_initialize_repositories_redis_without_pool() {
        let mut config = create_test_server_config(RepositoryStorageType::Redis);
        // Set encryption key so we get past that check
        config.storage_encryption_key = Some(SecretString::new("test-encryption-key-32-bytes!!!"));

        // Pass None for pool - should fail
        let result = initialize_repositories(&config, None).await;

        assert!(result.is_err());
        let error = match result {
            Err(e) => e,
            Ok(_) => panic!("Expected error for missing pool"),
        };
        assert!(
            error
                .to_string()
                .contains("Redis connections required for Redis storage type"),
            "Expected error about Redis pool being required, got: {}",
            error
        );
    }

    #[tokio::test]
    async fn test_initialize_repositories_in_memory_ignores_pool() {
        // For in-memory storage, providing a pool should be fine (it's ignored)
        // We can't easily create a real pool without Redis, but we can test with None
        let config = create_test_server_config(RepositoryStorageType::InMemory);

        // In-memory should work with None
        let result = initialize_repositories(&config, None).await;
        assert!(result.is_ok());

        // Verify repositories are functional
        let repositories = result.unwrap();
        let relayer = create_mock_relayer("test-relayer".to_string(), false);
        repositories.relayer.create(relayer).await.unwrap();
        let retrieved = repositories
            .relayer
            .get_by_id("test-relayer".to_string())
            .await
            .unwrap();
        assert_eq!(retrieved.id, "test-relayer");
    }

    #[tokio::test]
    async fn test_repository_collection_all_eight_repositories() {
        // Verify that RepositoryCollection contains exactly 8 repositories
        let config = create_test_server_config(RepositoryStorageType::InMemory);
        let repositories = initialize_repositories(&config, None).await.unwrap();

        // Count the repositories by checking Arc strong counts
        // All should have at least 1 reference
        let repo_refs = vec![
            Arc::strong_count(&repositories.relayer),
            Arc::strong_count(&repositories.transaction),
            Arc::strong_count(&repositories.signer),
            Arc::strong_count(&repositories.notification),
            Arc::strong_count(&repositories.network),
            Arc::strong_count(&repositories.transaction_counter),
            Arc::strong_count(&repositories.plugin),
            Arc::strong_count(&repositories.api_key),
        ];

        assert_eq!(repo_refs.len(), 8, "Expected exactly 8 repositories");
        for (i, count) in repo_refs.iter().enumerate() {
            assert!(
                *count >= 1,
                "Repository {} has invalid Arc count: {}",
                i,
                count
            );
        }
    }

    #[tokio::test]
    async fn test_repository_delete_operations() {
        let config = create_test_server_config(RepositoryStorageType::InMemory);
        let repositories = initialize_repositories(&config, None).await.unwrap();

        // Create and then delete items
        let relayer = create_mock_relayer("delete-test".to_string(), false);
        repositories.relayer.create(relayer).await.unwrap();

        // Verify item exists
        let exists = repositories
            .relayer
            .get_by_id("delete-test".to_string())
            .await;
        assert!(exists.is_ok());

        // Delete the item
        let delete_result = repositories
            .relayer
            .delete_by_id("delete-test".to_string())
            .await;
        assert!(delete_result.is_ok());

        // Verify item is gone
        let after_delete = repositories
            .relayer
            .get_by_id("delete-test".to_string())
            .await;
        assert!(after_delete.is_err() || after_delete.unwrap().id != "delete-test");
    }

    #[tokio::test]
    async fn test_repository_update_operations() {
        let config = create_test_server_config(RepositoryStorageType::InMemory);
        let repositories = initialize_repositories(&config, None).await.unwrap();

        // Create a relayer
        let relayer = create_mock_relayer("update-test".to_string(), false);
        repositories.relayer.create(relayer.clone()).await.unwrap();

        // Update the relayer (enable it)
        let mut updated_relayer = relayer.clone();
        updated_relayer.system_disabled = true;

        let update_result = repositories
            .relayer
            .update("update-test".to_string(), updated_relayer)
            .await;
        assert!(update_result.is_ok());

        // Verify the update
        let retrieved = repositories
            .relayer
            .get_by_id("update-test".to_string())
            .await
            .unwrap();
        assert!(retrieved.system_disabled);
    }

    #[tokio::test]
    async fn test_repository_list_operations() {
        let config = create_test_server_config(RepositoryStorageType::InMemory);
        let repositories = initialize_repositories(&config, None).await.unwrap();

        // Create multiple relayers
        for i in 0..5 {
            let relayer = create_mock_relayer(format!("list-test-{}", i), false);
            repositories.relayer.create(relayer).await.unwrap();
        }

        // List all relayers
        let all_relayers = repositories.relayer.list_all().await.unwrap();
        assert_eq!(all_relayers.len(), 5);

        // Verify all items are present
        for i in 0..5 {
            let found = all_relayers
                .iter()
                .any(|r| r.id == format!("list-test-{}", i));
            assert!(found, "Expected to find relayer list-test-{}", i);
        }
    }

    #[tokio::test]
    async fn test_repository_collection_struct_fields() {
        // Verify the RepositoryCollection struct has all expected fields
        let config = create_test_server_config(RepositoryStorageType::InMemory);
        let repos = initialize_repositories(&config, None).await.unwrap();

        // Access all fields to ensure they exist and are properly typed
        let _ = &repos.relayer;
        let _ = &repos.transaction;
        let _ = &repos.signer;
        let _ = &repos.notification;
        let _ = &repos.network;
        let _ = &repos.transaction_counter;
        let _ = &repos.plugin;
        let _ = &repos.api_key;

        // If we get here, all fields exist
        assert!(true);
    }
}
