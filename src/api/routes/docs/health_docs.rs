//! # Health Documentation
//!
//! This module contains the OpenAPI documentation for the health check API endpoints.
//!
//! ## Endpoints
//!
//! - `GET /api/v1/health`: Basic health check endpoint (liveness probe)
//! - `GET /api/v1/ready`: Readiness check endpoint with comprehensive health status

use crate::models::health::ReadinessResponse;

/// Health routes implementation
///
/// Note: OpenAPI documentation for these endpoints can be found in the `openapi.rs` file
///
/// Handles the `/health` endpoint.
///
/// Returns an `HttpResponse` with a status of `200 OK` and a body of `"OK"`.
/// This endpoint is used for liveness probes in container orchestration platforms.
#[utoipa::path(
    get,
    path = "/api/v1/health",
    tag = "Health",
    operation_id = "health",
    responses(
        (status = 200, description = "Service is alive", body = String, example = json!("OK")),
        (status = 500, description = "Internal server error", body = String),
    )
)]
#[allow(dead_code)]
fn doc_health() {}

/// Readiness endpoint that checks system resources, Redis, Queue, and plugins.
///
/// Returns 200 OK if the service is ready to accept traffic, or 503 Service Unavailable if not.
/// This endpoint is used for readiness probes in container orchestration platforms like
/// AWS ECS or Kubernetes.
///
/// ## Health Check Components
///
/// - **System**: File descriptor usage, CLOSE_WAIT socket count
/// - **Redis**: Primary and reader pool connectivity
/// - **Queue**: Queue's Redis connections (separate from app's Redis)
/// - **Plugins**: Plugin pool health, circuit breaker state, and connection metrics (if enabled)
///
/// ## Status Levels
///
/// - `healthy`: All components operational
/// - `degraded`: Some components degraded but service can function (e.g., reader pool down)
/// - `unhealthy`: Critical components failed, service unavailable
///
/// ## Plugin Connection Metrics
///
/// When plugins are enabled, the following connection metrics are exposed:
///
/// - `shared_socket_available_slots`: Number of additional concurrent plugin executions that can start
/// - `shared_socket_active_connections`: Current number of active plugin execution connections
/// - `shared_socket_registered_executions`: Number of plugin executions currently registered (awaiting response)
/// - `connection_pool_available_slots`: Available connections to the pool server
/// - `connection_pool_active_connections`: Active connections to the pool server
///
/// These metrics help diagnose connection pool exhaustion and plugin capacity issues.
///
/// ## Caching
///
/// Health check results are cached for 10 seconds to prevent excessive load from frequent
/// health checks. Multiple requests within the TTL return the same cached response.
#[utoipa::path(
    get,
    path = "/api/v1/ready",
    tag = "Health",
    operation_id = "readiness",
    responses(
        (
            status = 200,
            description = "Service is ready (healthy or degraded)",
            body = ReadinessResponse,
            example = json!({
                "ready": true,
                "status": "healthy",
                "components": {
                    "system": {
                        "status": "healthy",
                        "fd_count": 42,
                        "fd_limit": 1024,
                        "fd_usage_percent": 4,
                        "close_wait_count": 0
                    },
                    "redis": {
                        "status": "healthy",
                        "primary_pool": {
                            "connected": true,
                            "available": 8,
                            "max_size": 16
                        },
                        "reader_pool": {
                            "connected": true,
                            "available": 8,
                            "max_size": 16
                        }
                    },
                    "queue": {
                        "status": "healthy"
                    },
                    "plugins": {
                        "status": "healthy",
                        "enabled": true,
                        "circuit_state": "closed",
                        "uptime_ms": 3600000,
                        "memory": 52428800,
                        "pool_completed": 1000,
                        "pool_queued": 2,
                        "success_rate": 99.5,
                        "avg_response_time_ms": 120,
                        "recovering": false,
                        "shared_socket_available_slots": 48,
                        "shared_socket_active_connections": 2,
                        "shared_socket_registered_executions": 2,
                        "connection_pool_available_slots": 8,
                        "connection_pool_active_connections": 2
                    }
                },
                "timestamp": "2026-01-30T12:00:00Z"
            })
        ),
        (
            status = 503,
            description = "Service is not ready (unhealthy)",
            body = ReadinessResponse,
            example = json!({
                "ready": false,
                "status": "unhealthy",
                "reason": "Redis primary pool: PING timed out",
                "components": {
                    "system": {
                        "status": "healthy",
                        "fd_count": 42,
                        "fd_limit": 1024,
                        "fd_usage_percent": 4,
                        "close_wait_count": 0
                    },
                    "redis": {
                        "status": "unhealthy",
                        "primary_pool": {
                            "connected": false,
                            "available": 0,
                            "max_size": 16,
                            "error": "PING timed out"
                        },
                        "reader_pool": {
                            "connected": false,
                            "available": 0,
                            "max_size": 16,
                            "error": "PING timed out"
                        },
                        "error": "Redis primary pool: PING timed out"
                    },
                    "queue": {
                        "status": "unhealthy",
                        "error": "Queue connection: Stats check timed out"
                    },
                    "plugins": {
                        "status": "degraded",
                        "enabled": true,
                        "circuit_state": "open",
                        "error": "Plugin pool health check failed",
                        "uptime_ms": 3600000,
                        "memory": 52428800,
                        "pool_completed": 1000,
                        "pool_queued": 5,
                        "success_rate": 95.5,
                        "avg_response_time_ms": 150,
                        "recovering": true,
                        "recovery_percent": 10,
                        "shared_socket_available_slots": 45,
                        "shared_socket_active_connections": 5,
                        "shared_socket_registered_executions": 5,
                        "connection_pool_available_slots": 6,
                        "connection_pool_active_connections": 4
                    }
                },
                "timestamp": "2026-01-30T12:00:00Z"
            })
        )
    )
)]
#[allow(dead_code)]
fn doc_readiness() {}
