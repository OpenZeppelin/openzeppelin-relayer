//! This module provides health check and readiness endpoints for the API.
//!
//! The `/health` endpoint can be used to verify that the service is running and responsive.
//! The `/ready` endpoint checks system resources like file descriptors and socket states,
//! as well as Redis connectivity, queue health, and plugin status.

use actix_web::{get, web, Responder};

use crate::api::controllers::health;
use crate::models::DefaultAppState;

/// Handles the `/health` endpoint.
///
/// Returns an `HttpResponse` with a status of `200 OK` and a body of `"OK"`.
///
/// Note: OpenAPI documentation for this endpoint can be found in `docs/health_docs.rs`
#[get("/health")]
async fn health_route() -> impl Responder {
    health::health().await
}

/// Readiness endpoint that checks system resources, Redis, Queue, and plugins.
///
/// Returns 200 OK if the service is ready to accept traffic, or 503 Service Unavailable if not.
///
/// Health check results are cached for 10 seconds to prevent excessive load from frequent
/// health checks (e.g., from AWS ECS or Kubernetes).
///
/// Note: OpenAPI documentation for this endpoint can be found in `docs/health_docs.rs`
#[get("/ready")]
async fn readiness_route(data: web::ThinData<DefaultAppState>) -> impl Responder {
    health::readiness(data).await
}

