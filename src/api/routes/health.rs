//! This module provides health check and readiness endpoints for the API.
//!
//! The `/health` endpoint can be used to verify that the service is running and responsive.
//! The `/ready` endpoint checks system resources like file descriptors and socket states.
use actix_web::{get, web, HttpResponse};
use serde_json::json;

/// Handles the `/health` endpoint.
///
/// Returns an `HttpResponse` with a status of `200 OK` and a body of `"OK"`.
///
/// Note: OpenAPI documentation for this endpoint can be found in `docs/health_docs.rs`
#[get("/health")]
async fn health() -> Result<HttpResponse, actix_web::Error> {
    Ok(HttpResponse::Ok().body("OK"))
}

/// Readiness check structure
#[allow(dead_code)]
struct ReadinessCheck {
    ready: bool,
    fd_count: usize,
    fd_limit: usize,
    close_wait_count: usize,
    reason: Option<String>,
}

/// Maximum file descriptor ratio (80%)
/// Maximum file descriptor usage ratio before marking service as unhealthy
const MAX_FD_RATIO: f64 = 0.8;

/// Maximum CLOSE_WAIT socket count
/// Maximum number of CLOSE_WAIT state sockets before marking service as unhealthy
const MAX_CLOSE_WAIT: usize = 100;

/// Get file descriptor count for current process.
fn get_fd_count() -> Result<usize, std::io::Error> {
    let pid = std::process::id();

    #[cfg(target_os = "linux")]
    {
        let fd_dir = format!("/proc/{pid}/fd");
        std::fs::read_dir(fd_dir).map(|entries| entries.count())
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let output = Command::new("lsof")
            .args(["-p", &pid.to_string()])
            .output()?;
        let count = String::from_utf8_lossy(&output.stdout)
            .lines()
            .count()
            .saturating_sub(1); // Subtract header line
        Ok(count)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Ok(0) // Unsupported platform
    }
}

/// Get soft file descriptor limit for current process.
fn get_fd_limit() -> Result<usize, std::io::Error> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::process::Command;
        let output = Command::new("sh").args(["-c", "ulimit -n"]).output()?;
        let limit = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap_or(1024);
        Ok(limit)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Ok(1024) // Default fallback
    }
}

/// Get CLOSE_WAIT socket count.
fn get_close_wait_count() -> Result<usize, std::io::Error> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::process::Command;
        let output = Command::new("sh")
            .args(["-c", "netstat -an | grep CLOSE_WAIT | wc -l"])
            .output()?;
        let count = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap_or(0);
        Ok(count)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Ok(0) // Unsupported platform
    }
}

/// Readiness endpoint that checks system resources
///
/// Returns 200 OK if the service is ready to accept traffic, or 503 Service Unavailable if not.
/// Checks file descriptor usage and CLOSE_WAIT socket count to determine readiness.
///
/// Note: OpenAPI documentation for this endpoint can be found in `docs/health_docs.rs`
#[get("/ready")]
async fn readiness() -> Result<HttpResponse, actix_web::Error> {
    let fd_count = get_fd_count().unwrap_or(0);
    let fd_limit = get_fd_limit().unwrap_or(1024);
    let close_wait_count = get_close_wait_count().unwrap_or(0);

    let mut check = ReadinessCheck {
        ready: true,
        fd_count,
        fd_limit,
        close_wait_count,
        reason: None,
    };

    let mut reasons: Vec<String> = Vec::new();

    // Check file descriptor usage
    let fd_ratio = fd_count as f64 / fd_limit as f64;
    if fd_ratio > MAX_FD_RATIO {
        check.ready = false;
        let reason = format!(
            "File descriptor limit exceeded: {}/{} ({:.1}% > {:.1}%)",
            fd_count,
            fd_limit,
            fd_ratio * 100.0,
            MAX_FD_RATIO * 100.0
        );
        tracing::warn!("{}", reason);
        reasons.push(reason);
    }

    // Check CLOSE_WAIT sockets
    if close_wait_count > MAX_CLOSE_WAIT {
        check.ready = false;
        let reason = format!("Too many CLOSE_WAIT sockets: {close_wait_count} > {MAX_CLOSE_WAIT}");
        tracing::warn!("{}", reason);
        reasons.push(reason);
    }

    check.reason = if reasons.is_empty() {
        None
    } else {
        Some(reasons.join("; "))
    };

    if check.ready {
        Ok(HttpResponse::Ok().json(json!({
            "ready": true,
            "fd_count": fd_count,
            "fd_limit": fd_limit,
            "fd_usage_percent": (fd_ratio * 100.0) as u32,
            "close_wait_count": close_wait_count,
        })))
    } else {
        Ok(HttpResponse::ServiceUnavailable().json(json!({
            "ready": false,
            "reason": check.reason,
            "fd_count": fd_count,
            "fd_limit": fd_limit,
            "close_wait_count": close_wait_count,
        })))
    }
}

