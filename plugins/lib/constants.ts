/**
 * Plugin Pool Constants
 *
 * IMPORTANT: These are fallback values only for standalone testing.
 * In production, ALL configuration comes from Rust (src/services/plugins/config.rs)
 * and is passed via environment variables when spawning the pool server.
 *
 * The single source of truth is: PLUGIN_MAX_CONCURRENCY
 * Set it in .env and Rust derives everything else.
 */

// =============================================================================
// Fallbacks for standalone testing (when not spawned by Rust)
// =============================================================================

/** Fallback minimum worker threads if not passed from Rust */
export const DEFAULT_POOL_MIN_THREADS = 2;

/** Fallback max threads if not passed from Rust */
export const DEFAULT_POOL_MAX_THREADS_FLOOR = 8;

/** Fallback concurrent tasks if not passed from Rust */
export const DEFAULT_POOL_CONCURRENT_TASKS_PER_WORKER = 20;

/** Fallback idle timeout if not passed from Rust */
export const DEFAULT_POOL_IDLE_TIMEOUT_MS = 60000;

/** Socket backlog for high concurrency */
export const DEFAULT_POOL_SOCKET_BACKLOG = 2048;

/** Default execution timeout (ms) */
export const DEFAULT_POOL_EXECUTION_TIMEOUT_MS = 30000;

/** Default per-request timeout for socket communication (ms) */
export const DEFAULT_SOCKET_REQUEST_TIMEOUT_MS = 30000;