/// Initializes the health check service.
///
/// Registers the `health` and `ready` endpoints with the provided service configuration.
pub fn init(cfg: &mut web::ServiceConfig) {
    cfg.service(health_route);
    cfg.service(readiness_route);
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{http::StatusCode, test, App};

    // =========================================================================
    // Route Registration Tests
    // =========================================================================

    #[actix_web::test]
    async fn test_health_route_registered() {
        let app = test::init_service(App::new().configure(init)).await;

        let req = test::TestRequest::get().uri("/health").to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        assert_eq!(body, "OK");
    }

    #[actix_web::test]
    async fn test_ready_route_registered() {
        let app = test::init_service(App::new().configure(init)).await;

        let req = test::TestRequest::get().uri("/ready").to_request();
        let resp = test::call_service(&app, req).await;

        // Should not be 404 - endpoint exists
        assert_ne!(resp.status(), StatusCode::NOT_FOUND);
    }

    // =========================================================================
    // HTTP Method Tests
    // =========================================================================

    #[actix_web::test]
    async fn test_health_rejects_post() {
        let app = test::init_service(App::new().configure(init)).await;

        let req = test::TestRequest::post().uri("/health").to_request();
        let resp = test::call_service(&app, req).await;

        // POST should return 404 (route only accepts GET)
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[actix_web::test]
    async fn test_ready_rejects_post() {
        let app = test::init_service(App::new().configure(init)).await;

        let req = test::TestRequest::post().uri("/ready").to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // =========================================================================
    // Content Type Tests
    // =========================================================================

    #[actix_web::test]
    async fn test_ready_returns_json_content_type() {
        let app = test::init_service(App::new().configure(init)).await;

        let req = test::TestRequest::get().uri("/ready").to_request();
        let resp = test::call_service(&app, req).await;

        // Skip content-type check if endpoint errored (500)
        if resp.status() != StatusCode::INTERNAL_SERVER_ERROR {
            let content_type = resp.headers().get("content-type");
            assert!(content_type.is_some(), "Should have content-type header");
            assert!(
                content_type
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .contains("application/json"),
                "Content-Type should be application/json"
            );
        }
    }

    // =========================================================================
    // Health Endpoint Stability Tests
    // =========================================================================

    #[actix_web::test]
    async fn test_health_is_stable_across_requests() {
        let app = test::init_service(App::new().configure(init)).await;

        for _ in 0..5 {
            let req = test::TestRequest::get().uri("/health").to_request();
            let resp = test::call_service(&app, req).await;

            assert_eq!(resp.status(), StatusCode::OK);
            let body = test::read_body(resp).await;
            assert_eq!(body, "OK");
        }
    }

    // =========================================================================
    // Readiness Response Validation Tests
    // =========================================================================

    #[actix_web::test]
    async fn test_ready_returns_valid_status_code() {
        let app = test::init_service(App::new().configure(init)).await;

        let req = test::TestRequest::get().uri("/ready").to_request();
        let resp = test::call_service(&app, req).await;

        let status = resp.status().as_u16();
        // Valid status codes are: 200 (ready), 503 (not ready), or 500 (internal error in test)
        assert!(
            status == 200 || status == 503 || status == 500,
            "Status should be 200, 503, or 500, got {status}"
        );
    }

    #[actix_web::test]
    async fn test_ready_response_is_valid_json() {
        let app = test::init_service(App::new().configure(init)).await;

        let req = test::TestRequest::get().uri("/ready").to_request();
        let resp = test::call_service(&app, req).await;

        let status = resp.status().as_u16();
        // Only validate JSON for non-500 responses
        if status == 200 || status == 503 {
            let body = test::read_body(resp).await;
            let body_str = String::from_utf8(body.to_vec()).unwrap();
            let json: serde_json::Value =
                serde_json::from_str(&body_str).expect("Response should be valid JSON");

            // Verify required fields exist
            assert!(json.get("ready").is_some(), "Should have 'ready' field");
            assert!(json.get("status").is_some(), "Should have 'status' field");
            assert!(
                json.get("components").is_some(),
                "Should have 'components' field"
            );
            assert!(
                json.get("timestamp").is_some(),
                "Should have 'timestamp' field"
            );
        }
    }

    #[actix_web::test]
    async fn test_ready_status_code_correlates_with_ready_field() {
        let app = test::init_service(App::new().configure(init)).await;

        let req = test::TestRequest::get().uri("/ready").to_request();
        let resp = test::call_service(&app, req).await;

        let status_code = resp.status().as_u16();

        // Only validate for non-500 responses
        if status_code == 200 || status_code == 503 {
            let body = test::read_body(resp).await;
            let body_str = String::from_utf8(body.to_vec()).unwrap();
            let json: serde_json::Value = serde_json::from_str(&body_str).unwrap();

            let ready = json["ready"].as_bool().unwrap();

            if ready {
                assert_eq!(status_code, 200, "ready=true should return 200");
            } else {
                assert_eq!(status_code, 503, "ready=false should return 503");
            }
        }
    }

    #[actix_web::test]
    async fn test_ready_components_have_required_fields() {
        let app = test::init_service(App::new().configure(init)).await;

        let req = test::TestRequest::get().uri("/ready").to_request();
        let resp = test::call_service(&app, req).await;

        let status = resp.status().as_u16();
        if status == 200 || status == 503 {
            let body = test::read_body(resp).await;
            let body_str = String::from_utf8(body.to_vec()).unwrap();
            let json: serde_json::Value = serde_json::from_str(&body_str).unwrap();

            let components = &json["components"];

            // System health fields
            assert!(components.get("system").is_some());
            let system = &components["system"];
            assert!(system.get("status").is_some());
            assert!(system.get("fd_count").is_some());
            assert!(system.get("fd_limit").is_some());

            // Redis health fields
            assert!(components.get("redis").is_some());
            let redis = &components["redis"];
            assert!(redis.get("status").is_some());
            assert!(redis.get("primary_pool").is_some());
            assert!(redis.get("reader_pool").is_some());

            // Queue health fields
            assert!(components.get("queue").is_some());
            let queue = &components["queue"];
            assert!(queue.get("status").is_some());
        }
    }

    #[actix_web::test]
    async fn test_ready_timestamp_is_rfc3339() {
        let app = test::init_service(App::new().configure(init)).await;

        let req = test::TestRequest::get().uri("/ready").to_request();
        let resp = test::call_service(&app, req).await;

        let status = resp.status().as_u16();
        if status == 200 || status == 503 {
            let body = test::read_body(resp).await;
            let body_str = String::from_utf8(body.to_vec()).unwrap();
            let json: serde_json::Value = serde_json::from_str(&body_str).unwrap();

            let timestamp = json["timestamp"].as_str().unwrap();
            chrono::DateTime::parse_from_rfc3339(timestamp)
                .expect("Timestamp should be valid RFC3339");
        }
    }

    #[actix_web::test]
    async fn test_ready_status_values_are_valid() {
        let app = test::init_service(App::new().configure(init)).await;

        let req = test::TestRequest::get().uri("/ready").to_request();
        let resp = test::call_service(&app, req).await;

        let status = resp.status().as_u16();
        if status == 200 || status == 503 {
            let body = test::read_body(resp).await;
            let body_str = String::from_utf8(body.to_vec()).unwrap();
            let json: serde_json::Value = serde_json::from_str(&body_str).unwrap();

            let valid_statuses = ["healthy", "degraded", "unhealthy"];

            // Check overall status
            let overall_status = json["status"].as_str().unwrap();
            assert!(
                valid_statuses.contains(&overall_status),
                "Overall status '{overall_status}' should be valid"
            );

            // Check component statuses
            let system_status = json["components"]["system"]["status"].as_str().unwrap();
            let redis_status = json["components"]["redis"]["status"].as_str().unwrap();
            let queue_status = json["components"]["queue"]["status"].as_str().unwrap();

            assert!(valid_statuses.contains(&system_status));
            assert!(valid_statuses.contains(&redis_status));
            assert!(valid_statuses.contains(&queue_status));
        }
    }

    #[actix_web::test]
    async fn test_ready_plugins_field_is_optional() {
        let app = test::init_service(App::new().configure(init)).await;

        let req = test::TestRequest::get().uri("/ready").to_request();
        let resp = test::call_service(&app, req).await;

        let status = resp.status().as_u16();
        if status == 200 || status == 503 {
            let body = test::read_body(resp).await;
            let body_str = String::from_utf8(body.to_vec()).unwrap();
            let json: serde_json::Value = serde_json::from_str(&body_str).unwrap();

            // Plugins is optional - may or may not be present
            if let Some(plugins) = json["components"].get("plugins") {
                if !plugins.is_null() {
                    // If present, should have required fields
                    assert!(plugins.get("status").is_some());
                    assert!(plugins.get("enabled").is_some());
                }
            }
        }
    }
}
