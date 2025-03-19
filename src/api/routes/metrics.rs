//! This module provides HTTP endpoints for interacting with system metrics.
//!
//! # Endpoints
//!
//! - `/metrics`: Returns a list of all available metric names in JSON format.
//! - `/metrics/{metric_name}`: Returns the details of a specific metric in plain text format.
//! - `/debug/metrics/scrape`: Triggers an update of system metrics and returns the result in plain
//!   text format.
//!
//! # Usage
//!
//! These endpoints are designed to be used with a Prometheus server to scrape and monitor system
//! metrics.

use crate::metrics::{update_system_metrics, REGISTRY};
use actix_web::{get, web, HttpResponse, Responder};
use prometheus::{Encoder, TextEncoder};

/// Returns a list of all available metric names in JSON format.
///
/// # Returns
///
/// An `HttpResponse` containing a JSON array of metric names.
#[utoipa::path(
    get,
    path = "/metrics",
    tag = "Metrics",
    responses(
        (status = 200, description = "Metric names list", body = Vec<String>),
        (status = 401, description = "Unauthorized"),
    )
)]
#[get("/metrics")]
async fn list_metrics() -> impl Responder {
    // Gather the metric families from the registry and extract metric names.
    let metric_families = REGISTRY.gather();
    let metric_names: Vec<String> = metric_families
        .iter()
        .map(|mf| mf.get_name().to_string())
        .collect();
    HttpResponse::Ok().json(metric_names)
}

/// Returns the details of a specific metric in plain text format.
///
/// # Parameters
///
/// - `path`: The name of the metric to retrieve details for.
///
/// # Returns
///
/// An `HttpResponse` containing the metric details in plain text, or a 404 error if the metric is
/// not found.
#[utoipa::path(
    get,
    path = "/metrics/{metric_name}",
    tag = "Metrics",
    params(
        ("metric_name" = String, Path, description = "Name of the metric to retrieve, e.g. utopia_transactions_total")
    ),
    responses(
        (status = 200, description = "Metric details in Prometheus text format", content_type = "text/plain", body = String),
        (status = 401, description = "Unauthorized - missing or invalid API key"),
        (status = 403, description = "Forbidden - insufficient permissions to access this metric"),
        (status = 404, description = "Metric not found"),
        (status = 429, description = "Too many requests - rate limit for metrics access exceeded")
    ),
    security(
        ("bearer_auth" = ["metrics:read"])
    )
)]
#[get("/metrics/{metric_name}")]
async fn metric_detail(path: web::Path<String>) -> impl Responder {
    let metric_name = path.into_inner();
    let metric_families = REGISTRY.gather();

    for mf in metric_families {
        if mf.get_name() == metric_name {
            let encoder = TextEncoder::new();
            let mut buffer = Vec::new();
            if let Err(e) = encoder.encode(&[mf], &mut buffer) {
                return HttpResponse::InternalServerError().body(format!("Encoding error: {}", e));
            }
            return HttpResponse::Ok()
                .content_type(encoder.format_type())
                .body(buffer);
        }
    }
    HttpResponse::NotFound().body("Metric not found")
}

/// Triggers an update of system metrics and returns the result in plain text format.
///
/// # Returns
///
/// An `HttpResponse` containing the updated metrics in plain text, or an error message if the
/// update fails.
#[utoipa::path(
    get,
    path = "/debug/metrics/scrape",
    tag = "Metrics",
    responses(
        (status = 200, description = "Complete metrics in Prometheus exposition format", content_type = "text/plain",   body = String),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/debug/metrics/scrape")]
async fn scrape_metrics() -> impl Responder {
    update_system_metrics();
    match crate::metrics::gather_metrics() {
        Ok(body) => HttpResponse::Ok().content_type("text/plain;").body(body),
        Err(e) => HttpResponse::InternalServerError().body(format!("Error: {}", e)),
    }
}

/// Initializes the HTTP services for the metrics module.
///
/// # Parameters
///
/// - `cfg`: The service configuration to which the metrics services will be added.
pub fn init(cfg: &mut web::ServiceConfig) {
    cfg.service(list_metrics);
    cfg.service(metric_detail);
    cfg.service(scrape_metrics);
}
