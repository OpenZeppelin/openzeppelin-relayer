use std::sync::Arc;
use std::time::Duration;

use color_eyre::Result;
use deadpool_redis::{Config, Pool, Runtime};
use tracing::{debug, info, warn};

use crate::config::ServerConfig;

/// Holds separate connection pools for read and write operations.
///
/// This struct enables optimization for Redis deployments with read replicas,
/// such as AWS ElastiCache. Write operations use the primary pool, while read
/// operations can be distributed across reader replicas.
///
/// When `REDIS_READER_URL` is not configured, both pools point to the same
/// Redis instance (the primary), maintaining backward compatibility.
#[derive(Clone, Debug)]
pub struct RedisConnections {
    /// Pool for write operations (connected to primary endpoint)
    primary_pool: Arc<Pool>,
    /// Pool for read operations (connected to reader endpoint, or primary if not configured)
    reader_pool: Arc<Pool>,
}

impl RedisConnections {
    /// Creates a new `RedisConnections` with a single pool used for both reads and writes.
    ///
    /// This is useful for:
    /// - Testing where read/write separation is not needed
    /// - Simple deployments without read replicas
    /// - Backward compatibility
    pub fn new_single_pool(pool: Arc<Pool>) -> Self {
        Self {
            primary_pool: pool.clone(),
            reader_pool: pool,
        }
    }

    /// Returns the primary pool for write operations.
    ///
    /// Use this for: `create`, `update`, `delete`, and any operation that
    /// modifies data in Redis.
    pub fn primary(&self) -> &Arc<Pool> {
        &self.primary_pool
    }

    /// Returns the reader pool for read operations.
    ///
    /// Use this for: `get_by_id`, `list`, `find_by_*`, `count`, and any
    /// operation that only reads data from Redis.
    ///
    /// If no reader endpoint is configured, this returns the same pool as `primary()`.
    pub fn reader(&self) -> &Arc<Pool> {
        &self.reader_pool
    }
}

/// Creates a Redis connection pool with the specified URL, pool size, and configuration.
async fn create_pool(url: &str, pool_max_size: usize, config: &ServerConfig) -> Result<Arc<Pool>> {
    let cfg = Config::from_url(url);

    let pool = cfg
        .builder()
        .map_err(|e| eyre::eyre!("Failed to create Redis pool builder for {}: {}", url, e))?
        .max_size(pool_max_size)
        .wait_timeout(Some(Duration::from_millis(config.redis_pool_timeout_ms)))
        .create_timeout(Some(Duration::from_millis(
            config.redis_connection_timeout_ms,
        )))
        .recycle_timeout(Some(Duration::from_millis(
            config.redis_connection_timeout_ms,
        )))
        .runtime(Runtime::Tokio1)
        .build()
        .map_err(|e| eyre::eyre!("Failed to build Redis pool for {}: {}", url, e))?;

    // Verify the pool is working by getting a connection
    let conn = pool
        .get()
        .await
        .map_err(|e| eyre::eyre!("Failed to get initial Redis connection from {}: {}", url, e))?;
    drop(conn);

    Ok(Arc::new(pool))
}

/// Initializes Redis connection pools for both primary and reader endpoints.
///
/// # Arguments
///
/// * `config` - The server configuration containing Redis URLs and pool settings.
///
/// # Returns
///
/// A `RedisConnections` struct containing:
/// - `primary_pool`: Connected to `REDIS_URL` (for write operations)
/// - `reader_pool`: Connected to `REDIS_READER_URL` if set, otherwise same as primary
///
/// # Features
///
/// - **Read/Write Separation**: When `REDIS_READER_URL` is configured, read operations
///   can be distributed across read replicas, reducing load on the primary.
/// - **Backward Compatible**: If `REDIS_READER_URL` is not set, both pools use
///   the primary URL, maintaining existing behavior.
/// - **Connection Pooling**: Both pools use deadpool-redis with configurable
///   max size, wait timeout, and connection timeouts.
///
/// # Example Configuration
///
/// ```bash
/// # AWS ElastiCache with read replicas
/// REDIS_URL=redis://my-cluster.xxx.cache.amazonaws.com:6379
/// REDIS_READER_URL=redis://my-cluster-ro.xxx.cache.amazonaws.com:6379
/// ```
pub async fn initialize_redis_connections(config: &ServerConfig) -> Result<Arc<RedisConnections>> {
    let primary_pool_size = config.redis_pool_max_size;
    let primary_pool = create_pool(&config.redis_url, primary_pool_size, config).await?;

    info!(
        primary_url = %config.redis_url,
        primary_pool_size = %primary_pool_size,
        "initializing primary Redis connection pool"
    );
    let reader_pool = match &config.redis_reader_url {
        Some(reader_url) if !reader_url.is_empty() => {
            let reader_pool_size = config.redis_reader_pool_max_size;

            info!(
                primary_url = %config.redis_url,
                reader_url = %reader_url,
                primary_pool_size = %primary_pool_size,
                reader_pool_size = %reader_pool_size,
                "Using separate reader endpoint for read operations"
            );
            create_pool(reader_url, reader_pool_size, config).await?
        }
        _ => {
            debug!(
                primary_url = %config.redis_url,
                pool_size = %primary_pool_size,
                "No reader URL configured, using primary for all operations"
            );
            primary_pool.clone()
        }
    };

    Ok(Arc::new(RedisConnections {
        primary_pool,
        reader_pool,
    }))
}

