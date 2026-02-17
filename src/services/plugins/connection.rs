//! Connection management for Unix socket communication with the pool server.
//!
//! Provides:
//! - Fresh connection per request (prevents response mixing under high concurrency)
//! - Semaphore-based concurrency limiting
//! - RAII connection guards for automatic cleanup

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Semaphore;

use super::config::get_config;
use super::protocol::{PoolRequest, PoolResponse};
use super::PluginError;

/// A single connection to the pool server (single-use, not pooled)
pub struct PoolConnection {
    stream: UnixStream,
    /// Connection ID for tracking/debugging
    id: usize,
}

impl PoolConnection {
    pub async fn new(socket_path: &str, id: usize) -> Result<Self, PluginError> {
        let max_attempts = get_config().pool_connect_retries;
        let mut attempts = 0;
        let mut delay_ms = 10u64;

        tracing::debug!(connection_id = id, socket_path = %socket_path, "Connecting to pool server");

        loop {
            match UnixStream::connect(socket_path).await {
                Ok(stream) => {
                    if attempts > 0 {
                        tracing::debug!(
                            connection_id = id,
                            attempts = attempts,
                            "Connected to pool server after retries"
                        );
                    }
                    return Ok(Self { stream, id });
                }
                Err(e) => {
                    attempts += 1;

                    if attempts >= max_attempts {
                        return Err(PluginError::SocketError(format!(
                            "Failed to connect to pool after {max_attempts} attempts: {e}. \
                            Consider increasing PLUGIN_POOL_CONNECT_RETRIES or PLUGIN_POOL_MAX_CONNECTIONS."
                        )));
                    }

                    if attempts <= 3 || attempts % 5 == 0 {
                        tracing::debug!(
                            connection_id = id,
                            attempt = attempts,
                            max_attempts = max_attempts,
                            delay_ms = delay_ms,
                            "Retrying connection to pool server"
                        );
                    }

                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    delay_ms = std::cmp::min(delay_ms * 2, 1000);
                }
            }
        }
    }

    pub async fn send_request(
        &mut self,
        request: &PoolRequest,
    ) -> Result<PoolResponse, PluginError> {
        // Extract task_id from request for validation
        let request_task_id = Self::extract_task_id(request);

        let json = serde_json::to_string(request)
            .map_err(|e| PluginError::PluginError(format!("Failed to serialize request: {e}")))?;

        if let Err(e) = self.stream.write_all(format!("{json}\n").as_bytes()).await {
            return Err(PluginError::SocketError(format!(
                "Failed to send request: {e}"
            )));
        }

        if let Err(e) = self.stream.flush().await {
            return Err(PluginError::SocketError(format!(
                "Failed to flush request: {e}"
            )));
        }

        let mut reader = BufReader::new(&mut self.stream);
        let mut line = String::new();

        if let Err(e) = reader.read_line(&mut line).await {
            return Err(PluginError::SocketError(format!(
                "Failed to read response: {e}"
            )));
        }

        tracing::debug!(response_len = line.len(), "Received response from pool");

        let response: PoolResponse = serde_json::from_str(&line)
            .map_err(|e| PluginError::PluginError(format!("Failed to parse response: {e}")))?;

        // CRITICAL: Validate that response task_id matches request task_id
        if response.task_id != request_task_id {
            tracing::error!(
                request_task_id = %request_task_id,
                response_task_id = %response.task_id,
                connection_id = self.id,
                "Response task_id mismatch"
            );
            return Err(PluginError::PluginError(
                "Internal plugin error: response task_id mismatch".to_string(),
            ));
        }

        Ok(response)
    }

    /// Extract task_id from any PoolRequest variant
    fn extract_task_id(request: &PoolRequest) -> String {
        match request {
            PoolRequest::Execute(req) => req.task_id.clone(),
            PoolRequest::Precompile { task_id, .. } => task_id.clone(),
            PoolRequest::Cache { task_id, .. } => task_id.clone(),
            PoolRequest::Invalidate { task_id, .. } => task_id.clone(),
            PoolRequest::Stats { task_id } => task_id.clone(),
            PoolRequest::Health { task_id } => task_id.clone(),
            PoolRequest::Shutdown { task_id } => task_id.clone(),
        }
    }

    pub async fn send_request_with_timeout(
        &mut self,
        request: &PoolRequest,
        timeout_secs: u64,
    ) -> Result<PoolResponse, PluginError> {
        tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            self.send_request(request),
        )
        .await
        .map_err(|_| PluginError::SocketError("Request timed out".to_string()))?
    }

    /// Get the connection ID
    pub fn id(&self) -> usize {
        self.id
    }
}

