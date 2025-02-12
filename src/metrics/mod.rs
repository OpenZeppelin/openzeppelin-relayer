use lazy_static::lazy_static;
use prometheus::{Counter, Encoder, Opts, Registry, TextEncoder};

lazy_static! {
    // Global Prometheus registry.
    pub static ref REGISTRY: Registry = Registry::new();

    // Example counter: total HTTP requests.
    pub static ref REQUEST_COUNTER: Counter = {
        let opts = Opts::new("requests_total", "Total number of HTTP requests");
        let counter = Counter::with_opts(opts).unwrap();
        REGISTRY.register(Box::new(counter.clone())).unwrap();
        counter
    };
}

#[allow(dead_code)]
/// Gather all metrics and encode into the provided format.
pub fn gather_metrics() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let encoder = TextEncoder::new();
    let metric_families = REGISTRY.gather();
    let mut buffer = Vec::new();
    encoder.encode(&metric_families, &mut buffer)?;
    Ok(buffer)
}
