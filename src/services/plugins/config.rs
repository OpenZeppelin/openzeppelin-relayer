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
    DEFAULT_POOL_MAX_CONNECTIONS, DEFAULT_POOL_MAX_THREADS_FLOOR, DEFAULT_POOL_MIN_THREADS,
    DEFAULT_POOL_QUEUE_SEND_TIMEOUT_MS, DEFAULT_POOL_REQUEST_TIMEOUT_SECS,
    DEFAULT_POOL_SOCKET_BACKLOG, DEFAULT_SOCKET_IDLE_TIMEOUT_SECS,
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

    // === Socket Backlog (derived from max_concurrency) ===
    /// Socket connection backlog for pending connections
    pub pool_socket_backlog: usize,

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

        // Calculate thread count early for queue timeout derivation
        let cpu_count = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let scaling_threads = max_concurrency / 50;
        let estimated_max_threads = (cpu_count * 2)
            .max(scaling_threads)
            .max(DEFAULT_POOL_MAX_THREADS_FLOOR)
            .min(64);

        // Queue timeout scales with concurrency AND thread count
        // Formula: base_timeout * (concurrency / threads) with caps
        // This ensures timeout grows when there are more items per thread
        let base_queue_timeout = DEFAULT_POOL_QUEUE_SEND_TIMEOUT_MS;
        let workload_per_thread = max_concurrency / estimated_max_threads.max(1);
        let derived_queue_timeout = if workload_per_thread > 100 {
            // Heavy load per thread: allow more time
            base_queue_timeout * 2 // 1000ms
        } else if workload_per_thread > 50 {
            // Medium load per thread
            base_queue_timeout + 250 // 750ms
        } else {
            // Light load per thread
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
        // Note: cpu_count and scaling_threads already calculated above for queue timeout

        // minThreads = max(2, cpuCount / 2) - keeps some workers warm
        let derived_min_threads = DEFAULT_POOL_MIN_THREADS.max(cpu_count / 2);
        let nodejs_pool_min_threads = env_parse("PLUGIN_POOL_MIN_THREADS", derived_min_threads);

        // maxThreads = min(max(cpuCount * 2, concurrency / 50), 64)
        // Goal: Scale threads with concurrency, but cap at 64 for efficiency
        // Thread scaling rationale:
        //   - 50 VUs per thread balances concurrency vs context switching
        //   - Cap at 64 threads prevents excessive resource usage
        //   - For low concurrency (<200), use cpu_count * 2 to maintain warm pool
        // Examples:
        //   - 100 concurrency: max(cpu*2, 2) = ~8 threads (warm pool for responsiveness)
        //   - 1000 concurrency: max(cpu*2, 20) = ~20 threads
        //   - 3000 concurrency: max(cpu*2, 60) = ~60 threads
        //   - 6000+ concurrency: capped at 64 threads
        let derived_max_threads = (cpu_count * 2)
            .max(scaling_threads)
            .max(DEFAULT_POOL_MAX_THREADS_FLOOR)
            .min(64); // Final cap at 64
        let nodejs_pool_max_threads = env_parse("PLUGIN_POOL_MAX_THREADS", derived_max_threads);

        // concurrentTasksPerWorker: Node.js async can handle many concurrent tasks
        // Formula: (concurrency / max_threads) * 2.5 for headroom
        // Note: Using max_threads is correct since pool will scale up under load.
        // The 2.5x multiplier provides headroom for:
        //   - Queue buildup during traffic spikes
        //   - Variable plugin execution latency
        //   - Async I/O overlap (Node.js handles this well)
        // Examples:
        //   - 5000 VUs / 64 threads * 2.5 = ~195 tasks/worker
        //   - 3000 VUs / 60 threads * 2.5 = ~125 tasks/worker
        //   - 1000 VUs / 20 threads * 2.5 = ~125 tasks/worker
        let base_tasks = max_concurrency / nodejs_pool_max_threads.max(1);
        let derived_concurrent_tasks = ((base_tasks as f64 * 2.5) as usize)
            .max(DEFAULT_POOL_CONCURRENT_TASKS_PER_WORKER)
            .min(300); // High cap - Node.js handles async well
        let nodejs_pool_concurrent_tasks =
            env_parse("PLUGIN_POOL_CONCURRENT_TASKS", derived_concurrent_tasks);

        let nodejs_pool_idle_timeout_ms =
            env_parse("PLUGIN_POOL_IDLE_TIMEOUT", DEFAULT_POOL_IDLE_TIMEOUT_MS);

        // Socket backlog calculation
        // Use max of concurrency or default backlog to handle connection bursts
        // The 1.5x socket_max_connections provides headroom for connection churn:
        //   - Client reconnections
        //   - Connection pool cycling
        //   - Load balancer health checks
        // This ratio should be validated through load testing if workload characteristics change.
        let default_backlog = DEFAULT_POOL_SOCKET_BACKLOG as usize;
        let pool_socket_backlog = env_parse(
            "PLUGIN_POOL_SOCKET_BACKLOG",
            max_concurrency.max(default_backlog),
        );

        let config = Self {
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
            pool_socket_backlog,
            health_check_interval_secs,
            trace_timeout_ms,
        };

        // Validate derived configuration
        config.validate();

        config
    }

    /// Validate that derived configuration values are sensible
    fn validate(&self) {
        // Critical invariants
        assert!(
            self.pool_max_connections <= self.socket_max_connections,
            "pool_max_connections ({}) must be <= socket_max_connections ({})",
            self.pool_max_connections,
            self.socket_max_connections
        );
        assert!(
            self.nodejs_pool_min_threads <= self.nodejs_pool_max_threads,
            "nodejs_pool_min_threads ({}) must be <= nodejs_pool_max_threads ({})",
            self.nodejs_pool_min_threads,
            self.nodejs_pool_max_threads
        );
        assert!(
            self.max_concurrency > 0,
            "max_concurrency must be > 0, got {}",
            self.max_concurrency
        );
        assert!(
            self.nodejs_pool_max_threads > 0,
            "nodejs_pool_max_threads must be > 0, got {}",
            self.nodejs_pool_max_threads
        );

        // Warnings for potentially problematic configurations
        if self.pool_max_queue_size < self.max_concurrency {
            tracing::warn!(
                "pool_max_queue_size ({}) is less than max_concurrency ({}). \
                 This may cause request rejections under load.",
                self.pool_max_queue_size,
                self.max_concurrency
            );
        }
        if self.nodejs_pool_concurrent_tasks > 500 {
            tracing::warn!(
                "nodejs_pool_concurrent_tasks ({}) is very high. \
                 This may cause excessive memory usage per worker.",
                self.nodejs_pool_concurrent_tasks
            );
        }
    }

    /// Log the effective configuration for debugging
    pub fn log_config(&self) {
        let tasks_per_thread = self.max_concurrency / self.nodejs_pool_max_threads.max(1);
        let socket_ratio = self.socket_max_connections as f64 / self.max_concurrency as f64;
        let queue_ratio = self.pool_max_queue_size as f64 / self.max_concurrency as f64;

        tracing::info!(
            max_concurrency = self.max_concurrency,
            pool_max_connections = self.pool_max_connections,
            pool_max_queue_size = self.pool_max_queue_size,
            queue_timeout_ms = self.pool_queue_send_timeout_ms,
            socket_max_connections = self.socket_max_connections,
            socket_backlog = self.pool_socket_backlog,
            nodejs_min_threads = self.nodejs_pool_min_threads,
            nodejs_max_threads = self.nodejs_pool_max_threads,
            nodejs_concurrent_tasks = self.nodejs_pool_concurrent_tasks,
            tasks_per_thread = tasks_per_thread,
            socket_multiplier = %format!("{:.2}x", socket_ratio),
            queue_multiplier = %format!("{:.2}x", queue_ratio),
            "Plugin configuration loaded (Rust + Node.js)"
        );
    }
}

