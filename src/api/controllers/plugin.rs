//! # Plugin Controller
//!
//! Handles HTTP endpoints for plugin operations including:
//! - Calling plugins
use crate::{
    jobs::JobProducerTrait,
    models::{ApiError, ApiResponse, AppState, PluginCallRequest},
    services::plugins::{PluginService, PluginServiceTrait},
};
use actix_web::{web, HttpResponse};
use eyre::Result;

/// Call plugin
///
/// # Arguments
///
/// * `plugin_id` - The ID of the plugin to call.
/// * `plugin_call_request` - The plugin call request.
/// * `state` - The application state containing the plugin repository.
///
/// # Returns
///
/// The result of the plugin call.
pub async fn call_plugin<J: JobProducerTrait>(
    plugin_id: String,
    plugin_call_request: PluginCallRequest,
    state: web::ThinData<AppState<J>>,
) -> Result<HttpResponse, ApiError> {
    let plugin = state
        .plugin_repository
        .get_by_id(&plugin_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Plugin with id {} not found", plugin_id)))?;

    let plugin_service = PluginService::new();
    let result = plugin_service
        .call_plugin(&plugin.path, plugin_call_request)
        .await;

    Ok(HttpResponse::Ok().json(ApiResponse::success(result)))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        jobs::MockJobProducerTrait,
        models::PluginModel,
        repositories::{
            InMemoryNetworkRepository, InMemoryNotificationRepository, InMemoryPluginRepository,
            InMemoryRelayerRepository, InMemorySignerRepository, InMemoryTransactionCounter,
            InMemoryTransactionRepository, PluginRepositoryTrait, RelayerRepositoryStorage,
        },
    };

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

    #[actix_web::test]
    async fn test_call_plugin() {
        let app_state = get_test_app_state().await;
        let plugin_call_request = PluginCallRequest {
            params: serde_json::json!({"key":"value"}),
        };
        let response = call_plugin(
            "test-plugin".to_string(),
            plugin_call_request,
            web::ThinData(app_state),
        )
        .await;
        assert!(response.is_ok());
    }
}
