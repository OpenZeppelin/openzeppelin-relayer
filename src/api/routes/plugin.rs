//! This module defines the HTTP routes for plugin operations.
//! It includes handlers for calling plugin methods.
//! The routes are integrated with the Actix-web framework and interact with the plugin controller.
use crate::{
    api::controllers::plugin,
    jobs::JobProducer,
    models::{AppState, PluginCallRequest},
};
use actix_web::{post, web, Responder};

/// Calls a plugin method.
#[post("/plugins/{plugin_id}/call")]
async fn plugin_call(
    plugin_id: web::Path<String>,
    req: web::Json<PluginCallRequest>,
    data: web::ThinData<AppState<JobProducer>>,
) -> impl Responder {
    plugin::call_plugin(plugin_id.into_inner(), req.into_inner(), data).await
}

/// Initializes the routes for the plugins module.
pub fn init(cfg: &mut web::ServiceConfig) {
    // Register routes with literal segments before routes with path parameters
    cfg.service(plugin_call); // /plugins/{plugin_id}/call
}
