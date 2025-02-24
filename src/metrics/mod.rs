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
mod tests {
    use super::*;
    use proptest::prelude::*;

    // Call update_system_metrics function
    #[test]
    fn test_gather_metrics_contains_expected_names() {
        // Update the metrics with the current system values.
        update_system_metrics();
        let metrics = gather_metrics().expect("failed to gather metrics");
        let output = String::from_utf8(metrics).expect("metrics output is not valid UTF-8");

        // Check that some of our defined metric names are present.
        assert!(output.contains("cpu_usage_percentage"));
        assert!(output.contains("total_memory_bytes"));
        assert!(output.contains("disk_usage_bytes"));
    }

    // A helper function to compute percentage used from total.
    // This is used in the property-based test below.
    fn compute_percentage(used: u64, total: u64) -> f64 {
        if total > 0 {
            (used as f64 / total as f64) * 100.0
        } else {
            0.0
        }
    }

    // Property-based test:
    // We generate a total (at least 1) and a used value such that used <= total,
    // and check that the computed percentage is between 0 and 100.
    proptest! {
        #[test]
        fn prop_compute_percentage((total, used) in {
            // Generate a total between 1 and 1_000_000 and a used value between 0 and total.
            (1u64..1_000_000u64).prop_flat_map(|total| {
                (Just(total), 0u64..=total)
            })
        }) {
            let percentage = compute_percentage(used, total);
            // For valid memory metrics, the percentage should be in the [0, 100] range.
            prop_assert!(percentage >= 0.0);
            prop_assert!(percentage <= 100.0);
        }
    }
}
