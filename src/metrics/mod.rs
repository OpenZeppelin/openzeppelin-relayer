//! Metrics module for the application.
//!
//! - This module contains the global Prometheus registry.
//! - Defines specific metrics for the application.

pub mod metrics_middleware;
use lazy_static::lazy_static;
use prometheus::{CounterVec, Encoder, Opts, Registry, TextEncoder};

lazy_static! {
    // Global Prometheus registry.
    pub static ref REGISTRY: Registry = Registry::new();

    // Counter: Total HTTP requests.
    pub static ref REQUEST_COUNTER: CounterVec = {
        let opts = Opts::new("requests_total", "Total number of HTTP requests");
        let counter_vec = CounterVec::new(opts, &["endpoint"]).unwrap();
        REGISTRY.register(Box::new(counter_vec.clone())).unwrap();
        counter_vec
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
