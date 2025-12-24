/**
 * Plugin Pool Constants
 *
 * These values should match src/constants/plugins.rs which is the source of truth.
 * When updating these values, ensure the Rust constants are updated as well.
 */

// =============================================================================
// Pool Configuration
// =============================================================================

/** Default minimum worker threads */
export const DEFAULT_POOL_MIN_THREADS = 2;

/** Divisor for calculating minThreads from CPU count (cpuCount / divisor) */
export const DEFAULT_POOL_MIN_THREADS_DIVISOR = 2;

/** Default maximum worker threads floor (minimum threads even on small machines) */
export const DEFAULT_POOL_MAX_THREADS_FLOOR = 8;

/** Default concurrent tasks per worker thread */
export const DEFAULT_POOL_CONCURRENT_TASKS_PER_WORKER = 10;

/** Default worker idle timeout in milliseconds */
export const DEFAULT_POOL_IDLE_TIMEOUT_MS = 60000; // 60 seconds

/** Default socket connection backlog for high concurrency */
export const DEFAULT_POOL_SOCKET_BACKLOG = 1024;

/** Default plugin execution timeout in milliseconds */
export const DEFAULT_POOL_EXECUTION_TIMEOUT_MS = 30000; // 30 seconds

// =============================================================================
// Worker Pool Specific (may differ from pool-server defaults)
// =============================================================================

/** Higher thread floor for standalone worker pool usage */
export const DEFAULT_WORKER_POOL_MAX_THREADS_FLOOR = 16;

/** Higher concurrency for I/O bound tasks in worker pool */
export const DEFAULT_WORKER_POOL_CONCURRENT_TASKS_PER_WORKER = 20;
