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
