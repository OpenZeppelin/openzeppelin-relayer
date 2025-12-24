/// Default plugin execution timeout in seconds
pub const DEFAULT_PLUGIN_TIMEOUT_SECONDS: u64 = 300; // 5 minutes

// =============================================================================
// Plugin Pool Server Configuration
// These constants are the source of truth. The TypeScript pool-server.ts and
// worker-pool.ts files should use matching values.
// =============================================================================

/// Default maximum connections from Rust to the pool server
pub const DEFAULT_POOL_MAX_CONNECTIONS: usize = 64;

/// Default minimum worker threads (floor)
pub const DEFAULT_POOL_MIN_THREADS: usize = 2;

/// Default maximum worker threads floor (minimum threads even on small machines)
pub const DEFAULT_POOL_MAX_THREADS_FLOOR: usize = 8;

/// Default concurrent tasks per worker thread
pub const DEFAULT_POOL_CONCURRENT_TASKS_PER_WORKER: usize = 10;

/// Default worker idle timeout in milliseconds
pub const DEFAULT_POOL_IDLE_TIMEOUT_MS: u64 = 60000; // 60 seconds

/// Default socket connection backlog for high concurrency
pub const DEFAULT_POOL_SOCKET_BACKLOG: u32 = 1024;

/// Default plugin execution timeout in milliseconds (within pool)
pub const DEFAULT_POOL_EXECUTION_TIMEOUT_MS: u64 = 30000; // 30 seconds

/// Default request timeout in seconds (for pool requests)
pub const DEFAULT_POOL_REQUEST_TIMEOUT_SECS: u64 = 30;

/// Default maximum queue size for request throttling
pub const DEFAULT_POOL_MAX_QUEUE_SIZE: usize = 1000;

// =============================================================================
// Shared Socket Service Configuration
// =============================================================================

/// Default connection idle timeout in seconds
pub const DEFAULT_SOCKET_IDLE_TIMEOUT_SECS: u64 = 60;

/// Default read timeout per line in seconds
pub const DEFAULT_SOCKET_READ_TIMEOUT_SECS: u64 = 30;

/// Maximum concurrent connections (backlog limit)
pub const DEFAULT_SOCKET_MAX_CONCURRENT_CONNECTIONS: usize = 1024;

/// Default trace collection timeout in seconds
pub const DEFAULT_TRACE_TIMEOUT_SECS: u64 = 5;
