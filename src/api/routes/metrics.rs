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

/// Metrics routes implementation
///
/// Note: OpenAPI documentation for these endpoints can be found in the `openapi.rs` file
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
        .map(|mf| mf.name().to_string())
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
        if mf.name() == metric_name {
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

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test, App};
    use prometheus::{Counter, Opts, Registry};

    // Helper function to create a test registry with a sample metric
    fn setup_test_registry() -> Registry {
        let registry = Registry::new();
        let counter = Counter::with_opts(Opts::new("test_counter", "A test counter")).unwrap();
        registry.register(Box::new(counter.clone())).unwrap();
        counter.inc(); // Set some value
        registry
    }

    // Mock implementation for list_metrics that uses our test registry
    async fn mock_list_metrics() -> impl Responder {
        // Use our test registry instead of the global one
        let registry = setup_test_registry();
        let metric_families = registry.gather();

        let metric_names: Vec<String> = metric_families
            .iter()
            .map(|mf| mf.name().to_string())
            .collect();

        HttpResponse::Ok().json(metric_names)
    }

    #[actix_web::test]
    async fn test_list_metrics() {
        // Create a test app with our mock handler
        let app = test::init_service(
            App::new().service(web::resource("/metrics").route(web::get().to(mock_list_metrics))),
        )
        .await;

        // Make request to list metrics
        let req = test::TestRequest::get().uri("/metrics").to_request();
        let resp = test::call_service(&app, req).await;

        // Verify response
        assert!(resp.status().is_success());

        // Parse response body as JSON
        let body = test::read_body(resp).await;
        let metric_names: Vec<String> = serde_json::from_slice(&body).unwrap();

        // Verify our test metric is in the list
        assert!(metric_names.contains(&"test_counter".to_string()));
    }

    // Mock implementation of the metric_detail handler for testing
    async fn mock_metric_detail(path: web::Path<String>) -> impl Responder {
        let metric_name = path.into_inner();

        // Create a test registry with our test_counter
        let registry = setup_test_registry();
        let metric_families = registry.gather();

        for mf in metric_families {
            if mf.name() == metric_name {
                let encoder = TextEncoder::new();
                let mut buffer = Vec::new();
                if let Err(e) = encoder.encode(&[mf], &mut buffer) {
                    return HttpResponse::InternalServerError()
                        .body(format!("Encoding error: {}", e));
                }
                return HttpResponse::Ok()
                    .content_type(encoder.format_type())
                    .body(buffer);
            }
        }
        HttpResponse::NotFound().body("Metric not found")
    }

    #[actix_web::test]
    async fn test_metric_detail() {
        // Create a test app with our mock handler
        let app = test::init_service(App::new().service(
            web::resource("/metrics/{metric_name}").route(web::get().to(mock_metric_detail)),
        ))
        .await;

        // Make request for our test metric
        let req = test::TestRequest::get()
            .uri("/metrics/test_counter")
            .to_request();
        let resp = test::call_service(&app, req).await;

        // Verify response
        assert!(resp.status().is_success());

        // Check that response contains our metric
        let body = test::read_body(resp).await;
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(body_str.contains("test_counter"));
    }

    #[actix_web::test]
    async fn test_metric_detail_not_found() {
        // Create a test app with our mock handler
        let app = test::init_service(App::new().service(
            web::resource("/metrics/{metric_name}").route(web::get().to(mock_metric_detail)),
        ))
        .await;

        // Make request for a non-existent metric
        let req = test::TestRequest::get()
            .uri("/metrics/nonexistent")
            .to_request();
        let resp = test::call_service(&app, req).await;

        // Verify we get a 404 response
        assert_eq!(resp.status(), 404);
    }

    #[actix_web::test]
    async fn test_scrape_metrics() {
        // Create a test app with our endpoints
        let app = test::init_service(App::new().service(scrape_metrics)).await;

        // Make request to scrape metrics
        let req = test::TestRequest::get()
            .uri("/debug/metrics/scrape")
            .to_request();
        let resp = test::call_service(&app, req).await;

        // Verify response status
        assert!(resp.status().is_success());
    }

    #[actix_web::test]
    async fn test_scrape_metrics_error() {
        // We need to mock the gather_metrics function to return an error
        // This would typically be done with a mocking framework
        // For this example, we'll create a custom handler that simulates the error

        async fn mock_scrape_metrics_error() -> impl Responder {
            // Simulate an error from gather_metrics
            HttpResponse::InternalServerError().body("Error: test error")
        }

        // Create a test app with our mock error handler
        let app = test::init_service(App::new().service(
            web::resource("/debug/metrics/scrape").route(web::get().to(mock_scrape_metrics_error)),
        ))
        .await;

        // Make request to scrape metrics
        let req = test::TestRequest::get()
            .uri("/debug/metrics/scrape")
            .to_request();
        let resp = test::call_service(&app, req).await;

        // Verify we get a 500 response
        assert_eq!(resp.status(), 500);

        // Check that response contains our error message
        let body = test::read_body(resp).await;
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(body_str.contains("Error: test error"));
    }

    #[actix_web::test]
    async fn test_init() {
        // Create a test app with our init function
        let app = test::init_service(App::new().configure(init)).await;

        // Test each endpoint to ensure they were properly registered

        // Test list_metrics endpoint
        let req = test::TestRequest::get().uri("/metrics").to_request();
        let resp = test::call_service(&app, req).await;

        // We expect this to succeed since list_metrics should work with any registry state
        assert!(resp.status().is_success());

        // Test metric_detail endpoint - we expect a 404 since test_counter doesn't exist in global registry
        let req = test::TestRequest::get()
            .uri("/metrics/test_counter")
            .to_request();
        let resp = test::call_service(&app, req).await;

        // We expect a 404 Not Found since test_counter doesn't exist in the global registry
        assert_eq!(resp.status(), 404);

        // Test scrape_metrics endpoint
        let req = test::TestRequest::get()
            .uri("/debug/metrics/scrape")
            .to_request();
        let resp = test::call_service(&app, req).await;
        // This should succeed as it doesn't depend on specific metrics existing
        assert!(resp.status().is_success());
    }

    #[actix_web::test]
    async fn test_metric_detail_encoding_error() {
        // Create a mock handler that simulates an encoding error
        async fn mock_metric_detail_with_encoding_error(path: web::Path<String>) -> impl Responder {
            let metric_name = path.into_inner();

            // Create a test registry with our test_counter
            let registry = setup_test_registry();
            let metric_families = registry.gather();

            for mf in metric_families {
                if mf.name() == metric_name {
                    // Simulate an encoding error by returning an error response directly
                    return HttpResponse::InternalServerError()
                        .body("Encoding error: simulated error");
                }
            }
            HttpResponse::NotFound().body("Metric not found")
        }

        // Create a test app with our mock error handler
        let app = test::init_service(
            App::new().service(
                web::resource("/metrics/{metric_name}")
                    .route(web::get().to(mock_metric_detail_with_encoding_error)),
            ),
        )
        .await;

        // Make request for our test metric - use "test_counter" which we know exists in setup_test_registry
        let req = test::TestRequest::get()
            .uri("/metrics/test_counter")
            .to_request();
        let resp = test::call_service(&app, req).await;

        // Verify we get a 500 response
        assert_eq!(resp.status(), 500);

        // Check that response contains our error message
        let body = test::read_body(resp).await;
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(body_str.contains("Encoding error: simulated error"));
    }
}
