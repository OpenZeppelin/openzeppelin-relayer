//! # Relayer Controller
//!
//! Handles HTTP endpoints for relayer operations including:
//! - Listing relayers
//! - Getting relayer details
//! - Submitting transactions
//! - Signing messages
//! - JSON-RPC proxy
use crate::{
    jobs::JobProducer,
    models::{ApiError, ApiResponse, AppState, PluginCallRequest},
    services::plugins::PluginService,
};
use actix_web::{web, HttpResponse};
use eyre::Result;

/// Call plugin
///
/// # Arguments
///
/// * `plugin_call_request` - The plugin call request.
/// * `state` - The application state containing the relayer repository.
///
/// # Returns
///
/// A paginated list of relayers.
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

    let plugin_service = PluginService::default();

    let result = plugin_service.call_plugin(&plugin.path, plugin_call_request);

    Ok(HttpResponse::Ok().json(ApiResponse::success(result)))
}
