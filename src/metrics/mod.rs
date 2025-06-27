//! Metrics module for the application.
//!
//! - This module contains the global Prometheus registry.
//! - Defines specific metrics for the application.

pub mod middleware;
use lazy_static::lazy_static;
use prometheus::{
    CounterVec, Encoder, Gauge, HistogramOpts, HistogramVec, Opts, Registry, TextEncoder,
};
use sysinfo::{Disks, System};

lazy_static! {
    // Global Prometheus registry.
    pub static ref REGISTRY: Registry = Registry::new();

    // Counter: Total HTTP requests.
    pub static ref REQUEST_COUNTER: CounterVec = {
        let opts = Opts::new("requests_total", "Total number of HTTP requests");
        let counter_vec = CounterVec::new(opts, &["endpoint", "method", "status"]).unwrap();
        REGISTRY.register(Box::new(counter_vec.clone())).unwrap();
        counter_vec
    };

    // Counter: Total HTTP requests by raw URI.
    pub static ref RAW_REQUEST_COUNTER: CounterVec = {
      let opts = Opts::new("raw_requests_total", "Total number of HTTP requests by raw URI");
      let counter_vec = CounterVec::new(opts, &["raw_uri", "method", "status"]).unwrap();
      REGISTRY.register(Box::new(counter_vec.clone())).unwrap();
      counter_vec
    };

    // Histogram for request latency in seconds.
    pub static ref REQUEST_LATENCY: HistogramVec = {
      let histogram_opts = HistogramOpts::new("request_latency_seconds", "Request latency in seconds")
          .buckets(vec![0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0]);
      let histogram_vec = HistogramVec::new(histogram_opts, &["endpoint", "method", "status"]).unwrap();
      REGISTRY.register(Box::new(histogram_vec.clone())).unwrap();
      histogram_vec
    };

    // Counter for error responses.
    pub static ref ERROR_COUNTER: CounterVec = {
        let opts = Opts::new("error_requests_total", "Total number of error responses");
        // Using "status" to record the HTTP status code (or a special label like "service_error")
        let counter_vec = CounterVec::new(opts, &["endpoint", "method", "status"]).unwrap();
        REGISTRY.register(Box::new(counter_vec.clone())).unwrap();
        counter_vec
    };

    // Gauge for CPU usage percentage.
    pub static ref CPU_USAGE: Gauge = {
      let gauge = Gauge::new("cpu_usage_percentage", "Current CPU usage percentage").unwrap();
      REGISTRY.register(Box::new(gauge.clone())).unwrap();
      gauge
    };

    // Gauge for memory usage percentage.
    pub static ref MEMORY_USAGE_PERCENT: Gauge = {
      let gauge = Gauge::new("memory_usage_percentage", "Memory usage percentage").unwrap();
      REGISTRY.register(Box::new(gauge.clone())).unwrap();
      gauge
    };

    // Gauge for memory usage in bytes.
    pub static ref MEMORY_USAGE: Gauge = {
        let gauge = Gauge::new("memory_usage_bytes", "Memory usage in bytes").unwrap();
        REGISTRY.register(Box::new(gauge.clone())).unwrap();
        gauge
    };

    // Gauge for total memory in bytes.
    pub static ref TOTAL_MEMORY: Gauge = {
      let gauge = Gauge::new("total_memory_bytes", "Total memory in bytes").unwrap();
      REGISTRY.register(Box::new(gauge.clone())).unwrap();
      gauge
    };

    // Gauge for available memory in bytes.
    pub static ref AVAILABLE_MEMORY: Gauge = {
        let gauge = Gauge::new("available_memory_bytes", "Available memory in bytes").unwrap();
        REGISTRY.register(Box::new(gauge.clone())).unwrap();
        gauge
    };

    // Gauge for used disk space in bytes.
    pub static ref DISK_USAGE: Gauge = {
      let gauge = Gauge::new("disk_usage_bytes", "Used disk space in bytes").unwrap();
      REGISTRY.register(Box::new(gauge.clone())).unwrap();
      gauge
    };

    // Gauge for disk usage percentage.
    pub static ref DISK_USAGE_PERCENT: Gauge = {
      let gauge = Gauge::new("disk_usage_percentage", "Disk usage percentage").unwrap();
      REGISTRY.register(Box::new(gauge.clone())).unwrap();
      gauge
    };
}

/// Gather all metrics and encode into the provided format.
pub fn gather_metrics() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let encoder = TextEncoder::new();
    let metric_families = REGISTRY.gather();
    let mut buffer = Vec::new();
    encoder.encode(&metric_families, &mut buffer)?;
    Ok(buffer)
}

