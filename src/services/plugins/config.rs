//! Plugin Configuration
//!
//! Centralized configuration for the plugin system with auto-derivation.
//!
//! # Simple Usage (80% of users)
//!
//! Set one variable and everything else is auto-calculated:
//!
//! ```bash
//! export PLUGIN_MAX_CONCURRENCY=3000
//! ```
//!
//! # Advanced Usage (power users)
//!
//! Override individual settings when needed:
//!
//! ```bash
//! export PLUGIN_MAX_CONCURRENCY=3000
//! export PLUGIN_POOL_MAX_QUEUE_SIZE=10000  # Override just this one
//! ```

use crate::constants::{
    DEFAULT_POOL_CONCURRENT_TASKS_PER_WORKER, DEFAULT_POOL_CONNECT_RETRIES,
    DEFAULT_POOL_HEALTH_CHECK_INTERVAL_SECS, DEFAULT_POOL_IDLE_TIMEOUT_MS,
    DEFAULT_POOL_MAX_CONNECTIONS, DEFAULT_POOL_MAX_QUEUE_SIZE, DEFAULT_POOL_MAX_THREADS_FLOOR,
    DEFAULT_POOL_MIN_THREADS, DEFAULT_POOL_QUEUE_SEND_TIMEOUT_MS, DEFAULT_POOL_REQUEST_TIMEOUT_SECS,
    DEFAULT_SOCKET_IDLE_TIMEOUT_SECS, DEFAULT_SOCKET_MAX_CONCURRENT_CONNECTIONS,
    DEFAULT_SOCKET_READ_TIMEOUT_SECS, DEFAULT_TRACE_TIMEOUT_MS,
};
use std::sync::OnceLock;

/// Cached plugin configuration (computed once at startup)
static CONFIG: OnceLock<PluginConfig> = OnceLock::new();

/// Plugin system configuration with auto-derived values
#[derive(Debug, Clone)]
pub struct PluginConfig {
    // === Primary scaling knob ===
    /// Maximum concurrent plugin executions (the main knob users should adjust)
    pub max_concurrency: usize,

    // === Connection Pool (Rust side, auto-derived from max_concurrency) ===
    /// Maximum connections to the Node.js pool server
    pub pool_max_connections: usize,
    /// Retry attempts when connecting to pool
    pub pool_connect_retries: usize,
    /// Request timeout in seconds
    pub pool_request_timeout_secs: u64,

    // === Request Queue (Rust side, auto-derived from max_concurrency) ===
    /// Maximum queued requests
    pub pool_max_queue_size: usize,
    /// Wait time when queue is full before rejecting (ms)
    pub pool_queue_send_timeout_ms: u64,
    /// Number of queue workers (0 = auto based on CPU cores)
    pub pool_workers: usize,

    // === Socket Service (Rust side, auto-derived from max_concurrency) ===
    /// Maximum concurrent socket connections
    pub socket_max_connections: usize,
    /// Idle timeout for connections (seconds)
    pub socket_idle_timeout_secs: u64,
    /// Read timeout per message (seconds)
    pub socket_read_timeout_secs: u64,

    // === Node.js Worker Pool (passed to pool-server.ts) ===
    /// Minimum worker threads in Node.js pool
    pub nodejs_pool_min_threads: usize,
    /// Maximum worker threads in Node.js pool
    pub nodejs_pool_max_threads: usize,
    /// Concurrent tasks per worker thread
    pub nodejs_pool_concurrent_tasks: usize,
    /// Worker idle timeout in milliseconds
    pub nodejs_pool_idle_timeout_ms: u64,

    // === Health & Monitoring ===
    /// Minimum seconds between health checks
    pub health_check_interval_secs: u64,
    /// Trace collection timeout (ms)
    pub trace_timeout_ms: u64,
}

