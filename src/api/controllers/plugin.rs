//! # Plugin Controller
//!
//! Handles HTTP endpoints for plugin operations including:
//! - Calling plugins
use crate::{
    jobs::JobProducer,
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
pub async fn call_plugin(
    plugin_id: String,
    plugin_call_request: PluginCallRequest,
    state: web::ThinData<AppState<JobProducer>>,
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
