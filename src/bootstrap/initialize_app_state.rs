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
};
use actix_web::web;
use color_eyre::Result;
use std::{sync::Arc, time::Duration};
use tokio::time::timeout;

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
            let redis_client = redis::Client::open(config.redis_url.as_str())?;
            let connection_manager = timeout(
                Duration::from_millis(config.redis_connection_timeout_ms),
                redis::aio::ConnectionManager::new(redis_client),
            )
            .await
            .map_err(|_| {
                eyre::eyre!(
                    "Redis connection timeout after {}ms",
                    config.redis_connection_timeout_ms
                )
            })??;
            let connection_manager = Arc::new(connection_manager);

            RepositoryCollection {
                relayer: Arc::new(RelayerRepositoryStorage::new_redis(
                    connection_manager.clone(),
                    config.redis_key_prefix.clone(),
                )?),
                transaction: Arc::new(TransactionRepositoryStorage::new_redis(
                    connection_manager.clone(),
                    config.redis_key_prefix.clone(),
                )?),
                // signer: Arc::new(SignerRepositoryStorage::new_redis(
                //     connection_manager.clone(),
                //     config.redis_key_prefix.clone(),
                // )?),
                signer: Arc::new(SignerRepositoryStorage::new_in_memory()),
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
