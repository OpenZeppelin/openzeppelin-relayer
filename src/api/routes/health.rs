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
const MAX_FD_RATIO: f64 = 0.8;

/// Maximum CLOSE_WAIT socket count
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

    // Check file descriptor usage
    let fd_ratio = fd_count as f64 / fd_limit as f64;
    if fd_ratio > MAX_FD_RATIO {
        check.ready = false;
        check.reason = Some(format!(
            "File descriptor limit exceeded: {}/{} ({:.1}% > {:.1}%)",
            fd_count,
            fd_limit,
            fd_ratio * 100.0,
            MAX_FD_RATIO * 100.0
        ));
        tracing::warn!("{}", check.reason.as_ref().unwrap());
    }

    // Check CLOSE_WAIT sockets
    if close_wait_count > MAX_CLOSE_WAIT {
        check.ready = false;
        check.reason = Some(format!(
            "Too many CLOSE_WAIT sockets: {close_wait_count} > {MAX_CLOSE_WAIT}"
        ));
        tracing::warn!("{}", check.reason.as_ref().unwrap());
    }

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
}
