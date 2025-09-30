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
        split_handler_error, HandlerErrorParts, PluginHandlerError, PluginRunner, PluginService,
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
        Ok(plugin_result) => Ok(HttpResponse::Ok().json(ApiResponse::success(plugin_result))),
        Err(error) => match split_handler_error(error) {
            Ok(parts) => {
                let status = parts.status;
                let http_status =
                    StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                let derived_message = parts.derived_message();
                let HandlerErrorParts {
                    code,
                    details,
                    logs,
                    traces,
                    ..
                } = parts;
                let log_count = logs.as_ref().map(|logs| logs.len()).unwrap_or(0);
                let trace_count = traces.as_ref().map(|traces| traces.len()).unwrap_or(0);
                let payload = PluginHandlerError {
                    code,
                    details,
                    logs,
                    traces,
                };

                // This is an intentional error thrown by the plugin handler - log at debug level
                tracing::debug!(
                    status,
                    message = %derived_message,
                    code = ?payload.code.as_ref(),
                    details = ?payload.details.as_ref(),
                    log_count,
                    trace_count,
                    "Plugin handler error"
                );

                Ok(HttpResponse::build(http_status).json(ApiResponse::new(
                    Some(payload),
                    Some(derived_message),
                    None,
                )))
            }
            Err(e) => {
                tracing::error!("Plugin error: {:?}", e);
                Ok(HttpResponse::InternalServerError()
                    .json(ApiResponse::<String>::error("Internal server error")))
            }
        },
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
            legacy_payload: false,
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
