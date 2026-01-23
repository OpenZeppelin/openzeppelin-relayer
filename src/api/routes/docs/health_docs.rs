//! # Health Documentation
//!
//! This module contains the OpenAPI documentation for the health check API endpoints.
//!
//! ## Endpoints
//!
//! - `GET /api/v1/health`: Basic health check endpoint
//! - `GET /api/v1/ready`: Readiness check endpoint that verifies system resources

/// Health routes implementation
///
/// Note: OpenAPI documentation for these endpoints can be found in the `openapi.rs` file
///
/// Handles the `/health` endpoint.
///
/// Returns an `HttpResponse` with a status of `200 OK` and a body of `"OK"`.
#[utoipa::path(
    get,
    path = "/api/v1/health",
    tag = "Health",
    operation_id = "health",
    responses(
        (status = 200, description = "Service is healthy", body = String),
        (status = 500, description = "Internal server error", body = String),
    )
)]
#[allow(dead_code)]
fn doc_health() {}

/// Readiness endpoint that checks system resources
///
/// Returns 200 OK if the service is ready to accept traffic, or 503 Service Unavailable if not.
/// Checks file descriptor usage and CLOSE_WAIT socket count to determine readiness.
#[utoipa::path(
    get,
    path = "/api/v1/ready",
    tag = "Health",
    operation_id = "readiness",
    responses(
        (
            status = 200,
            description = "Service is ready",
            body = Object,
            example = json!({
                "ready": true,
                "fd_count": 42,
                "fd_limit": 1024,
                "fd_usage_percent": 4,
                "close_wait_count": 0
            })
        ),
        (
            status = 503,
            description = "Service is not ready",
            body = Object,
            example = json!({
                "ready": false,
                "reason": "File descriptor limit exceeded: 900/1024 (87.9% > 80.0%)",
                "fd_count": 900,
                "fd_limit": 1024,
                "close_wait_count": 0
            })
        )
    )
)]
#[allow(dead_code)]
fn doc_readiness() {}