/// A distributed lock implementation using Redis SET NX EX pattern.
///
/// This lock is designed for distributed systems where multiple instances
/// need to coordinate exclusive access to shared resources. It uses the
/// Redis SET command with NX (only set if not exists) and EX (expiry time)
/// options to atomically acquire locks.
///
/// # Features
/// - **Atomic acquisition**: Uses Redis SET NX to ensure only one instance can acquire the lock
/// - **Automatic expiry**: Lock automatically expires after TTL to prevent deadlocks
/// - **Unique identifiers**: Each lock acquisition gets a unique ID to prevent accidental release by other instances
/// - **Auto-release on drop**: Lock is released when the guard is dropped (best-effort via spawned task)
///
/// # TTL Considerations
/// The TTL should be set to accommodate the worst-case runtime of the protected operation.
/// If the operation runs longer than TTL, another instance may acquire the lock concurrently.
/// For long-running operations, consider:
/// - Setting a generous TTL (e.g., 2x expected runtime)
/// - Implementing lock refresh/extension during processing
/// - Breaking the operation into smaller locked sections
///
/// # Example
/// ```ignore
/// // Key format: {prefix}:lock:{name}
/// let lock_key = format!("{}:lock:{}", prefix, "my-operation");
/// let lock = DistributedLock::new(redis_client, &lock_key, Duration::from_secs(60));
/// if let Some(guard) = lock.try_acquire().await? {
///     // Do exclusive work here
///     // Lock is automatically released when guard is dropped
/// }
/// ```
#[derive(Clone)]
pub struct DistributedLock {
    pool: Arc<Pool>,
    lock_key: String,
    ttl: Duration,
}

impl DistributedLock {
    /// Creates a new distributed lock instance.
    ///
    /// # Arguments
    /// * `pool` - Redis connection pool
    /// * `lock_key` - Full Redis key for this lock (e.g., "myprefix:lock:cleanup")
    /// * `ttl` - Time-to-live for the lock. Lock will automatically expire after this duration
    ///   to prevent deadlocks if the holder crashes.
    pub fn new(pool: Arc<Pool>, lock_key: &str, ttl: Duration) -> Self {
        Self {
            pool,
            lock_key: lock_key.to_string(),
            ttl,
        }
    }

    /// Attempts to acquire the distributed lock.
    ///
    /// This is a non-blocking operation. If the lock is already held by another
    /// instance, it returns `Ok(None)` immediately without waiting.
    ///
    /// # Returns
    /// * `Ok(Some(LockGuard))` - Lock was successfully acquired
    /// * `Ok(None)` - Lock is already held by another instance
    /// * `Err(_)` - Redis communication error
    ///
    /// # Lock Semantics
    /// The lock is implemented using Redis `SET key value NX EX ttl`:
    /// - `NX`: Only set if key does not exist (atomic check-and-set)
    /// - `EX`: Set expiry time in seconds
    pub async fn try_acquire(&self) -> Result<Option<LockGuard>> {
        let lock_value = generate_lock_value();
        let mut conn = self.pool.get().await?;

        // Use SET NX EX for atomic lock acquisition with expiry
        let result: Option<String> = redis::cmd("SET")
            .arg(&self.lock_key)
            .arg(&lock_value)
            .arg("NX")
            .arg("EX")
            .arg(self.ttl.as_secs())
            .query_async(&mut conn)
            .await?;

        match result {
            Some(_) => {
                debug!(
                    lock_key = %self.lock_key,
                    ttl_secs = %self.ttl.as_secs(),
                    "distributed lock acquired"
                );
                Ok(Some(LockGuard {
                    release_info: Some(LockReleaseInfo {
                        pool: self.pool.clone(),
                        lock_key: self.lock_key.clone(),
                        lock_value,
                    }),
                }))
            }
            None => {
                debug!(
                    lock_key = %self.lock_key,
                    "distributed lock already held by another instance"
                );
                Ok(None)
            }
        }
    }
}

