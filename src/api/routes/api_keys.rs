//! This module defines the HTTP routes for api keys operations.
//! The routes are integrated with the Actix-web framework and interact with the api key controller.
use crate::{
    api::controllers::api_key,
    models::{ApiKeyRequest, DefaultAppState, PaginationQuery},
};
use actix_web::{delete, get, post, web, Responder};

/// List plugins
#[get("/api-keys")]
async fn list_api_keys(
    query: web::Query<PaginationQuery>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    api_key::list_api_keys(query.into_inner(), data).await
}

#[get("/api-keys/{api_key_id}/permissions")]
async fn get_api_key_permissions(
    api_key_id: web::Path<String>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    api_key::get_api_key_permissions(api_key_id.into_inner(), data).await
}

/// Calls a plugin method.
#[post("/api-keys")]
async fn create_api_key(
    req: web::Json<ApiKeyRequest>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    api_key::create_api_key(req.into_inner(), data).await
}

#[delete("/api-keys/{api_key_id}")]
async fn delete_api_key(
    api_key_id: web::Path<String>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    api_key::delete_api_key(api_key_id.into_inner(), data).await
}

/// Initializes the routes for api keys.
pub fn init(cfg: &mut web::ServiceConfig) {
    // Register routes with literal segments before routes with path parameters
    cfg.service(create_api_key); // /api-keys
    cfg.service(list_api_keys); // /api-keys
    cfg.service(get_api_key_permissions); // /api-keys/{api_key_id}/permissions
    cfg.service(delete_api_key); // /api-keys/{api_key_id}
}

#[cfg(test)]
mod tests {}