/// Connection manager for Unix socket connections.
///
/// Creates fresh connections per request (pooling disabled to prevent response mixing).
/// Uses semaphore to limit concurrent connections.
pub struct ConnectionPool {
    socket_path: String,
    /// Maximum number of connections (used for logging)
    #[allow(dead_code)]
    max_connections: usize,
    /// Next connection ID (atomic for lock-free increment)
    next_id: Arc<AtomicUsize>,
    /// Semaphore for limiting concurrent connections
    pub semaphore: Arc<Semaphore>,
}

impl ConnectionPool {
    pub fn new(socket_path: String, max_connections: usize) -> Self {
        Self {
            socket_path,
            max_connections,
            next_id: Arc::new(AtomicUsize::new(0)),
            semaphore: Arc::new(Semaphore::new(max_connections)),
        }
    }

    /// Acquire a connection. Always creates a fresh connection to prevent response mixing.
    /// Uses semaphore for concurrency limiting.
    /// Accepts optional pre-acquired permit for fast path optimization.
    pub async fn acquire_with_permit(
        &self,
        permit: Option<tokio::sync::OwnedSemaphorePermit>,
    ) -> Result<PooledConnection<'_>, PluginError> {
        let permit = match permit {
            Some(p) => p,
            None => {
                let available_permits = self.semaphore.available_permits();
                if available_permits == 0 {
                    tracing::warn!(
                        max_connections = self.max_connections,
                        "All connection permits exhausted - waiting for connection"
                    );
                }
                self.semaphore.clone().acquire_owned().await.map_err(|_| {
                    PluginError::PluginError("Connection semaphore closed".to_string())
                })?
            }
        };

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(connection_id = id, "Creating connection");

        let conn = PoolConnection::new(&self.socket_path, id).await?;