/// A guard that represents an acquired distributed lock.
///
/// The lock is automatically released when this guard is dropped. This ensures
/// the lock is released regardless of how the protected code exits (normal return,
/// early return via `?`, or panic).
///
/// The release is performed via a spawned task to avoid blocking in `Drop`.
/// If you need to confirm the release succeeded, call `release()` explicitly instead.
///
/// # Drop Behavior
/// When dropped, the guard spawns an async task to release the lock. This is
/// best-effort: if the task fails (e.g., Redis unavailable), the lock will
/// still expire after TTL.
pub struct LockGuard {
    /// Release info is wrapped in Option to support both explicit release and Drop.
    /// When `release()` is called, this is taken (set to None) to prevent double-release.
    /// When Drop runs, if this is Some, we spawn a release task.
    release_info: Option<LockReleaseInfo>,
}

/// Internal struct holding the information needed to release a lock.
struct LockReleaseInfo {
    pool: Arc<Pool>,
    lock_key: String,
    lock_value: String,
}

impl LockGuard {
    /// Explicitly releases the lock before TTL expiry.
    ///
    /// This uses a Lua script to ensure we only delete the lock if we still own it.
    /// This prevents accidentally releasing a lock that was already expired and
    /// re-acquired by another instance.
    ///
    /// Calling this method consumes the guard and prevents the Drop-based release.
    ///
    /// # Returns
    /// * `Ok(true)` - Lock was successfully released
    /// * `Ok(false)` - Lock was not released (already expired or owned by another instance)
    /// * `Err(_)` - Redis communication error
    pub async fn release(mut self) -> Result<bool> {
        // Take the release info to prevent Drop from also trying to release
        let info = self
            .release_info
            .take()
            .expect("release_info should always be Some before release");

        Self::do_release(info).await
    }

    /// Internal async release implementation.
    async fn do_release(info: LockReleaseInfo) -> Result<bool> {
        let mut conn = info.pool.get().await?;

        // Lua script to atomically check and delete
        // Only delete if the value matches (we still own the lock)
        let script = r#"
            if redis.call("GET", KEYS[1]) == ARGV[1] then
                return redis.call("DEL", KEYS[1])
            else
                return 0
            end
        "#;

        let result: i32 = redis::Script::new(script)
            .key(&info.lock_key)
            .arg(&info.lock_value)
            .invoke_async(&mut conn)
            .await?;

        if result == 1 {
            debug!(lock_key = %info.lock_key, "distributed lock released");
            Ok(true)
        } else {
            warn!(
                lock_key = %info.lock_key,
                "distributed lock release failed - lock already expired or owned by another instance"
            );
            Ok(false)
        }
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // If release_info is still Some, we need to release the lock
        if let Some(info) = self.release_info.take() {
            // Spawn a task to release the lock asynchronously
            // This is best-effort; if it fails, the lock will expire after TTL
            tokio::spawn(async move {
                if let Err(e) = LockGuard::do_release(info).await {
                    warn!(error = %e, "failed to release distributed lock in drop, will expire after TTL");
                }
            });
        }
    }
}