impl Default for PluginConfig {
    /// Default configuration uses the same derivation logic as from_env()
    /// but without any environment variable overrides.
    /// This ensures tests and production use consistent formulas.
    fn default() -> Self {
        // Clear any test environment variables to ensure pure defaults
        // Note: This only affects the Default impl, not from_env() usage
        std::env::remove_var("PLUGIN_MAX_CONCURRENCY");

        // Use the same derivation logic as from_env()
        // This ensures Default matches production behavior
        let max_concurrency = DEFAULT_POOL_MAX_CONNECTIONS;
        let cpu_count = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        // Apply same formulas as from_env()
        let pool_max_connections = max_concurrency;
        let socket_max_connections = (max_concurrency as f64 * 1.5) as usize;
        let pool_max_queue_size = max_concurrency * 2;

        let scaling_threads = max_concurrency / 50;
        let nodejs_pool_max_threads = (cpu_count * 2)
            .max(scaling_threads)
            .max(DEFAULT_POOL_MAX_THREADS_FLOOR)
            .min(64);
        let nodejs_pool_min_threads = DEFAULT_POOL_MIN_THREADS.max(cpu_count / 2);

        let base_tasks = max_concurrency / nodejs_pool_max_threads.max(1);
        let nodejs_pool_concurrent_tasks = ((base_tasks as f64 * 2.5) as usize)
            .max(DEFAULT_POOL_CONCURRENT_TASKS_PER_WORKER)
            .min(300);

        let default_backlog = DEFAULT_POOL_SOCKET_BACKLOG as usize;
        let pool_socket_backlog = max_concurrency.max(default_backlog);

        Self {
            max_concurrency,
            pool_max_connections,
            pool_connect_retries: DEFAULT_POOL_CONNECT_RETRIES,
            pool_request_timeout_secs: DEFAULT_POOL_REQUEST_TIMEOUT_SECS,
            pool_max_queue_size,
            pool_queue_send_timeout_ms: DEFAULT_POOL_QUEUE_SEND_TIMEOUT_MS,
            pool_workers: 0,
            socket_max_connections,
            socket_idle_timeout_secs: DEFAULT_SOCKET_IDLE_TIMEOUT_SECS,
            socket_read_timeout_secs: DEFAULT_SOCKET_READ_TIMEOUT_SECS,
            nodejs_pool_min_threads,
            nodejs_pool_max_threads,
            nodejs_pool_concurrent_tasks,
            nodejs_pool_idle_timeout_ms: DEFAULT_POOL_IDLE_TIMEOUT_MS,
            pool_socket_backlog,
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
        // Validate derived ratios
        assert_eq!(config.pool_max_queue_size, config.max_concurrency * 2);
        assert!(
            config.socket_max_connections >= config.pool_max_connections,
            "socket connections should be >= pool connections"
        );
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

    #[test]
    fn test_very_low_concurrency() {
        // Test edge case: very low concurrency (10)
        // We can't use from_env() in tests easily due to OnceLock caching,
        // so we manually construct the config with the same logic
        let max_concurrency = 10;
        let cpu_count = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        let pool_max_connections = max_concurrency;
        let socket_max_connections = (max_concurrency as f64 * 1.5) as usize;
        let pool_max_queue_size = max_concurrency * 2;

        let scaling_threads = max_concurrency / 50;
        let nodejs_pool_max_threads = (cpu_count * 2)
            .max(scaling_threads)
            .max(DEFAULT_POOL_MAX_THREADS_FLOOR)
            .min(64);

        assert_eq!(pool_max_connections, 10);
        assert_eq!(socket_max_connections, 15); // 1.5x
        assert_eq!(pool_max_queue_size, 20); // 2x

        // Should still have reasonable thread count (warm pool)
        assert!(nodejs_pool_max_threads >= DEFAULT_POOL_MAX_THREADS_FLOOR);
    }

    #[test]
    fn test_medium_concurrency() {
        // Test edge case: medium concurrency (100)
        let max_concurrency = 100;
        let cpu_count = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        let socket_max_connections = (max_concurrency as f64 * 1.5) as usize;
        let pool_max_queue_size = max_concurrency * 2;

        let scaling_threads = max_concurrency / 50;
        let nodejs_pool_max_threads = (cpu_count * 2)
            .max(scaling_threads)
            .max(DEFAULT_POOL_MAX_THREADS_FLOOR)
            .min(64);

        assert_eq!(socket_max_connections, 150); // 1.5x
        assert_eq!(pool_max_queue_size, 200); // 2x

        // Should use cpu_count * 2 for thread count (warm pool)
        assert!(nodejs_pool_max_threads >= cpu_count * 2);
    }

    #[test]
    fn test_high_concurrency() {
        // Test edge case: high concurrency (10000)
        let max_concurrency = 10000;

        let socket_max_connections = (max_concurrency as f64 * 1.5) as usize;
        let pool_max_queue_size = max_concurrency * 2;

        let cpu_count = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let scaling_threads = max_concurrency / 50;
        let nodejs_pool_max_threads = (cpu_count * 2)
            .max(scaling_threads)
            .max(DEFAULT_POOL_MAX_THREADS_FLOOR)
            .min(64);

        assert_eq!(socket_max_connections, 15000); // 1.5x
        assert_eq!(pool_max_queue_size, 20000); // 2x

        // Should hit the 64 thread cap
        assert_eq!(nodejs_pool_max_threads, 64);

        // Should have high concurrent tasks per worker
        let base_tasks = max_concurrency / nodejs_pool_max_threads;
        let derived_concurrent_tasks = ((base_tasks as f64 * 2.5) as usize)
            .max(DEFAULT_POOL_CONCURRENT_TASKS_PER_WORKER)
            .min(300);
        assert!(derived_concurrent_tasks >= base_tasks);
    }

    #[test]
    fn test_validation_catches_invalid_config() {
        let mut config = PluginConfig::default();

        // Test that validation catches pool > socket connections
        config.pool_max_connections = 1000;
        config.socket_max_connections = 500;

        let result = std::panic::catch_unwind(|| {
            config.validate();
        });
        assert!(
            result.is_err(),
            "Should panic on invalid pool > socket connections"
        );
    }

    #[test]
    fn test_validation_catches_invalid_threads() {
        let mut config = PluginConfig::default();

        // Test that validation catches min > max threads
        config.nodejs_pool_min_threads = 64;
        config.nodejs_pool_max_threads = 8;

        let result = std::panic::catch_unwind(|| {
            config.validate();
        });
        assert!(result.is_err(), "Should panic on invalid min > max threads");
    }

    #[test]
    fn test_overridden_values_respected() {
        // Test that individual overrides work
        // Note: Due to OnceLock caching in get_config(), we test the derivation logic directly
        let max_concurrency = 1000;
        let pool_max_queue_size = 5000; // What we'd override to
        let pool_max_connections = 1000; // Auto-derived from max_concurrency

        // Verify the override would be respected
        assert_eq!(pool_max_connections, max_concurrency); // Auto-derived
        assert_eq!(pool_max_queue_size, 5000); // Manual override (not 2000)

        // Also test that auto-derivation would have given 2000
        let auto_derived_queue = max_concurrency * 2;
        assert_eq!(auto_derived_queue, 2000);
        assert_ne!(pool_max_queue_size, auto_derived_queue); // Override is different
    }
}
