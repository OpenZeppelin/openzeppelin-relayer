//! Application state initialization
//!
//! This module contains functions for initializing the application state,
//! including setting up repositories, job queues, and other necessary components.
use crate::{
    config::ServerConfig,
    jobs::{self, Queue},
    models::{AppState, DefaultAppState},
    repositories::{
        NetworkRepositoryImpl, NotificationRepositoryImpl, PluginRepositoryImpl,
        RelayerRepositoryImpl, SignerRepositoryImpl, TransactionCounterRepositoryImpl,
        TransactionRepositoryType,
    },
};
use actix_web::web;
use color_eyre::Result;
use std::sync::Arc;

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
    let relayer_repository = Arc::new(RelayerRepositoryImpl::new_in_memory());

    let transaction_repository = Arc::new(
        TransactionRepositoryType::Redis
            .create_repository(&server_config)
            .await,
    );
    let signer_repository = Arc::new(SignerRepositoryImpl::new_in_memory());
    let notification_repository = Arc::new(NotificationRepositoryImpl::new_in_memory());
    let network_repository = Arc::new(NetworkRepositoryImpl::new_in_memory());
    let transaction_counter_store = Arc::new(TransactionCounterRepositoryImpl::new_in_memory());
    let queue = Queue::setup().await?;
    let job_producer = Arc::new(jobs::JobProducer::new(queue.clone()));
    let plugin_repository = Arc::new(PluginRepositoryImpl::new_in_memory());

    let app_state = web::ThinData(AppState {
        relayer_repository,
        transaction_repository,
        signer_repository,
        notification_repository,
        network_repository,
        transaction_counter_store,
        job_producer,
        plugin_repository,
    });

    Ok(app_state)
}
