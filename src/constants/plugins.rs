// =============================================================================
// Plugin Configuration Constants
// =============================================================================
//
// All constants below can be overridden via environment variables.
// See docs/plugins/index.mdx for the full configuration guide.
//
// Environment Variable Naming Convention:
//   PLUGIN_POOL_*  → Pool server & connection pool settings
//   PLUGIN_SOCKET_* → Shared socket service settings
//   PLUGIN_TRACE_*  → Trace collection settings
//
// =============================================================================

/// Default plugin execution timeout in seconds.
/// Override in config.json per-plugin: `"timeout": 60`
pub const DEFAULT_PLUGIN_TIMEOUT_SECONDS: u64 = 300; // 5 minutes

// =============================================================================
// Plugin Pool Server Configuration
// These constants are the source of truth. The TypeScript pool-server.ts and
// worker-pool.ts files should use matching values.
// =============================================================================

/// Maximum concurrent connections from Rust to the Node.js pool server.
/// Env: PLUGIN_POOL_MAX_CONNECTIONS
/// Increase for high concurrency (3000+ VUs).
pub const DEFAULT_POOL_MAX_CONNECTIONS: usize = 2048;

/// Minimum worker threads in Node.js pool (floor).
/// Internal constant, not user-configurable.
pub const DEFAULT_POOL_MIN_THREADS: usize = 2;

/// Maximum worker threads floor (minimum threads even on small machines).
/// Internal constant, not user-configurable.
pub const DEFAULT_POOL_MAX_THREADS_FLOOR: usize = 8;

/// Concurrent tasks per worker thread in Node.js pool.
/// Internal constant, not user-configurable.
pub const DEFAULT_POOL_CONCURRENT_TASKS_PER_WORKER: usize = 20;

/// Headroom multiplier for calculating concurrent tasks per worker.
/// Applied to base task calculation to provide buffer for:
///   - Queue buildup during traffic spikes
///   - Variable plugin execution latency
///   - Temporary load imbalances across workers
///     Internal constant, not user-configurable.
pub const CONCURRENT_TASKS_HEADROOM_MULTIPLIER: f64 = 1.2;

/// Maximum concurrent tasks per worker thread (hard cap).
/// This cap prevents excessive memory usage and GC pressure per worker.
/// Validated through load testing as a stable upper bound.
/// Internal constant, not user-configurable.
pub const MAX_CONCURRENT_TASKS_PER_WORKER: usize = 250;

/// Worker idle timeout in milliseconds.
/// Internal constant, not user-configurable.
pub const DEFAULT_POOL_IDLE_TIMEOUT_MS: u64 = 60000; // 60 seconds

/// Socket connection backlog for the pool server.
/// Env: PLUGIN_POOL_SOCKET_BACKLOG (internal, rarely needs tuning)
pub const DEFAULT_POOL_SOCKET_BACKLOG: u32 = 2048;

/// Plugin execution timeout within the pool (milliseconds).
/// Internal constant - use per-plugin `timeout` in config.json instead.
pub const DEFAULT_POOL_EXECUTION_TIMEOUT_MS: u64 = 30000; // 30 seconds

/// Timeout for individual pool requests (seconds).
/// Env: PLUGIN_POOL_REQUEST_TIMEOUT_SECS
pub const DEFAULT_POOL_REQUEST_TIMEOUT_SECS: u64 = 30;

/// Maximum queued requests before rejection.
/// Env: PLUGIN_POOL_MAX_QUEUE_SIZE
/// Increase for high concurrency (3000+ VUs).
pub const DEFAULT_POOL_MAX_QUEUE_SIZE: usize = 5000;

/// Wait time (ms) when queue is full before rejecting.
/// Env: PLUGIN_POOL_QUEUE_SEND_TIMEOUT_MS
/// Increase for bursty traffic patterns.
pub const DEFAULT_POOL_QUEUE_SEND_TIMEOUT_MS: u64 = 500;

/// Minimum seconds between health checks.
/// Env: PLUGIN_POOL_HEALTH_CHECK_INTERVAL_SECS
/// Prevents health check storms under high load.
pub const DEFAULT_POOL_HEALTH_CHECK_INTERVAL_SECS: u64 = 5;

/// Retry attempts when connecting to pool server.
/// Env: PLUGIN_POOL_CONNECT_RETRIES
/// Increase for high concurrency scenarios.
pub const DEFAULT_POOL_CONNECT_RETRIES: usize = 15;

// =============================================================================
// Shared Socket Service Configuration
// Controls the Unix socket for plugin ↔ relayer communication.
// =============================================================================

/// Idle timeout for plugin connections (seconds).
/// Env: PLUGIN_SOCKET_IDLE_TIMEOUT_SECS
/// Connections idle longer than this are closed.
pub const DEFAULT_SOCKET_IDLE_TIMEOUT_SECS: u64 = 60;

/// Read timeout per line from plugins (seconds).
/// Env: PLUGIN_SOCKET_READ_TIMEOUT_SECS
/// Time to wait for a complete message from a plugin.
pub const DEFAULT_SOCKET_READ_TIMEOUT_SECS: u64 = 30;

/// Maximum concurrent plugin connections to the relayer.
/// Env: PLUGIN_SOCKET_MAX_CONCURRENT_CONNECTIONS
/// Should be >= PLUGIN_POOL_MAX_CONNECTIONS.
pub const DEFAULT_SOCKET_MAX_CONCURRENT_CONNECTIONS: usize = 4096;

/// Trace collection timeout (milliseconds).
/// Env: PLUGIN_TRACE_TIMEOUT_MS
/// Short timeout since traces arrive immediately after plugin execution.
pub const DEFAULT_TRACE_TIMEOUT_MS: u64 = 100;
