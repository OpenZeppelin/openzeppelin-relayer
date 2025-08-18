//! Application state initialization
//!
//! This module contains functions for initializing the application state,
//! including setting up repositories, job queues, and other necessary components.
use crate::{
    config::{RepositoryStorageType, ServerConfig},
    jobs::{self, Queue},
    models::{AppState, DefaultAppState},
    repositories::{
        NetworkRepositoryStorage, NotificationRepositoryStorage, PluginRepositoryStorage,
        RelayerRepositoryStorage, SignerRepositoryStorage, TransactionCounterRepositoryStorage,
        TransactionRepositoryStorage,
    },
    utils::initialize_redis_connection,
};
use actix_web::web;
use color_eyre::Result;
use log::warn;
use std::sync::Arc;

pub struct RepositoryCollection {
    pub relayer: Arc<RelayerRepositoryStorage>,
    pub transaction: Arc<TransactionRepositoryStorage>,
    pub signer: Arc<SignerRepositoryStorage>,
    pub notification: Arc<NotificationRepositoryStorage>,
    pub network: Arc<NetworkRepositoryStorage>,
    pub transaction_counter: Arc<TransactionCounterRepositoryStorage>,
    pub plugin: Arc<PluginRepositoryStorage>,
}

/// Initializes repositories based on the server configuration
///
/// # Returns
///
/// * `Result<RepositoryCollection>` - Initialized repositories
///
/// # Errors
pub async fn initialize_repositories(config: &ServerConfig) -> eyre::Result<RepositoryCollection> {
    let repositories = match config.repository_storage_type {
        RepositoryStorageType::InMemory => RepositoryCollection {
            relayer: Arc::new(RelayerRepositoryStorage::new_in_memory()),
            transaction: Arc::new(TransactionRepositoryStorage::new_in_memory()),
            signer: Arc::new(SignerRepositoryStorage::new_in_memory()),
            notification: Arc::new(NotificationRepositoryStorage::new_in_memory()),
            network: Arc::new(NetworkRepositoryStorage::new_in_memory()),
            transaction_counter: Arc::new(TransactionCounterRepositoryStorage::new_in_memory()),
            plugin: Arc::new(PluginRepositoryStorage::new_in_memory()),
        },
        RepositoryStorageType::Redis => {
            warn!("⚠️ Redis repository storage support is experimental. Use with caution.");

            if config.storage_encryption_key.is_none() {
                warn!("⚠️ Storage encryption key is not set. Please set the STORAGE_ENCRYPTION_KEY environment variable.");
                return Err(eyre::eyre!("Storage encryption key is not set. Please set the STORAGE_ENCRYPTION_KEY environment variable."));
            }

            let connection_manager = initialize_redis_connection(config).await?;

            RepositoryCollection {
                relayer: Arc::new(RelayerRepositoryStorage::new_redis(
                    connection_manager.clone(),
                    config.redis_key_prefix.clone(),
                )?),
                transaction: Arc::new(TransactionRepositoryStorage::new_redis(
                    connection_manager.clone(),
                    config.redis_key_prefix.clone(),
                )?),
                signer: Arc::new(SignerRepositoryStorage::new_redis(
                    connection_manager.clone(),
                    config.redis_key_prefix.clone(),
                )?),
                notification: Arc::new(NotificationRepositoryStorage::new_redis(
                    connection_manager.clone(),
                    config.redis_key_prefix.clone(),
                )?),
                network: Arc::new(NetworkRepositoryStorage::new_redis(
                    connection_manager.clone(),
                    config.redis_key_prefix.clone(),
                )?),
                transaction_counter: Arc::new(TransactionCounterRepositoryStorage::new_redis(
                    connection_manager.clone(),
                    config.redis_key_prefix.clone(),
                )?),
                plugin: Arc::new(PluginRepositoryStorage::new_redis(
                    connection_manager,
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
    let repositories = initialize_repositories(&server_config).await?;

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
    });

    Ok(app_state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::RepositoryStorageType,
        repositories::Repository,
        utils::mocks::mockutils::{
            create_mock_network, create_mock_relayer, create_mock_signer, create_test_server_config,
        },
    };
    use std::sync::Arc;

    #[tokio::test]
    async fn test_initialize_repositories_in_memory() {
        let config = create_test_server_config(RepositoryStorageType::InMemory);
        let result = initialize_repositories(&config).await;

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
    }

    #[tokio::test]
    async fn test_repository_collection_functionality() {
        let config = create_test_server_config(RepositoryStorageType::InMemory);
        let repositories = initialize_repositories(&config).await.unwrap();

        // Test basic repository operations
        let relayer = create_mock_relayer("test-relayer".to_string(), false);
        let signer = create_mock_signer();
        let network = create_mock_network();

        // Test creating and retrieving items
        repositories.relayer.create(relayer.clone()).await.unwrap();
        repositories.signer.create(signer.clone()).await.unwrap();
        repositories.network.create(network.clone()).await.unwrap();

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

        assert_eq!(retrieved_relayer.id, "test-relayer");
        assert_eq!(retrieved_signer.id, "test");
        assert_eq!(retrieved_network.id, "test");
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
}