/// Updates the system metrics for CPU and memory usage.
pub fn update_system_metrics() {
    let mut sys = System::new_all();
    sys.refresh_all();

    // Overall CPU usage.
    let cpu_usage = sys.global_cpu_usage();
    CPU_USAGE.set(cpu_usage as f64);

    // Total memory (in bytes).
    let total_memory = sys.total_memory();
    TOTAL_MEMORY.set(total_memory as f64);

    // Available memory (in bytes).
    let available_memory = sys.available_memory();
    AVAILABLE_MEMORY.set(available_memory as f64);

    // Used memory (in bytes).
    let memory_usage = sys.used_memory();
    MEMORY_USAGE.set(memory_usage as f64);

    // Calculate memory usage percentage
    let memory_percentage = if total_memory > 0 {
        (memory_usage as f64 / total_memory as f64) * 100.0
    } else {
        0.0
    };
    MEMORY_USAGE_PERCENT.set(memory_percentage);

    // Calculate disk usage:
    // Sum total space and available space across all disks.
    let disks = Disks::new_with_refreshed_list();
    let mut total_disk_space: u64 = 0;
    let mut total_disk_available: u64 = 0;
    for disk in disks.list() {
        total_disk_space += disk.total_space();
        total_disk_available += disk.available_space();
    }
    // Used disk space is total minus available ( in bytes).
    let used_disk_space = total_disk_space.saturating_sub(total_disk_available);
    DISK_USAGE.set(used_disk_space as f64);

    // Calculate disk usage percentage.
    let disk_percentage = if total_disk_space > 0 {
        (used_disk_space as f64 / total_disk_space as f64) * 100.0
    } else {
        0.0
    };
    DISK_USAGE_PERCENT.set(disk_percentage);
}

#[cfg(test)]
mod actix_tests {
    use super::*;
    use actix_web::{
        dev::{Service, ServiceRequest, ServiceResponse, Transform},
        http, test, Error, HttpResponse,
    };
    use futures::future::{self};
    use middleware::MetricsMiddleware;
    use prometheus::proto::MetricFamily;
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };

    // Dummy service that always returns a successful response (HTTP 200 OK).
    struct DummySuccessService;

    impl Service<ServiceRequest> for DummySuccessService {
        type Response = ServiceResponse;
        type Error = Error;
        type Future = Pin<Box<dyn future::Future<Output = Result<Self::Response, Self::Error>>>>;

        fn poll_ready(&self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&self, req: ServiceRequest) -> Self::Future {
            let resp = req.into_response(HttpResponse::Ok().finish());
            Box::pin(async move { Ok(resp) })
        }
    }

    // Dummy service that always returns an error.
    struct DummyErrorService;

    impl Service<ServiceRequest> for DummyErrorService {
        type Response = ServiceResponse;
        type Error = Error;
        type Future = Pin<Box<dyn future::Future<Output = Result<Self::Response, Self::Error>>>>;

        fn poll_ready(&self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&self, _req: ServiceRequest) -> Self::Future {
            Box::pin(async move { Err(actix_web::error::ErrorInternalServerError("dummy error")) })
        }
    }

    // Helper function to find a metric family by name.
    fn find_metric_family<'a>(
        name: &str,
        families: &'a [MetricFamily],
    ) -> Option<&'a MetricFamily> {
        families.iter().find(|mf| mf.name() == name)
    }

    #[actix_rt::test]
    async fn test_gather_metrics_contains_expected_names() {
        // Update system metrics
        update_system_metrics();

        // Increment request counters to ensure they appear in output
        REQUEST_COUNTER
            .with_label_values(&["/test", "GET", "200"])
            .inc();
        RAW_REQUEST_COUNTER
            .with_label_values(&["/test?param=value", "GET", "200"])
            .inc();
        REQUEST_LATENCY
            .with_label_values(&["/test", "GET", "200"])
            .observe(0.1);
        ERROR_COUNTER
            .with_label_values(&["/test", "GET", "500"])
            .inc();

        let metrics = gather_metrics().expect("failed to gather metrics");
        let output = String::from_utf8(metrics).expect("metrics output is not valid UTF-8");

        // System metrics
        assert!(output.contains("cpu_usage_percentage"));
        assert!(output.contains("memory_usage_percentage"));
        assert!(output.contains("memory_usage_bytes"));
        assert!(output.contains("total_memory_bytes"));
        assert!(output.contains("available_memory_bytes"));
        assert!(output.contains("disk_usage_bytes"));
        assert!(output.contains("disk_usage_percentage"));

        // Request metrics
        assert!(output.contains("requests_total"));
        assert!(output.contains("raw_requests_total"));
        assert!(output.contains("request_latency_seconds"));
        assert!(output.contains("error_requests_total"));
    }

    #[actix_rt::test]
    async fn test_update_system_metrics() {
        // Reset metrics to ensure clean state
        CPU_USAGE.set(0.0);
        TOTAL_MEMORY.set(0.0);
        AVAILABLE_MEMORY.set(0.0);
        MEMORY_USAGE.set(0.0);
        MEMORY_USAGE_PERCENT.set(0.0);
        DISK_USAGE.set(0.0);
        DISK_USAGE_PERCENT.set(0.0);

        // Call the function we're testing
        update_system_metrics();

        // Verify that metrics have been updated with reasonable values
        let cpu_usage = CPU_USAGE.get();
        assert!(
            (0.0..=100.0).contains(&cpu_usage),
            "CPU usage should be between 0-100%, got {}",
            cpu_usage
        );

        let memory_usage = MEMORY_USAGE.get();
        assert!(
            memory_usage >= 0.0,
            "Memory usage should be >= 0, got {}",
            memory_usage
        );

        let memory_percent = MEMORY_USAGE_PERCENT.get();
        assert!(
            (0.0..=100.0).contains(&memory_percent),
            "Memory usage percentage should be between 0-100%, got {}",
            memory_percent
        );

        let total_memory = TOTAL_MEMORY.get();
        assert!(
            total_memory > 0.0,
            "Total memory should be > 0, got {}",
            total_memory
        );

        let available_memory = AVAILABLE_MEMORY.get();
        assert!(
            available_memory >= 0.0,
            "Available memory should be >= 0, got {}",
            available_memory
        );

        let disk_usage = DISK_USAGE.get();
        assert!(
            disk_usage >= 0.0,
            "Disk usage should be >= 0, got {}",
            disk_usage
        );

        let disk_percent = DISK_USAGE_PERCENT.get();
        assert!(
            (0.0..=100.0).contains(&disk_percent),
            "Disk usage percentage should be between 0-100%, got {}",
            disk_percent
        );

        // Verify that memory usage doesn't exceed total memory
        assert!(
            memory_usage <= total_memory,
            "Memory usage should be <= total memory, got {}",
            memory_usage
        );

        // Verify that available memory plus used memory doesn't exceed total memory
        assert!(
            (available_memory + memory_usage) <= total_memory,
            "Available memory plus used memory should be <= total memory {}, got {}",
            total_memory,
            available_memory + memory_usage
        );
    }

    #[actix_rt::test]
    async fn test_middleware_success() {
        let req = test::TestRequest::with_uri("/test_success").to_srv_request();

        let middleware = MetricsMiddleware;
        let service = middleware.new_transform(DummySuccessService).await.unwrap();

        let resp = service.call(req).await.unwrap();
        assert_eq!(resp.response().status(), http::StatusCode::OK);

        let families = REGISTRY.gather();
        let counter_fam = find_metric_family("requests_total", &families)
            .expect("requests_total metric family not found");

        let mut found = false;
        for m in counter_fam.get_metric() {
            let labels = m.get_label();
            if labels
                .iter()
                .any(|l| l.name() == "endpoint" && l.value() == "/test_success")
            {
                found = true;
                assert!(m.get_counter().value() >= 1.0);
            }
        }
        assert!(
            found,
            "Expected metric with endpoint '/test_success' not found"
        );
    }

    #[actix_rt::test]
    async fn test_middleware_error() {
        let req = test::TestRequest::with_uri("/test_error").to_srv_request();

        let middleware = MetricsMiddleware;
        let service = middleware.new_transform(DummyErrorService).await.unwrap();

        let result = service.call(req).await;
        assert!(result.is_err());

        let families = REGISTRY.gather();
        let error_counter_fam = find_metric_family("error_requests_total", &families)
            .expect("error_requests_total metric family not found");

        let mut found = false;
        for m in error_counter_fam.get_metric() {
            let labels = m.get_label();
            if labels
                .iter()
                .any(|l| l.name() == "endpoint" && l.value() == "/test_error")
            {
                found = true;
                assert!(m.get_counter().value() >= 1.0);
            }
        }
        assert!(
            found,
            "Expected error metric with endpoint '/test_error' not found"
        );
    }
}