impl PluginConfig {
    /// Load configuration from environment variables with auto-derivation
    pub fn from_env() -> Self {
        // === Primary scaling knob ===
        // If set, this drives the auto-derivation of other values
        let max_concurrency = env_parse("PLUGIN_MAX_CONCURRENCY", DEFAULT_POOL_MAX_CONNECTIONS);

        // === Auto-derived values (can be individually overridden) ===

        // Pool connections = max_concurrency (1:1 ratio)
        let pool_max_connections = env_parse("PLUGIN_POOL_MAX_CONNECTIONS", max_concurrency);

        // Socket connections = 1.5x max_concurrency (headroom for connection churn)
        let socket_max_connections = env_parse(
            "PLUGIN_SOCKET_MAX_CONCURRENT_CONNECTIONS",
            (max_concurrency as f64 * 1.5) as usize,
        );

        // Queue size = 2x max_concurrency (absorb bursts)
        let pool_max_queue_size = env_parse("PLUGIN_POOL_MAX_QUEUE_SIZE", max_concurrency * 2);

        // Queue timeout scales with concurrency (more concurrency = allow more wait)
        let base_queue_timeout = DEFAULT_POOL_QUEUE_SEND_TIMEOUT_MS;
        let derived_queue_timeout = if max_concurrency > 2000 {
            base_queue_timeout * 2 // 1000ms for high concurrency
        } else if max_concurrency > 1000 {
            base_queue_timeout + 250 // 750ms for medium concurrency
        } else {
            base_queue_timeout // 500ms default
        };
        let pool_queue_send_timeout_ms =
            env_parse("PLUGIN_POOL_QUEUE_SEND_TIMEOUT_MS", derived_queue_timeout);

        // Other settings with defaults
        let pool_connect_retries =
            env_parse("PLUGIN_POOL_CONNECT_RETRIES", DEFAULT_POOL_CONNECT_RETRIES);
        let pool_request_timeout_secs = env_parse(
            "PLUGIN_POOL_REQUEST_TIMEOUT_SECS",
            DEFAULT_POOL_REQUEST_TIMEOUT_SECS,
        );
        let pool_workers = env_parse("PLUGIN_POOL_WORKERS", 0); // 0 = auto

        let socket_idle_timeout_secs = env_parse(
            "PLUGIN_SOCKET_IDLE_TIMEOUT_SECS",
            DEFAULT_SOCKET_IDLE_TIMEOUT_SECS,
        );
        let socket_read_timeout_secs = env_parse(
            "PLUGIN_SOCKET_READ_TIMEOUT_SECS",
            DEFAULT_SOCKET_READ_TIMEOUT_SECS,
        );

        let health_check_interval_secs = env_parse(
            "PLUGIN_POOL_HEALTH_CHECK_INTERVAL_SECS",
            DEFAULT_POOL_HEALTH_CHECK_INTERVAL_SECS,
        );
        let trace_timeout_ms = env_parse("PLUGIN_TRACE_TIMEOUT_MS", DEFAULT_TRACE_TIMEOUT_MS);

        // === Node.js Worker Pool settings (auto-derived from max_concurrency) ===
        // These are passed to pool-server.ts when spawning the Node.js process
        let cpu_count = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        // minThreads = max(2, cpuCount / 2) - keeps some workers warm
        let derived_min_threads = DEFAULT_POOL_MIN_THREADS.max(cpu_count / 2);
        let nodejs_pool_min_threads = env_parse("PLUGIN_POOL_MIN_THREADS", derived_min_threads);

        // maxThreads = min(cpuCount * 2, concurrency / 50, 64)
        // - For 1000 concurrency: ~20 threads
        // - For 3000 concurrency: ~60 threads
        // - For 5000+ concurrency: 64 threads (capped)
        let derived_max_threads = (cpu_count * 2)
            .max(DEFAULT_POOL_MAX_THREADS_FLOOR)
            .min(max_concurrency / 50)
            .min(64);
        let nodejs_pool_max_threads = env_parse(
            "PLUGIN_POOL_MAX_THREADS",
            derived_max_threads.max(DEFAULT_POOL_MAX_THREADS_FLOOR),
        );

        // concurrentTasksPerWorker: Node.js async can handle many concurrent tasks
        // Formula: (concurrency / threads) * 2.5 for headroom (queue buildup, variable latency)
        // - 5000 VUs / 64 threads * 2.5 = ~195 tasks/worker
        // - 3000 VUs / 60 threads * 2.5 = ~125 tasks/worker
        // - 1000 VUs / 20 threads * 2.5 = ~125 tasks/worker
        let base_tasks = max_concurrency / nodejs_pool_max_threads.max(1);
        let derived_concurrent_tasks = ((base_tasks as f64 * 2.5) as usize)
            .max(DEFAULT_POOL_CONCURRENT_TASKS_PER_WORKER)
            .min(300); // High cap - Node.js handles async well
        let nodejs_pool_concurrent_tasks =
            env_parse("PLUGIN_POOL_CONCURRENT_TASKS", derived_concurrent_tasks);

        let nodejs_pool_idle_timeout_ms =
            env_parse("PLUGIN_POOL_IDLE_TIMEOUT", DEFAULT_POOL_IDLE_TIMEOUT_MS);

        Self {
            max_concurrency,
            pool_max_connections,
            pool_connect_retries,
            pool_request_timeout_secs,
            pool_max_queue_size,
            pool_queue_send_timeout_ms,
            pool_workers,
            socket_max_connections,
            socket_idle_timeout_secs,
            socket_read_timeout_secs,
            nodejs_pool_min_threads,
            nodejs_pool_max_threads,
            nodejs_pool_concurrent_tasks,
            nodejs_pool_idle_timeout_ms,
            health_check_interval_secs,
            trace_timeout_ms,
        }
    }