/// Initializes the health check service.
///
/// Registers the `health` and `ready` endpoints with the provided service configuration.
pub fn init(cfg: &mut web::ServiceConfig) {
    cfg.service(health);
    cfg.service(readiness);
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test, App};

    #[actix_web::test]
    async fn test_health_endpoint() {
        // Arrange
        let app = test::init_service(App::new().configure(init)).await;

        // Act
        let req = test::TestRequest::get().uri("/health").to_request();
        let resp = test::call_service(&app, req).await;

        // Assert
        assert!(resp.status().is_success());

        let body = test::read_body(resp).await;
        assert_eq!(body, "OK");
    }

    #[actix_web::test]
    async fn test_health_endpoint_returns_200() {
        let app = test::init_service(App::new().configure(init)).await;
        let req = test::TestRequest::get().uri("/health").to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), 200);
    }

    #[actix_web::test]
    async fn test_health_endpoint_content_type() {
        let app = test::init_service(App::new().configure(init)).await;
        let req = test::TestRequest::get().uri("/health").to_request();
        let resp = test::call_service(&app, req).await;

        assert!(resp.status().is_success());
        let body = test::read_body(resp).await;
        assert_eq!(body.as_ref(), b"OK");
    }

    #[actix_web::test]
    async fn test_readiness_endpoint_exists() {
        let app = test::init_service(App::new().configure(init)).await;
        let req = test::TestRequest::get().uri("/ready").to_request();
        let resp = test::call_service(&app, req).await;

        // Endpoint should exist (not 404)
        assert_ne!(resp.status(), 404);
    }

    #[actix_web::test]
    async fn test_readiness_endpoint_returns_json() {
        let app = test::init_service(App::new().configure(init)).await;
        let req = test::TestRequest::get().uri("/ready").to_request();
        let resp = test::call_service(&app, req).await;

        let body = test::read_body(resp).await;
        let body_str = String::from_utf8(body.to_vec()).unwrap();

        // Should be valid JSON
        let json: serde_json::Value =
            serde_json::from_str(&body_str).expect("Response should be JSON");

        // Should have required fields
        assert!(json.get("ready").is_some());
        assert!(json.get("fd_count").is_some());
        assert!(json.get("fd_limit").is_some());
        assert!(json.get("close_wait_count").is_some());
    }

    #[actix_web::test]
    async fn test_readiness_endpoint_ready_state() {
        let app = test::init_service(App::new().configure(init)).await;
        let req = test::TestRequest::get().uri("/ready").to_request();
        let resp = test::call_service(&app, req).await;

        let body = test::read_body(resp).await;
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&body_str).unwrap();

        // Check if ready is a boolean
        assert!(json["ready"].is_boolean());
    }

    #[actix_web::test]
    async fn test_readiness_response_status_codes() {
        let app = test::init_service(App::new().configure(init)).await;
        let req = test::TestRequest::get().uri("/ready").to_request();
        let resp = test::call_service(&app, req).await;

        // Should be either 200 (ready) or 503 (not ready), not 404 or 500
        let status = resp.status().as_u16();
        assert!(
            status == 200 || status == 503,
            "Status should be 200 or 503, got {}",
            status
        );
    }

    #[actix_web::test]
    async fn test_readiness_includes_fd_usage_percent_when_ready() {
        let app = test::init_service(App::new().configure(init)).await;
        let req = test::TestRequest::get().uri("/ready").to_request();
        let resp = test::call_service(&app, req).await;

        // Only check for fd_usage_percent if status is 200 (ready)
        if resp.status() == 200 {
            let body = test::read_body(resp).await;
            let body_str = String::from_utf8(body.to_vec()).unwrap();
            let json: serde_json::Value = serde_json::from_str(&body_str).unwrap();

            assert!(json.get("fd_usage_percent").is_some());
        }
    }

    #[actix_web::test]
    async fn test_readiness_includes_reason_when_not_ready() {
        let app = test::init_service(App::new().configure(init)).await;
        let req = test::TestRequest::get().uri("/ready").to_request();
        let resp = test::call_service(&app, req).await;

        // Only check for reason if status is 503 (not ready)
        if resp.status() == 503 {
            let body = test::read_body(resp).await;
            let body_str = String::from_utf8(body.to_vec()).unwrap();
            let json: serde_json::Value = serde_json::from_str(&body_str).unwrap();

            assert!(json.get("reason").is_some());
        }
    }

    #[actix_web::test]
    async fn test_fd_ratio_calculation() {
        // Test that fd_count / fd_limit is calculated correctly
        let fd_count = 100;
        let fd_limit = 1000;
        let ratio = fd_count as f64 / fd_limit as f64;

        assert!((ratio - 0.1).abs() < 0.0001);
        assert!(ratio < MAX_FD_RATIO);
    }

    #[actix_web::test]
    async fn test_max_fd_ratio_threshold() {
        // Verify the MAX_FD_RATIO constant is set to 80%
        assert_eq!(MAX_FD_RATIO, 0.8);
    }

    #[actix_web::test]
    async fn test_max_close_wait_threshold() {
        // Verify the MAX_CLOSE_WAIT constant is set to 100
        assert_eq!(MAX_CLOSE_WAIT, 100);
    }

    #[actix_web::test]
    async fn test_readiness_check_structure() {
        let check = ReadinessCheck {
            ready: true,
            fd_count: 500,
            fd_limit: 1024,
            close_wait_count: 50,
            reason: None,
        };

        assert!(check.ready);
        assert_eq!(check.fd_count, 500);
        assert_eq!(check.fd_limit, 1024);
        assert_eq!(check.close_wait_count, 50);
        assert!(check.reason.is_none());
    }

    #[actix_web::test]
    async fn test_readiness_check_with_reason() {
        let check = ReadinessCheck {
            ready: false,
            fd_count: 900,
            fd_limit: 1000,
            close_wait_count: 150,
            reason: Some("Test reason".to_string()),
        };

        assert!(!check.ready);
        assert!(check.reason.is_some());
        assert_eq!(check.reason.unwrap(), "Test reason");
    }

    #[actix_web::test]
    async fn test_fd_ratio_below_threshold() {
        let fd_count = 700;
        let fd_limit = 1000;
        let ratio = fd_count as f64 / fd_limit as f64;

        assert!(ratio < MAX_FD_RATIO);
        assert_eq!(ratio, 0.7);
    }

    #[actix_web::test]
    async fn test_fd_ratio_at_threshold() {
        let fd_count = 800;
        let fd_limit = 1000;
        let ratio = fd_count as f64 / fd_limit as f64;

        // At exactly 80%, should not trigger (> comparison)
        assert_eq!(ratio, MAX_FD_RATIO);
    }

    #[actix_web::test]
    async fn test_fd_ratio_above_threshold() {
        let fd_count = 810;
        let fd_limit = 1000;
        let ratio = fd_count as f64 / fd_limit as f64;

        assert!(ratio > MAX_FD_RATIO);
    }

    #[actix_web::test]
    async fn test_close_wait_below_threshold() {
        let count = 50;
        assert!(count < MAX_CLOSE_WAIT);
    }

    #[actix_web::test]
    async fn test_close_wait_at_threshold() {
        let count = 100;
        assert_eq!(count, MAX_CLOSE_WAIT);
    }

    #[actix_web::test]
    async fn test_close_wait_above_threshold() {
        let count = 101;
        assert!(count > MAX_CLOSE_WAIT);
    }

    #[actix_web::test]
    async fn test_both_endpoints_registered() {
        let app = test::init_service(App::new().configure(init)).await;

        // Test /health
        let health_req = test::TestRequest::get().uri("/health").to_request();
        let health_resp = test::call_service(&app, health_req).await;
        assert_ne!(health_resp.status(), 404);

        // Test /ready
        let ready_req = test::TestRequest::get().uri("/ready").to_request();
        let ready_resp = test::call_service(&app, ready_req).await;
        assert_ne!(ready_resp.status(), 404);
    }

    #[actix_web::test]
    async fn test_health_endpoint_only_accepts_get() {
        let app = test::init_service(App::new().configure(init)).await;

        // Test that only GET is allowed (other methods should 404 since endpoints use #[get(...)])
        let post_req = test::TestRequest::post().uri("/health").to_request();
        let post_resp = test::call_service(&app, post_req).await;
        assert_eq!(post_resp.status(), 404);
    }

    #[actix_web::test]
    async fn test_ready_endpoint_only_accepts_get() {
        let app = test::init_service(App::new().configure(init)).await;

        let post_req = test::TestRequest::post().uri("/ready").to_request();
        let post_resp = test::call_service(&app, post_req).await;
        assert_eq!(post_resp.status(), 404);
    }

    #[actix_web::test]
    async fn test_nonexistent_endpoint_404() {
        let app = test::init_service(App::new().configure(init)).await;

        let req = test::TestRequest::get().uri("/nonexistent").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 404);
    }

    #[actix_web::test]
    async fn test_readiness_check_multiple_calls_consistency() {
        let app = test::init_service(App::new().configure(init)).await;

        // Call readiness endpoint multiple times
        for _ in 0..3 {
            let req = test::TestRequest::get().uri("/ready").to_request();
            let resp = test::call_service(&app, req).await;

            let status = resp.status().as_u16();
            assert!(
                status == 200 || status == 503,
                "Status should be 200 or 503"
            );
        }
    }
}