#[cfg(test)]
mod property_tests {
    use proptest::{prelude::*, test_runner::Config};

    // A helper function to compute percentage used from total.
    fn compute_percentage(used: u64, total: u64) -> f64 {
        if total > 0 {
            (used as f64 / total as f64) * 100.0
        } else {
            0.0
        }
    }

    proptest! {
        // Set the number of cases to 1000
        #![proptest_config(Config {
          cases: 1000, ..Config::default()
        })]

        #[test]
        fn prop_compute_percentage((total, used) in {
            (1u64..1_000_000u64).prop_flat_map(|total| {
                (Just(total), 0u64..=total)
            })
        }) {
            let percentage = compute_percentage(used, total);
            prop_assert!(percentage >= 0.0);
            prop_assert!(percentage <= 100.0);
        }

        #[test]
        fn prop_labels_are_reasonable(
              endpoint in ".*",
              method in prop::sample::select(vec![
                "GET".to_string(),
                "POST".to_string(),
                "PUT".to_string(),
                "DELETE".to_string()
                ])
            ) {
            let endpoint_label = if endpoint.is_empty() { "/".to_string() } else { endpoint.clone() };
            let method_label = method;

            prop_assert!(endpoint_label.chars().count() <= 1024, "Endpoint label too long");
            prop_assert!(method_label.chars().count() <= 16, "Method label too long");

            let status = "200".to_string();
            let labels = vec![endpoint_label, method_label, status];

            for label in labels {
                prop_assert!(!label.is_empty());
                prop_assert!(label.len() < 1024);
            }
        }
    }
}
