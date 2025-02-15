//! Metrics module for the application.
//!
//! - This module contains the global Prometheus registry.
//! - Defines specific metrics for the application.

pub mod middleware;
use lazy_static::lazy_static;
use prometheus::{
    CounterVec, Encoder, Gauge, HistogramOpts, HistogramVec, Opts, Registry, TextEncoder,
};
use sysinfo::System;

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

    // Get overall CPU usage.
    let cpu_usage = sys.global_cpu_usage();
    CPU_USAGE.set(cpu_usage as f64);

    // Get total memory (in bytes).
    let total_memory = sys.total_memory() * 1024; // sysinfo returns kilobytes
    TOTAL_MEMORY.set(total_memory as f64);

    // Get available memory (in bytes).
    let available_memory = sys.available_memory() * 1024; // sysinfo returns kilobytes
    AVAILABLE_MEMORY.set(available_memory as f64);

    // Get used memory (in bytes).
    let memory_usage = sys.used_memory() * 1024; // sysinfo returns kilobytes
    MEMORY_USAGE.set(memory_usage as f64);
}