/// Generates a unique value for lock ownership verification.
///
/// Uses a combination of process ID, timestamp, and a monotonic counter
/// to create a unique identifier for this lock acquisition. This avoids
/// collisions in the same process even when calls share a timestamp.
fn generate_lock_value() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};

    static LOCK_VALUE_COUNTER: AtomicU64 = AtomicU64::new(0);
    let process_id = std::process::id();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = LOCK_VALUE_COUNTER.fetch_add(1, Ordering::Relaxed);

    format!("{process_id}:{timestamp}:{counter}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_lock_value_is_unique() {
        let value1 = generate_lock_value();
        let value2 = generate_lock_value();

        // Values should be different due to monotonic counter
        assert_ne!(value1, value2);
    }

    #[test]
    fn test_generate_lock_value_contains_process_id() {
        let value = generate_lock_value();

        // Should contain a colon separator
        assert!(value.contains(':'));

        // Should have two parts: process_id and timestamp
        let parts: Vec<&str> = value.split(':').collect();
        assert_eq!(parts.len(), 3);

        // First part should be parseable as u32 (process ID)
        assert!(parts[0].parse::<u32>().is_ok());
    }

    #[test]
    fn test_distributed_lock_key_format() {
        // Verify the lock key format: {prefix}:lock:{name}
        let prefix = "myrelayer";
        let lock_name = "transaction_cleanup";
        let lock_key = format!("{}:lock:{}", prefix, lock_name);
        assert_eq!(lock_key, "myrelayer:lock:transaction_cleanup");
    }

    #[test]
    fn test_distributed_lock_key_format_with_complex_prefix() {
        // Test with a more realistic prefix
        let prefix = "oz-relayer-prod";
        let lock_name = "transaction_cleanup";
        let lock_key = format!("{}:lock:{}", prefix, lock_name);
        assert_eq!(lock_key, "oz-relayer-prod:lock:transaction_cleanup");
    }

    #[test]
    fn test_distributed_lock_key_uses_exact_key() {
        // DistributedLock now uses the exact key provided (no automatic prefix)
        let lock_key = "myprefix:lock:myoperation";
        // The lock will use this key exactly as provided
        assert_eq!(lock_key, "myprefix:lock:myoperation");
    }

    // =========================================================================
    // RedisConnections tests
    // =========================================================================

    mod redis_connections_tests {
        use super::*;

        #[test]
        fn test_new_single_pool_returns_same_pool_for_both() {
            // This test verifies the backward-compatible single pool mode
            // When no REDIS_READER_URL is set, both pools should be the same
            let cfg = Config::from_url("redis://127.0.0.1:6379");
            let pool = cfg
                .builder()
                .expect("Failed to create pool builder")
                .max_size(16)
                .runtime(Runtime::Tokio1)
                .build()
                .expect("Failed to build pool");
            let pool = Arc::new(pool);

            let connections = RedisConnections::new_single_pool(pool.clone());

            // Both primary and reader should return the same pool
            assert!(Arc::ptr_eq(connections.primary(), &pool));
            assert!(Arc::ptr_eq(connections.reader(), &pool));
            assert!(Arc::ptr_eq(connections.primary(), connections.reader()));
        }

        #[test]
        fn test_redis_connections_clone() {
            // Verify that RedisConnections can be cloned
            let cfg = Config::from_url("redis://127.0.0.1:6379");
            let pool = cfg
                .builder()
                .expect("Failed to create pool builder")
                .max_size(16)
                .runtime(Runtime::Tokio1)
                .build()
                .expect("Failed to build pool");
            let pool = Arc::new(pool);

            let connections = RedisConnections::new_single_pool(pool);
            let cloned = connections.clone();

            // Both should point to the same pools
            assert!(Arc::ptr_eq(connections.primary(), cloned.primary()));
            assert!(Arc::ptr_eq(connections.reader(), cloned.reader()));
        }

        #[test]
        fn test_redis_connections_debug() {
            // Verify Debug implementation exists
            let cfg = Config::from_url("redis://127.0.0.1:6379");
            let pool = cfg
                .builder()
                .expect("Failed to create pool builder")
                .max_size(16)
                .runtime(Runtime::Tokio1)
                .build()
                .expect("Failed to build pool");
            let pool = Arc::new(pool);

            let connections = RedisConnections::new_single_pool(pool);
            let debug_str = format!("{:?}", connections);

            assert!(debug_str.contains("RedisConnections"));
        }
    }

    // =========================================================================
    // Integration tests - require a running Redis instance
    // Run with: cargo test --lib redis::tests::integration -- --ignored
    // =========================================================================

    /// Helper to create a Redis connection for integration tests.
    /// Expects Redis to be running on localhost:6379.
    async fn create_test_redis_connection() -> Option<Arc<Pool>> {
        let cfg = Config::from_url("redis://127.0.0.1:6379");
        let pool = cfg
            .builder()
            .ok()?
            .max_size(16)
            .runtime(Runtime::Tokio1)
            .build()
            .ok()?;
        Some(Arc::new(pool))
    }

    mod integration {
        use super::*;

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_distributed_lock_acquire_and_release() {
            let conn = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            let lock =
                DistributedLock::new(conn, "test:lock:acquire_release", Duration::from_secs(60));

            // Should be able to acquire the lock
            let guard = lock.try_acquire().await.expect("Redis error");
            assert!(guard.is_some(), "Should acquire lock when not held");

            // Release the lock
            let released = guard.unwrap().release().await.expect("Redis error");
            assert!(released, "Should successfully release the lock");
        }

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_distributed_lock_prevents_double_acquisition() {
            let conn = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            let lock1 = DistributedLock::new(
                conn.clone(),
                "test:lock:double_acquire",
                Duration::from_secs(60),
            );
            let lock2 =
                DistributedLock::new(conn, "test:lock:double_acquire", Duration::from_secs(60));

            // First acquisition should succeed
            let guard1 = lock1.try_acquire().await.expect("Redis error");
            assert!(guard1.is_some(), "First acquisition should succeed");

            // Second acquisition should fail (lock already held)
            let guard2 = lock2.try_acquire().await.expect("Redis error");
            assert!(
                guard2.is_none(),
                "Second acquisition should fail - lock already held"
            );

            // Release the first lock
            guard1.unwrap().release().await.expect("Redis error");

            // Now second acquisition should succeed
            let guard2_retry = lock2.try_acquire().await.expect("Redis error");
            assert!(guard2_retry.is_some(), "Should acquire lock after release");

            // Cleanup
            guard2_retry.unwrap().release().await.expect("Redis error");
        }

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_distributed_lock_expires_after_ttl() {
            let conn = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            // Use a very short TTL for testing
            let lock =
                DistributedLock::new(conn.clone(), "test:lock:ttl_expiry", Duration::from_secs(1));

            // Acquire the lock
            let guard = lock.try_acquire().await.expect("Redis error");
            assert!(guard.is_some(), "Should acquire lock");

            // Don't release - let it expire
            drop(guard);

            // Wait for TTL to expire
            tokio::time::sleep(Duration::from_secs(2)).await;

            // Should be able to acquire again after expiry
            let lock2 = DistributedLock::new(conn, "test:lock:ttl_expiry", Duration::from_secs(60));
            let guard2 = lock2.try_acquire().await.expect("Redis error");
            assert!(guard2.is_some(), "Should acquire lock after TTL expiry");

            // Cleanup
            if let Some(g) = guard2 {
                g.release().await.expect("Redis error");
            }
        }

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_distributed_lock_release_only_own_lock() {
            let conn = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            // Use a short TTL
            let lock = DistributedLock::new(
                conn.clone(),
                "test:lock:release_own",
                Duration::from_secs(1),
            );

            // Acquire the lock
            let guard = lock.try_acquire().await.expect("Redis error");
            assert!(guard.is_some(), "Should acquire lock");

            // Wait for lock to expire
            tokio::time::sleep(Duration::from_secs(2)).await;

            // Try to release expired lock - should return false (not owned anymore)
            let released = guard.unwrap().release().await.expect("Redis error");
            assert!(!released, "Should not release expired lock");
        }

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_distributed_lock_with_prefix() {
            let conn = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            // Simulate prefixed lock key like in transaction cleanup
            // Format: {prefix}:lock:{name}
            let prefix = "test_prefix";
            let lock_name = "cleanup";
            let lock_key = format!("{}:lock:{}", prefix, lock_name);

            let lock = DistributedLock::new(conn, &lock_key, Duration::from_secs(60));

            let guard = lock.try_acquire().await.expect("Redis error");
            assert!(guard.is_some(), "Should acquire prefixed lock");

            // Cleanup
            guard.unwrap().release().await.expect("Redis error");
        }

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_distributed_lock_drop_releases_lock() {
            let conn = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            let lock_key = "test:lock:drop_release";

            // Acquire lock in inner scope
            {
                let lock = DistributedLock::new(conn.clone(), lock_key, Duration::from_secs(60));
                let guard = lock.try_acquire().await.expect("Redis error");
                assert!(guard.is_some(), "Should acquire lock");

                // Guard is dropped here without explicit release()
            }

            // Give the spawned release task time to complete
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Should be able to acquire again because Drop released the lock
            let lock2 = DistributedLock::new(conn, lock_key, Duration::from_secs(60));
            let guard2 = lock2.try_acquire().await.expect("Redis error");
            assert!(
                guard2.is_some(),
                "Should acquire lock after previous guard was dropped"
            );

            // Cleanup
            if let Some(g) = guard2 {
                g.release().await.expect("Redis error");
            }
        }

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_distributed_lock_explicit_release_prevents_double_release() {
            let conn = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            let lock_key = "test:lock:no_double_release";
            let lock = DistributedLock::new(conn.clone(), lock_key, Duration::from_secs(60));

            let guard = lock.try_acquire().await.expect("Redis error");
            assert!(guard.is_some(), "Should acquire lock");

            // Explicitly release
            let released = guard.unwrap().release().await.expect("Redis error");
            assert!(released, "Should release successfully");

            // Guard is now consumed by release(), Drop won't run
            // Verify lock is still released (not double-locked)
            tokio::time::sleep(Duration::from_millis(50)).await;

            let lock2 = DistributedLock::new(conn, lock_key, Duration::from_secs(60));
            let guard2 = lock2.try_acquire().await.expect("Redis error");
            assert!(
                guard2.is_some(),
                "Should acquire lock - no double-release issue"
            );

            // Cleanup
            if let Some(g) = guard2 {
                g.release().await.expect("Redis error");
            }
        }

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_distributed_lock_drop_on_early_return() {
            let conn = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            let lock_key = "test:lock:early_return";

            // Simulate a function that returns early (like ? operator)
            async fn simulated_work_with_early_return(
                conn: Arc<Pool>,
                lock_key: &str,
            ) -> Result<(), &'static str> {
                let lock = DistributedLock::new(conn, lock_key, Duration::from_secs(60));
                let _guard = lock
                    .try_acquire()
                    .await
                    .map_err(|_| "lock error")?
                    .ok_or("lock held")?;

                // Simulate early return (error path)
                return Err("simulated error");

                // _guard is dropped here due to early return
            }

            // Call the function that returns early
            let result = simulated_work_with_early_return(conn.clone(), lock_key).await;
            assert!(result.is_err(), "Should have returned early with error");

            // Give the spawned release task time to complete
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Lock should be released despite the early return
            let lock2 = DistributedLock::new(conn, lock_key, Duration::from_secs(60));
            let guard2 = lock2.try_acquire().await.expect("Redis error");
            assert!(
                guard2.is_some(),
                "Should acquire lock after early return released it"
            );

            // Cleanup
            if let Some(g) = guard2 {
                g.release().await.expect("Redis error");
            }
        }

        // =========================================================================
        // RedisConnections integration tests
        // =========================================================================

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_redis_connections_single_pool_operations() {
            let pool = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            let connections = RedisConnections::new_single_pool(pool);

            // Test that we can get connections from both pools
            let mut primary_conn = connections
                .primary()
                .get()
                .await
                .expect("Failed to get primary connection");
            let mut reader_conn = connections
                .reader()
                .get()
                .await
                .expect("Failed to get reader connection");

            // Test basic operations on both connections
            let _: () = redis::cmd("SET")
                .arg("test:connections:key")
                .arg("test_value")
                .query_async(&mut primary_conn)
                .await
                .expect("Failed to SET");

            let value: String = redis::cmd("GET")
                .arg("test:connections:key")
                .query_async(&mut reader_conn)
                .await
                .expect("Failed to GET");

            assert_eq!(value, "test_value");

            // Cleanup
            let _: () = redis::cmd("DEL")
                .arg("test:connections:key")
                .query_async(&mut primary_conn)
                .await
                .expect("Failed to DEL");
        }

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_redis_connections_backward_compatible() {
            // Verify that single pool mode (no reader URL) works correctly
            let pool = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            let connections = Arc::new(RedisConnections::new_single_pool(pool));

            // Multiple repositories should be able to share the connections
            let conn1 = connections.clone();
            let conn2 = connections.clone();

            // Both should be able to get connections
            let _primary1 = conn1.primary().get().await.expect("Failed to get primary");
            let _reader1 = conn1.reader().get().await.expect("Failed to get reader");
            let _primary2 = conn2.primary().get().await.expect("Failed to get primary");
            let _reader2 = conn2.reader().get().await.expect("Failed to get reader");

            // All should work without issues (backward compatible)
        }
    }
}
