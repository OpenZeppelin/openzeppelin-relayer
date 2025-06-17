//! Plugins service module for handling plugins execution and interaction with relayer
use std::sync::Arc;

use crate::{
    jobs::JobProducerTrait,
    models::{AppState, PluginCallRequest},
};
use actix_web::web;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub mod socket;
pub use socket::*;

#[cfg(test)]
use mockall::automock;

#[derive(Error, Debug, Serialize)]
pub enum PluginError {
    #[error("Socket error: {0}")]
    SocketError(String),
    #[error("Plugin error: {0}")]
    PluginError(String),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PluginCallResponse {
    pub success: bool,
    pub message: String,
    pub output: String,
    pub error: String,
}

pub struct PluginService {}

impl PluginService {
    pub fn new<J: JobProducerTrait + 'static>() -> Self {
        Self {}
    }
}

#[async_trait]
#[cfg_attr(test, automock)]
pub trait PluginServiceTrait<J: JobProducerTrait + 'static>: Send + Sync {
    fn new() -> Self;
    async fn call_plugin(
        &self,
        path: String,
        plugin_call_request: PluginCallRequest,
        state: Arc<web::ThinData<AppState<J>>>,
    ) -> Result<PluginCallResponse, PluginError>;
}

#[async_trait]
impl<J: JobProducerTrait + 'static> PluginServiceTrait<J> for PluginService {
    fn new() -> Self {
        Self {}
    }
    async fn call_plugin(
        &self,
        code_path: String,
        _plugin_call_request: PluginCallRequest,
        state: Arc<web::ThinData<AppState<J>>>,
    ) -> Result<PluginCallResponse, PluginError> {
        let socket_path = format!("/tmp/{}.sock", Uuid::new_v4());

        let socket_service = SocketService::from_path(&socket_path)
            .map_err(|e| PluginError::PluginError(e.to_string()))?;

        let result = socket_service
            .run(code_path, state)
            .await
            .map_err(|e| PluginError::PluginError(e.to_string()))?;

        Ok(PluginCallResponse {
            success: true,
            message: "Plugin called successfully".to_string(),
            output: result.output,
            error: result.error,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        jobs::MockJobProducerTrait,
        models::PluginModel,
        repositories::{
            InMemoryNetworkRepository, InMemoryNotificationRepository, InMemoryPluginRepository,
            InMemoryRelayerRepository, InMemorySignerRepository, InMemoryTransactionCounter,
            InMemoryTransactionRepository, PluginRepositoryTrait, RelayerRepositoryStorage,
        },
    };

    use super::*;

    async fn get_test_app_state() -> AppState<MockJobProducerTrait> {
        // adds a custom plugin
        let plugin_repository = InMemoryPluginRepository::new();
        let plugin = PluginModel {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
        };
        plugin_repository.add(plugin.clone()).await.unwrap();

        AppState {
            relayer_repository: Arc::new(RelayerRepositoryStorage::in_memory(
                InMemoryRelayerRepository::new(),
            )),
            transaction_repository: Arc::new(InMemoryTransactionRepository::new()),
            signer_repository: Arc::new(InMemorySignerRepository::new()),
            notification_repository: Arc::new(InMemoryNotificationRepository::new()),
            network_repository: Arc::new(InMemoryNetworkRepository::new()),
            transaction_counter_store: Arc::new(InMemoryTransactionCounter::new()),
            job_producer: Arc::new(MockJobProducerTrait::new()),
            plugin_repository: Arc::new(plugin_repository),
        }
    }

    #[tokio::test]
    async fn test_call_plugin() {
        let app_state = get_test_app_state().await;
        let plugin_service = PluginService::new::<MockJobProducerTrait>();
        let result = plugin_service
            .call_plugin(
                "test-plugin".to_string(),
                PluginCallRequest {
                    plugin_id: "test-plugin".to_string(),
                    params: serde_json::Value::Null,
                },
                Arc::new(web::ThinData(app_state)),
            )
            .await;
        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result.success);
    }
}
