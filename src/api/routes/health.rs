//! This module provides a health check endpoint for the API.
//!
//! The `/health` endpoint can be used to verify that the service is running and responsive.
use actix_web::{get, web, HttpResponse};

/// Handles the `/health` endpoint.
///
/// Returns an `HttpResponse` with a status of `200 OK` and a body of `"OK"`.

#[utoipa::path(
    get,
    path = "/v1/health",
    tag = "Health",
    responses(
        (status = 200, description = "Service is healthy", body = String),
        (status = 500, description = "Internal server error", body = String),
    )
)]
#[get("/health")]
async fn health() -> Result<HttpResponse, actix_web::Error> {
    Ok(HttpResponse::Ok().body("OK"))
}

/// Initializes the health check service.
///
/// Registers the `health` endpoint with the provided service configuration.
pub fn init(cfg: &mut web::ServiceConfig) {
    cfg.service(health);
}
