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

/**
 * Per-API-call socket timeout (ms). This is the timeout for individual
 * relayer API calls within a plugin (e.g., sendTransaction, getTransaction).
 * Short timeout since it's just a socket round-trip to the Rust relayer.
 */
export const SOCKET_REQUEST_TIMEOUT_MS = 30000; // 30 seconds

/**
 * Default plugin execution timeout (ms). Matches DEFAULT_PLUGIN_TIMEOUT_SECONDS
 * (300s) in Rust. In production, Rust sends the per-plugin timeout with each request,
 * so this is only a fallback for standalone testing.
 */
export const DEFAULT_PLUGIN_TIMEOUT_MS = 300000; // 5 minutes