        Ok(PooledConnection {
            conn: Some(conn),
            pool: self,
            _permit: permit,
        })
    }

    /// Convenience method for acquiring without pre-acquired permit
    pub async fn acquire(&self) -> Result<PooledConnection<'_>, PluginError> {
        self.acquire_with_permit(None).await
    }

    /// Release a connection (closes the socket)
    pub fn release(&self, conn: PoolConnection) {
        let conn_id = conn.id();
        tracing::debug!(connection_id = conn_id, "Connection closed");
        drop(conn);
    }

    /// Get the next connection ID from the atomic counter.
    /// This is useful for creating connections outside the pool (e.g., shutdown requests).
    pub fn next_connection_id(&self) -> usize {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

/// RAII wrapper that returns connection to pool on drop
pub struct PooledConnection<'a> {
    conn: Option<PoolConnection>,
    pool: &'a ConnectionPool,
    /// Semaphore permit - released when dropped
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl<'a> PooledConnection<'a> {
    pub async fn send_request_with_timeout(
        &mut self,
        request: &PoolRequest,
        timeout_secs: u64,
    ) -> Result<PoolResponse, PluginError> {
        if let Some(ref mut conn) = self.conn {
            conn.send_request_with_timeout(request, timeout_secs).await
        } else {
            Err(PluginError::PluginError(
                "Connection already released".to_string(),
            ))
        }
    }
}

impl<'a> Drop for PooledConnection<'a> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.release(conn);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::plugins::protocol::ExecuteRequest;

    // ============================================
    // ConnectionPool creation tests
    // ============================================

    #[test]
    fn test_connection_pool_creation() {
        let pool = ConnectionPool::new("/tmp/test.sock".to_string(), 10);
        // Verify semaphore has correct number of permits
        assert_eq!(pool.semaphore.available_permits(), 10);
    }

    #[test]
    fn test_connection_pool_creation_single_connection() {
        let pool = ConnectionPool::new("/tmp/single.sock".to_string(), 1);
        assert_eq!(pool.semaphore.available_permits(), 1);
    }

    #[test]
    fn test_connection_pool_creation_large_pool() {
        let pool = ConnectionPool::new("/tmp/large.sock".to_string(), 1000);
        assert_eq!(pool.semaphore.available_permits(), 1000);
    }

    #[test]
    fn test_connection_pool_stores_socket_path() {
        let path = "/var/run/custom.sock";
        let pool = ConnectionPool::new(path.to_string(), 5);
        assert_eq!(pool.socket_path, path);
    }

    #[test]
    fn test_connection_pool_stores_max_connections() {
        let pool = ConnectionPool::new("/tmp/test.sock".to_string(), 42);
        assert_eq!(pool.max_connections, 42);
    }

    // ============================================
    // Semaphore tests
    // ============================================

    #[tokio::test]
    async fn test_connection_pool_semaphore_limits() {
        let pool = ConnectionPool::new("/tmp/test.sock".to_string(), 2);

        let permit1 = pool.semaphore.clone().try_acquire_owned();
        assert!(permit1.is_ok());

        let permit2 = pool.semaphore.clone().try_acquire_owned();
        assert!(permit2.is_ok());

        // Third permit should fail - all permits exhausted
        let permit3 = pool.semaphore.clone().try_acquire_owned();
        assert!(permit3.is_err());
    }

    #[tokio::test]
    async fn test_semaphore_permit_release_restores_capacity() {
        let pool = ConnectionPool::new("/tmp/test.sock".to_string(), 2);

        // Acquire all permits
        let permit1 = pool.semaphore.clone().try_acquire_owned().unwrap();
        let permit2 = pool.semaphore.clone().try_acquire_owned().unwrap();

        // No permits available
        assert_eq!(pool.semaphore.available_permits(), 0);

        // Drop one permit
        drop(permit1);

        // One permit available again
        assert_eq!(pool.semaphore.available_permits(), 1);

        // Can acquire again
        let permit3 = pool.semaphore.clone().try_acquire_owned();
        assert!(permit3.is_ok());

        // Drop remaining permits
        drop(permit2);
        drop(permit3.unwrap());

        // All permits restored
        assert_eq!(pool.semaphore.available_permits(), 2);
    }

    #[tokio::test]
    async fn test_semaphore_async_acquire() {
        let pool = ConnectionPool::new("/tmp/test.sock".to_string(), 1);

        // Acquire the only permit
        let permit = pool.semaphore.clone().acquire_owned().await;
        assert!(permit.is_ok());
        let _permit = permit.unwrap();

        // Verify no permits available
        assert_eq!(pool.semaphore.available_permits(), 0);
    }

    // ============================================
    // Connection ID tests
    // ============================================

    #[test]
    fn test_connection_id_increment() {
        let pool = ConnectionPool::new("/tmp/test.sock".to_string(), 10);
        assert_eq!(pool.next_connection_id(), 0);
        assert_eq!(pool.next_connection_id(), 1);
        assert_eq!(pool.next_connection_id(), 2);
    }

    #[test]
    fn test_connection_id_starts_at_zero() {
        let pool = ConnectionPool::new("/tmp/test.sock".to_string(), 10);
        assert_eq!(pool.next_connection_id(), 0);
    }

    #[test]
    fn test_connection_id_monotonically_increasing() {
        let pool = ConnectionPool::new("/tmp/test.sock".to_string(), 10);

        let mut last_id = pool.next_connection_id();
        for _ in 0..100 {
            let current_id = pool.next_connection_id();
            assert!(
                current_id > last_id,
                "IDs should be monotonically increasing"
            );
            last_id = current_id;
        }
    }

    #[test]
    fn test_connection_id_thread_safe() {
        use std::thread;

        let pool = Arc::new(ConnectionPool::new("/tmp/test.sock".to_string(), 100));
        let mut handles = vec![];

        // Spawn multiple threads getting connection IDs
        for _ in 0..10 {
            let pool_clone = pool.clone();
            handles.push(thread::spawn(move || {
                let mut ids = vec![];
                for _ in 0..100 {
                    ids.push(pool_clone.next_connection_id());
                }
                ids
            }));
        }

        // Collect all IDs
        let mut all_ids: Vec<usize> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();

        // Sort and verify uniqueness
        all_ids.sort();
        let unique_count = all_ids.windows(2).filter(|w| w[0] != w[1]).count() + 1;
        assert_eq!(unique_count, all_ids.len(), "All IDs should be unique");
    }

    // ============================================
    // extract_task_id tests
    // ============================================

    #[test]
    fn test_extract_task_id_from_execute_request() {
        let request = PoolRequest::Execute(Box::new(ExecuteRequest {
            task_id: "execute-task-123".to_string(),
            plugin_id: "test-plugin".to_string(),
            compiled_code: None,
            plugin_path: None,
            params: serde_json::json!({}),
            headers: None,
            socket_path: "/tmp/test.sock".to_string(),
            http_request_id: None,
            timeout: Some(30000),
            route: None,
            config: None,
            method: None,
            query: None,
        }));

        let task_id = PoolConnection::extract_task_id(&request);
        assert_eq!(task_id, "execute-task-123");
    }

    #[test]
    fn test_extract_task_id_from_precompile_request() {
        let request = PoolRequest::Precompile {
            task_id: "precompile-task-456".to_string(),
            plugin_id: "test-plugin".to_string(),
            plugin_path: Some("/path/to/plugin.ts".to_string()),
            source_code: None,
        };

        let task_id = PoolConnection::extract_task_id(&request);
        assert_eq!(task_id, "precompile-task-456");
    }

    #[test]
    fn test_extract_task_id_from_cache_request() {
        let request = PoolRequest::Cache {
            task_id: "cache-task-789".to_string(),
            plugin_id: "test-plugin".to_string(),
            compiled_code: "compiled code".to_string(),
        };

        let task_id = PoolConnection::extract_task_id(&request);
        assert_eq!(task_id, "cache-task-789");
    }

    #[test]
    fn test_extract_task_id_from_invalidate_request() {
        let request = PoolRequest::Invalidate {
            task_id: "invalidate-task-abc".to_string(),
            plugin_id: "test-plugin".to_string(),
        };

        let task_id = PoolConnection::extract_task_id(&request);
        assert_eq!(task_id, "invalidate-task-abc");
    }

    #[test]
    fn test_extract_task_id_from_stats_request() {
        let request = PoolRequest::Stats {
            task_id: "stats-task-def".to_string(),
        };

        let task_id = PoolConnection::extract_task_id(&request);
        assert_eq!(task_id, "stats-task-def");
    }

    #[test]
    fn test_extract_task_id_from_health_request() {
        let request = PoolRequest::Health {
            task_id: "health-task-ghi".to_string(),
        };

        let task_id = PoolConnection::extract_task_id(&request);
        assert_eq!(task_id, "health-task-ghi");
    }

    #[test]
    fn test_extract_task_id_from_shutdown_request() {
        let request = PoolRequest::Shutdown {
            task_id: "shutdown-task-jkl".to_string(),
        };

        let task_id = PoolConnection::extract_task_id(&request);
        assert_eq!(task_id, "shutdown-task-jkl");
    }

    #[test]
    fn test_extract_task_id_preserves_special_characters() {
        let request = PoolRequest::Stats {
            task_id: "task-with-special_chars.and/slashes:colons".to_string(),
        };

        let task_id = PoolConnection::extract_task_id(&request);
        assert_eq!(task_id, "task-with-special_chars.and/slashes:colons");
    }

    #[test]
    fn test_extract_task_id_handles_empty_string() {
        let request = PoolRequest::Health {
            task_id: "".to_string(),
        };

        let task_id = PoolConnection::extract_task_id(&request);
        assert_eq!(task_id, "");
    }

    #[test]
    fn test_extract_task_id_handles_uuid_format() {
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        let request = PoolRequest::Stats {
            task_id: uuid.to_string(),
        };

        let task_id = PoolConnection::extract_task_id(&request);
        assert_eq!(task_id, uuid);
    }

    // ============================================
    // acquire_with_permit tests
    // ============================================

    #[tokio::test]
    async fn test_acquire_without_server_fails() {
        let pool = ConnectionPool::new("/tmp/nonexistent_socket_12345.sock".to_string(), 10);

        let result = pool.acquire().await;
        assert!(result.is_err());

        match result {
            Err(PluginError::SocketError(msg)) => {
                assert!(msg.contains("Failed to connect"));
            }
            _ => panic!("Expected SocketError"),
        }
    }

    #[tokio::test]
    async fn test_acquire_with_pre_acquired_permit() {
        let pool = ConnectionPool::new("/tmp/nonexistent_socket_67890.sock".to_string(), 10);

        // Pre-acquire a permit
        let permit = pool.semaphore.clone().acquire_owned().await.unwrap();
        assert_eq!(pool.semaphore.available_permits(), 9);

        // Try to acquire with pre-acquired permit (will fail due to no server, but permit logic works)
        let result = pool.acquire_with_permit(Some(permit)).await;

        // Connection fails but permit was used
        assert!(result.is_err());
    }

    // ============================================
    // PooledConnection tests
    // ============================================

    #[test]
    fn test_pooled_connection_cannot_be_used_after_release() {
        // This tests the Option<PoolConnection> pattern - we can't easily
        // test this without a live connection, but we document the behavior
        // that send_request_with_timeout returns error when conn is None
    }

    // ============================================
    // Error message tests
    // ============================================

    #[tokio::test]
    async fn test_acquire_error_message_contains_helpful_info() {
        let pool = ConnectionPool::new("/tmp/no_server_here_xyz.sock".to_string(), 10);

        let result = pool.acquire().await;
        assert!(result.is_err());

        if let Err(PluginError::SocketError(msg)) = result {
            // Verify error message contains helpful suggestions
            assert!(
                msg.contains("PLUGIN_POOL_CONNECT_RETRIES")
                    || msg.contains("PLUGIN_POOL_MAX_CONNECTIONS")
                    || msg.contains("Failed to connect"),
                "Error message should contain helpful info: {msg}"
            );
        }
    }

    // ============================================
    // Multiple pool instances tests
    // ============================================

    #[test]
    fn test_multiple_pools_independent() {
        let pool1 = ConnectionPool::new("/tmp/pool1.sock".to_string(), 5);
        let pool2 = ConnectionPool::new("/tmp/pool2.sock".to_string(), 10);

        // Each pool has its own semaphore
        assert_eq!(pool1.semaphore.available_permits(), 5);
        assert_eq!(pool2.semaphore.available_permits(), 10);

        // Each pool has its own connection ID counter
        assert_eq!(pool1.next_connection_id(), 0);
        assert_eq!(pool2.next_connection_id(), 0);
        assert_eq!(pool1.next_connection_id(), 1);
        assert_eq!(pool2.next_connection_id(), 1);
    }

    // ============================================
    // Concurrent access tests
    // ============================================

    #[tokio::test]
    async fn test_concurrent_semaphore_acquire() {
        let pool = Arc::new(ConnectionPool::new("/tmp/concurrent.sock".to_string(), 3));

        let mut handles = vec![];

        // Spawn tasks that try to acquire permits
        for i in 0..3 {
            let pool_clone = pool.clone();
            handles.push(tokio::spawn(async move {
                let permit = pool_clone.semaphore.clone().acquire_owned().await;
                assert!(permit.is_ok(), "Task {i} should acquire permit");
                // Hold permit briefly
                tokio::time::sleep(Duration::from_millis(10)).await;
            }));
        }

        // All tasks should complete successfully
        for handle in handles {
            handle.await.unwrap();
        }

        // All permits should be released
        assert_eq!(pool.semaphore.available_permits(), 3);
    }

    #[tokio::test]
    async fn test_semaphore_fairness() {
        use std::sync::atomic::AtomicU32;

        let pool = Arc::new(ConnectionPool::new("/tmp/fairness.sock".to_string(), 1));
        let counter = Arc::new(AtomicU32::new(0));

        // Acquire the only permit
        let permit = pool.semaphore.clone().acquire_owned().await.unwrap();

        let mut handles = vec![];

        // Spawn waiting tasks
        for _ in 0..3 {
            let pool_clone = pool.clone();
            let counter_clone = counter.clone();
            handles.push(tokio::spawn(async move {
                let _permit = pool_clone.semaphore.clone().acquire_owned().await.unwrap();
                counter_clone.fetch_add(1, Ordering::SeqCst);
            }));
        }

        // Give tasks time to start waiting
        tokio::time::sleep(Duration::from_millis(50)).await;

        // No task should have completed yet
        assert_eq!(counter.load(Ordering::SeqCst), 0);

        // Release the permit
        drop(permit);

        // Wait for all tasks
        for handle in handles {
            handle.await.unwrap();
        }

        // All tasks should have completed
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    // ============================================
    // Edge cases
    // ============================================

    #[test]
    fn test_zero_max_connections_creates_closed_semaphore() {
        let pool = ConnectionPool::new("/tmp/zero.sock".to_string(), 0);
        assert_eq!(pool.semaphore.available_permits(), 0);

        // Can't acquire any permits
        let permit = pool.semaphore.clone().try_acquire_owned();
        assert!(permit.is_err());
    }

    #[test]
    fn test_socket_path_with_spaces() {
        let path = "/tmp/path with spaces/test.sock";
        let pool = ConnectionPool::new(path.to_string(), 5);
        assert_eq!(pool.socket_path, path);
    }

    #[test]
    fn test_socket_path_with_unicode() {
        let path = "/tmp/тест/套接字.sock";
        let pool = ConnectionPool::new(path.to_string(), 5);
        assert_eq!(pool.socket_path, path);
    }

    #[test]
    fn test_very_long_socket_path() {
        let path = format!("/tmp/{}/test.sock", "a".repeat(200));
        let pool = ConnectionPool::new(path.clone(), 5);
        assert_eq!(pool.socket_path, path);
    }
}
