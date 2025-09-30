//! # Plugin Controller
//!
//! Handles HTTP endpoints for plugin operations including:
//! - Calling plugins
use crate::{
    jobs::JobProducerTrait,
    models::{
        ApiError, ApiResponse, NetworkRepoModel, NotificationRepoModel, PaginationMeta,
        PaginationQuery, PluginCallRequest, PluginModel, RelayerRepoModel, SignerRepoModel,
        ThinDataAppState, TransactionRepoModel,
    },
    repositories::{
        ApiKeyRepositoryTrait, NetworkRepository, PluginRepositoryTrait, RelayerRepository,
        Repository, TransactionCounterTrait, TransactionRepository,
    },
    services::plugins::{
        PluginCallResponse, PluginCallResult, PluginHandlerResponse, PluginRunner, PluginService,
        PluginServiceTrait,
    },
};
use actix_web::{http::StatusCode, HttpResponse};
use eyre::Result;
use std::sync::Arc;

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
pub async fn call_plugin<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    plugin_id: String,
    plugin_call_request: PluginCallRequest,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
) -> Result<HttpResponse, ApiError>
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
    let plugin = state
        .plugin_repository
        .get_by_id(&plugin_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Plugin with id {} not found", plugin_id)))?;

    let plugin_runner = PluginRunner;
    let plugin_service = PluginService::new(plugin_runner);
    let result = plugin_service
        .call_plugin(plugin, plugin_call_request, Arc::new(state))
        .await;

    match result {
        PluginCallResult::Success(plugin_result) => {
            let PluginCallResponse { result, metadata } = plugin_result;

            let mut response = ApiResponse::success(result);
            response.metadata = metadata;
            Ok(HttpResponse::Ok().json(response))
        }
        PluginCallResult::Handler(handler) => {
            let PluginHandlerResponse {
                status,
                message,
                error,
                metadata,
            } = handler;

            let log_count = metadata
                .as_ref()
                .and_then(|meta| meta.logs.as_ref().map(|logs| logs.len()))
                .unwrap_or(0);
            let trace_count = metadata
                .as_ref()
                .and_then(|meta| meta.traces.as_ref().map(|traces| traces.len()))
                .unwrap_or(0);

            let http_status =
                StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

            // This is an intentional error thrown by the plugin handler - log at debug level
            tracing::debug!(
                status,
                message = %message,
                code = ?error.code.as_ref(),
                details = ?error.details.as_ref(),
                log_count,
                trace_count,
                "Plugin handler error"
            );

            let mut response = ApiResponse::new(Some(error), Some(message.clone()), None);
            response.metadata = metadata;
            Ok(HttpResponse::build(http_status).json(response))
        }
        PluginCallResult::Fatal(error) => {
            tracing::error!("Plugin error: {:?}", error);
            Ok(HttpResponse::InternalServerError()
                .json(ApiResponse::<String>::error("Internal server error")))
        }
    }
}

/// List plugins
///
/// # Arguments
///
/// * `query` - The pagination query parameters.
///     * `page` - The page number.
///     * `per_page` - The number of items per page.
/// * `state` - The application state containing the plugin repository.
///
/// # Returns
///
/// The result of the plugin list.
pub async fn list_plugins<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    query: PaginationQuery,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
) -> Result<HttpResponse, ApiError>
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
    let plugins = state.plugin_repository.list_paginated(query).await?;

    let plugin_items: Vec<PluginModel> = plugins.items.into_iter().collect();

    Ok(HttpResponse::Ok().json(ApiResponse::paginated(
        plugin_items,
        PaginationMeta {
            total_items: plugins.total,
            current_page: plugins.page,
            per_page: plugins.per_page,
        },
    )))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use actix_web::web;

    use crate::{
        constants::DEFAULT_PLUGIN_TIMEOUT_SECONDS, models::PluginModel,
        utils::mocks::mockutils::create_mock_app_state,
    };

    #[actix_web::test]
    async fn test_call_plugin() {
        let plugin = PluginModel {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
            timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS),
            emit_logs: false,
            emit_traces: false,
        };
        let app_state =
            create_mock_app_state(None, None, None, None, Some(vec![plugin]), None).await;
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
