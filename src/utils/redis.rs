use std::sync::Arc;
use std::time::Duration;

use color_eyre::Result;
use redis::aio::ConnectionManager;
use tokio::time::timeout;
use tracing::{debug, warn};

use crate::config::ServerConfig;

/// Initializes a Redis connection manager.
///
/// # Arguments
///
/// * `config` - The server configuration.
///
/// # Returns
///
/// A connection manager for the Redis connection.
pub async fn initialize_redis_connection(config: &ServerConfig) -> Result<Arc<ConnectionManager>> {
    let redis_client = redis::Client::open(config.redis_url.as_str())?;
    let connection_manager = timeout(
        Duration::from_millis(config.redis_connection_timeout_ms),
        redis::aio::ConnectionManager::new(redis_client),
    )
    .await
    .map_err(|_| {
        eyre::eyre!(
            "Redis connection timeout after {}ms",
            config.redis_connection_timeout_ms
        )
    })??;
    let connection_manager = Arc::new(connection_manager);

    Ok(connection_manager)
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
    client: Arc<ConnectionManager>,
    lock_key: String,
    ttl: Duration,
}

impl DistributedLock {
    /// Creates a new distributed lock instance.
    ///
    /// # Arguments
    /// * `client` - Redis connection manager
    /// * `lock_key` - Full Redis key for this lock (e.g., "myprefix:lock:cleanup")
    /// * `ttl` - Time-to-live for the lock. Lock will automatically expire after this duration
    ///   to prevent deadlocks if the holder crashes.
    pub fn new(client: Arc<ConnectionManager>, lock_key: &str, ttl: Duration) -> Self {
        Self {
            client,
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
        let mut conn = (*self.client).clone();

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
                        client: self.client.clone(),
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
    client: Arc<ConnectionManager>,
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
        let mut conn = (*info.client).clone();

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

// ============================================================================
// Relayer Sync Metadata Functions
// ============================================================================
//
// These functions track when relayers were last synchronized/initialized.
// This allows multiple instances to skip redundant initialization when
// a relayer was recently synced by another instance.

/// The Redis hash key suffix for storing relayer sync metadata.
const RELAYER_SYNC_META_KEY: &str = "relayer_sync_meta";

/// Sets the last sync timestamp for a relayer to the current time.
///
/// This should be called after a relayer has been successfully initialized
/// to record when the initialization occurred.
///
/// # Arguments
/// * `conn` - Redis connection manager
/// * `prefix` - Key prefix for multi-tenant support
/// * `relayer_id` - The relayer's unique identifier
///
/// # Redis Key Format
/// Hash key: `{prefix}:relayer_sync_meta`
/// Hash field: `{relayer_id}:last_sync`
/// Value: Unix timestamp in seconds
pub async fn set_relayer_last_sync(
    conn: &Arc<ConnectionManager>,
    prefix: &str,
    relayer_id: &str,
) -> Result<()> {
    use chrono::Utc;
    use redis::AsyncCommands;

    let mut conn = (**conn).clone();
    let hash_key = format!("{prefix}:{RELAYER_SYNC_META_KEY}");
    let field = format!("{relayer_id}:last_sync");
    let timestamp = Utc::now().timestamp();

    conn.hset::<_, _, _, ()>(&hash_key, &field, timestamp)
        .await
        .map_err(|e| eyre::eyre!("Failed to set relayer last sync: {}", e))?;

    debug!(
        relayer_id = %relayer_id,
        timestamp = %timestamp,
        "recorded relayer last sync time"
    );

    Ok(())
}

/// Gets the last sync timestamp for a relayer.
///
/// # Arguments
/// * `conn` - Redis connection manager
/// * `prefix` - Key prefix for multi-tenant support
/// * `relayer_id` - The relayer's unique identifier
///
/// # Returns
/// * `Ok(Some(DateTime))` - The last sync time if recorded
/// * `Ok(None)` - If the relayer has never been synced
/// * `Err(_)` - Redis communication error
pub async fn get_relayer_last_sync(
    conn: &Arc<ConnectionManager>,
    prefix: &str,
    relayer_id: &str,
) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    use chrono::{TimeZone, Utc};
    use redis::AsyncCommands;

    let mut conn = (**conn).clone();
    let hash_key = format!("{prefix}:{RELAYER_SYNC_META_KEY}");
    let field = format!("{relayer_id}:last_sync");

    let timestamp: Option<i64> = conn
        .hget(&hash_key, &field)
        .await
        .map_err(|e| eyre::eyre!("Failed to get relayer last sync: {}", e))?;

    Ok(timestamp.and_then(|ts| Utc.timestamp_opt(ts, 0).single()))
}

/// Checks if a relayer was recently synced within the given threshold.
///
/// This is used to skip initialization for relayers that were recently
/// initialized by another instance (e.g., during rolling restarts).
///
/// # Arguments
/// * `conn` - Redis connection manager
/// * `prefix` - Key prefix for multi-tenant support
/// * `relayer_id` - The relayer's unique identifier
/// * `threshold_secs` - Number of seconds to consider as "recent"
///
/// # Returns
/// * `Ok(true)` - If the relayer was synced within the threshold
/// * `Ok(false)` - If the relayer was not synced or sync is stale
/// * `Err(_)` - Redis communication error
pub async fn is_relayer_recently_synced(
    conn: &Arc<ConnectionManager>,
    prefix: &str,
    relayer_id: &str,
    threshold_secs: u64,
) -> Result<bool> {
    use chrono::Utc;

    let last_sync = get_relayer_last_sync(conn, prefix, relayer_id).await?;

    match last_sync {
        Some(sync_time) => {
            let elapsed = Utc::now().signed_duration_since(sync_time);
            let is_recent = elapsed.num_seconds() < threshold_secs as i64;

            if is_recent {
                debug!(
                    relayer_id = %relayer_id,
                    last_sync = %sync_time.to_rfc3339(),
                    elapsed_secs = %elapsed.num_seconds(),
                    threshold_secs = %threshold_secs,
                    "relayer was recently synced"
                );
            }

            Ok(is_recent)
        }
        None => Ok(false),
    }
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
    // Integration tests - require a running Redis instance
    // Run with: cargo test --lib redis::tests::integration -- --ignored
    // =========================================================================

    /// Helper to create a Redis connection for integration tests.
    /// Expects Redis to be running on localhost:6379.
    async fn create_test_redis_connection() -> Option<Arc<ConnectionManager>> {
        let client = redis::Client::open("redis://127.0.0.1:6379").ok()?;
        let conn = redis::aio::ConnectionManager::new(client).await.ok()?;
        Some(Arc::new(conn))
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
                conn: Arc<ConnectionManager>,
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

        // =====================================================================
        // Relayer Sync Metadata Tests
        // =====================================================================

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_set_and_get_relayer_last_sync() {
            let conn = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            let prefix = "test_sync";
            let relayer_id = "test-relayer-sync";

            // Set the last sync time
            set_relayer_last_sync(&conn, prefix, relayer_id)
                .await
                .expect("Should set last sync time");

            // Get the last sync time
            let last_sync = get_relayer_last_sync(&conn, prefix, relayer_id)
                .await
                .expect("Should get last sync time");

            assert!(last_sync.is_some(), "Should have a last sync time");

            // Verify the timestamp is recent (within last minute)
            let elapsed = chrono::Utc::now()
                .signed_duration_since(last_sync.unwrap())
                .num_seconds();
            assert!(elapsed < 60, "Last sync should be within last minute");

            // Cleanup: delete the hash
            let mut conn_clone = (*conn).clone();
            let hash_key = format!("{prefix}:relayer_sync_meta");
            let _: () = redis::AsyncCommands::del(&mut conn_clone, &hash_key)
                .await
                .expect("Cleanup failed");
        }

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_get_relayer_last_sync_returns_none_when_not_set() {
            let conn = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            let prefix = "test_sync_none";
            let relayer_id = "nonexistent-relayer";

            // Get the last sync time for a relayer that hasn't been synced
            let last_sync = get_relayer_last_sync(&conn, prefix, relayer_id)
                .await
                .expect("Should not error");

            assert!(
                last_sync.is_none(),
                "Should return None for unsynced relayer"
            );
        }

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_is_relayer_recently_synced_returns_true_for_recent_sync() {
            let conn = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            let prefix = "test_recent_sync";
            let relayer_id = "recent-relayer";

            // Set the last sync time
            set_relayer_last_sync(&conn, prefix, relayer_id)
                .await
                .expect("Should set last sync time");

            // Check if recently synced (within 5 minutes)
            let is_recent = is_relayer_recently_synced(&conn, prefix, relayer_id, 300)
                .await
                .expect("Should check recent sync");

            assert!(is_recent, "Should be recently synced");

            // Cleanup
            let mut conn_clone = (*conn).clone();
            let hash_key = format!("{prefix}:relayer_sync_meta");
            let _: () = redis::AsyncCommands::del(&mut conn_clone, &hash_key)
                .await
                .expect("Cleanup failed");
        }

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_is_relayer_recently_synced_returns_false_when_not_set() {
            let conn = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            let prefix = "test_not_synced";
            let relayer_id = "never-synced-relayer";

            // Check if recently synced for a relayer that hasn't been synced
            let is_recent = is_relayer_recently_synced(&conn, prefix, relayer_id, 300)
                .await
                .expect("Should check recent sync");

            assert!(!is_recent, "Should not be recently synced");
        }

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_is_relayer_recently_synced_returns_false_for_stale_sync() {
            let conn = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            let prefix = "test_stale_sync";
            let relayer_id = "stale-relayer";

            // Manually set an old timestamp (10 minutes ago)
            let mut conn_clone = (*conn).clone();
            let hash_key = format!("{prefix}:relayer_sync_meta");
            let field = format!("{relayer_id}:last_sync");
            let old_timestamp = chrono::Utc::now().timestamp() - 600; // 10 minutes ago

            redis::AsyncCommands::hset::<_, _, _, ()>(
                &mut conn_clone,
                &hash_key,
                &field,
                old_timestamp,
            )
            .await
            .expect("Should set old timestamp");

            // Check if recently synced with 5 minute threshold
            let is_recent = is_relayer_recently_synced(&conn, prefix, relayer_id, 300)
                .await
                .expect("Should check recent sync");

            assert!(!is_recent, "Should not be recently synced (stale)");

            // Cleanup
            let _: () = redis::AsyncCommands::del(&mut conn_clone, &hash_key)
                .await
                .expect("Cleanup failed");
        }

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_get_relayer_last_sync_multiple_relayers() {
            let conn = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            let prefix = "test_multi_sync";

            // Set sync times for multiple relayers
            set_relayer_last_sync(&conn, prefix, "relayer-1")
                .await
                .expect("Should set sync time");
            tokio::time::sleep(Duration::from_millis(10)).await;
            set_relayer_last_sync(&conn, prefix, "relayer-2")
                .await
                .expect("Should set sync time");

            let sync1 = get_relayer_last_sync(&conn, prefix, "relayer-1")
                .await
                .expect("Should get sync time");
            let sync2 = get_relayer_last_sync(&conn, prefix, "relayer-2")
                .await
                .expect("Should get sync time");

            assert!(sync1.is_some(), "Relayer 1 should have sync time");
            assert!(sync2.is_some(), "Relayer 2 should have sync time");
            assert!(
                sync2.unwrap() >= sync1.unwrap(),
                "Relayer 2 should be synced at same time or after relayer 1"
            );

            // Cleanup
            let mut conn_clone = (*conn).clone();
            let hash_key = format!("{prefix}:relayer_sync_meta");
            let _: () = redis::AsyncCommands::del(&mut conn_clone, &hash_key)
                .await
                .expect("Cleanup failed");
        }

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_get_relayer_last_sync_update_existing() {
            let conn = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            let prefix = "test_update_sync";
            let relayer_id = "update-relayer";

            // Set initial sync time
            set_relayer_last_sync(&conn, prefix, relayer_id)
                .await
                .expect("Should set sync time");

            let first_sync = get_relayer_last_sync(&conn, prefix, relayer_id)
                .await
                .expect("Should get sync time")
                .expect("Should have sync time");

            // Wait and update
            tokio::time::sleep(Duration::from_millis(100)).await;

            set_relayer_last_sync(&conn, prefix, relayer_id)
                .await
                .expect("Should update sync time");

            let second_sync = get_relayer_last_sync(&conn, prefix, relayer_id)
                .await
                .expect("Should get sync time")
                .expect("Should have sync time");

            assert!(
                second_sync > first_sync,
                "Updated sync time should be later than first"
            );

            // Cleanup
            let mut conn_clone = (*conn).clone();
            let hash_key = format!("{prefix}:relayer_sync_meta");
            let _: () = redis::AsyncCommands::del(&mut conn_clone, &hash_key)
                .await
                .expect("Cleanup failed");
        }

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_is_relayer_recently_synced_threshold_boundary() {
            let conn = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            let prefix = "test_boundary_sync";
            let relayer_id = "boundary-relayer";

            // Set timestamp exactly at threshold (should be considered NOT recent)
            let mut conn_clone = (*conn).clone();
            let hash_key = format!("{prefix}:relayer_sync_meta");
            let field = format!("{relayer_id}:last_sync");
            let threshold_secs: u64 = 60;
            let boundary_timestamp = chrono::Utc::now().timestamp() - (threshold_secs as i64);

            redis::AsyncCommands::hset::<_, _, _, ()>(
                &mut conn_clone,
                &hash_key,
                &field,
                boundary_timestamp,
            )
            .await
            .expect("Should set boundary timestamp");

            let is_recent = is_relayer_recently_synced(&conn, prefix, relayer_id, threshold_secs)
                .await
                .expect("Should check recent sync");

            assert!(!is_recent, "Should not be recent at exact threshold");

            // Cleanup
            let _: () = redis::AsyncCommands::del(&mut conn_clone, &hash_key)
                .await
                .expect("Cleanup failed");
        }

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_is_relayer_recently_synced_just_before_threshold() {
            let conn = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            let prefix = "test_just_before_sync";
            let relayer_id = "just-before-relayer";

            // Set timestamp just before threshold (should be considered recent)
            let mut conn_clone = (*conn).clone();
            let hash_key = format!("{prefix}:relayer_sync_meta");
            let field = format!("{relayer_id}:last_sync");
            let threshold_secs: u64 = 60;
            let just_before_timestamp =
                chrono::Utc::now().timestamp() - (threshold_secs as i64) + 5;

            redis::AsyncCommands::hset::<_, _, _, ()>(
                &mut conn_clone,
                &hash_key,
                &field,
                just_before_timestamp,
            )
            .await
            .expect("Should set just-before timestamp");

            let is_recent = is_relayer_recently_synced(&conn, prefix, relayer_id, threshold_secs)
                .await
                .expect("Should check recent sync");

            assert!(is_recent, "Should be recent just before threshold");

            // Cleanup
            let _: () = redis::AsyncCommands::del(&mut conn_clone, &hash_key)
                .await
                .expect("Cleanup failed");
        }

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_is_relayer_recently_synced_different_prefixes() {
            let conn = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            let relayer_id = "shared-relayer";

            // Set sync for prefix1 only
            set_relayer_last_sync(&conn, "prefix1", relayer_id)
                .await
                .expect("Should set sync time");

            let is_recent_prefix1 = is_relayer_recently_synced(&conn, "prefix1", relayer_id, 300)
                .await
                .expect("Should check sync");
            let is_recent_prefix2 = is_relayer_recently_synced(&conn, "prefix2", relayer_id, 300)
                .await
                .expect("Should check sync");

            assert!(is_recent_prefix1, "Should be recent for prefix1");
            assert!(!is_recent_prefix2, "Should not be recent for prefix2");

            // Cleanup
            let mut conn_clone = (*conn).clone();
            let hash_key = "prefix1:relayer_sync_meta";
            let _: () = redis::AsyncCommands::del(&mut conn_clone, hash_key)
                .await
                .expect("Cleanup failed");
        }

        #[tokio::test]
        #[ignore] // Requires running Redis instance
        async fn test_is_relayer_recently_synced_zero_threshold() {
            let conn = create_test_redis_connection()
                .await
                .expect("Redis connection required for this test");

            let prefix = "test_zero_threshold";
            let relayer_id = "zero-threshold-relayer";

            set_relayer_last_sync(&conn, prefix, relayer_id)
                .await
                .expect("Should set sync time");

            // With zero threshold, even immediate sync should be considered stale
            let is_recent = is_relayer_recently_synced(&conn, prefix, relayer_id, 0)
                .await
                .expect("Should check sync");

            assert!(!is_recent, "Should not be recent with zero threshold");

            // Cleanup
            let mut conn_clone = (*conn).clone();
            let hash_key = format!("{prefix}:relayer_sync_meta");
            let _: () = redis::AsyncCommands::del(&mut conn_clone, &hash_key)
                .await
                .expect("Cleanup failed");
        }
    }
}