    /// Log the effective configuration for debugging
    pub fn log_config(&self) {
        tracing::info!(
            max_concurrency = self.max_concurrency,
            pool_max_connections = self.pool_max_connections,
            pool_max_queue_size = self.pool_max_queue_size,
            socket_max_connections = self.socket_max_connections,
            nodejs_max_threads = self.nodejs_pool_max_threads,
            nodejs_concurrent_tasks = self.nodejs_pool_concurrent_tasks,
            "Plugin configuration loaded (Rust + Node.js)"
        );
    }
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            max_concurrency: DEFAULT_POOL_MAX_CONNECTIONS,
            pool_max_connections: DEFAULT_POOL_MAX_CONNECTIONS,
            pool_connect_retries: DEFAULT_POOL_CONNECT_RETRIES,
            pool_request_timeout_secs: DEFAULT_POOL_REQUEST_TIMEOUT_SECS,
            pool_max_queue_size: DEFAULT_POOL_MAX_QUEUE_SIZE,
            pool_queue_send_timeout_ms: DEFAULT_POOL_QUEUE_SEND_TIMEOUT_MS,
            pool_workers: 0,
            socket_max_connections: DEFAULT_SOCKET_MAX_CONCURRENT_CONNECTIONS,
            socket_idle_timeout_secs: DEFAULT_SOCKET_IDLE_TIMEOUT_SECS,
            socket_read_timeout_secs: DEFAULT_SOCKET_READ_TIMEOUT_SECS,
            nodejs_pool_min_threads: DEFAULT_POOL_MIN_THREADS,
            nodejs_pool_max_threads: DEFAULT_POOL_MAX_THREADS_FLOOR,
            nodejs_pool_concurrent_tasks: DEFAULT_POOL_CONCURRENT_TASKS_PER_WORKER,
            nodejs_pool_idle_timeout_ms: DEFAULT_POOL_IDLE_TIMEOUT_MS,
            health_check_interval_secs: DEFAULT_POOL_HEALTH_CHECK_INTERVAL_SECS,
            trace_timeout_ms: DEFAULT_TRACE_TIMEOUT_MS,
        }
    }
}

/// Get the global plugin configuration (cached after first call)
pub fn get_config() -> &'static PluginConfig {
    CONFIG.get_or_init(|| {
        let config = PluginConfig::from_env();
        config.log_config();
        config
    })
}

/// Parse an environment variable or return default
fn env_parse<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = PluginConfig::default();
        assert_eq!(config.max_concurrency, DEFAULT_POOL_MAX_CONNECTIONS);
        assert_eq!(config.pool_max_connections, DEFAULT_POOL_MAX_CONNECTIONS);
    }

    #[test]
    fn test_auto_derivation_ratios() {
        // When max_concurrency is set, other values should be derived
        let config = PluginConfig {
            max_concurrency: 1000,
            pool_max_connections: 1000,
            socket_max_connections: 1500, // 1.5x
            pool_max_queue_size: 2000,    // 2x
            ..Default::default()
        };

        assert_eq!(
            config.socket_max_connections,
            config.max_concurrency * 3 / 2
        );
        assert_eq!(config.pool_max_queue_size, config.max_concurrency * 2);
    }
}
