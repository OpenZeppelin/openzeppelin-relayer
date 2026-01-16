//! Connection pool for Unix socket communication with the pool server.
//!
//! Provides efficient connection reuse with:
//! - Lock-free connection acquisition via crossbeam SegQueue
//! - Semaphore-based concurrency limiting
//! - RAII connection guards for automatic cleanup

use crossbeam::queue::SegQueue;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Semaphore;

use super::config::get_config;
use super::protocol::{PoolRequest, PoolResponse};
use super::PluginError;

/// A single connection to the pool server
pub struct PoolConnection {
    stream: UnixStream,
    /// Connection ID for tracking
    id: usize,
    /// Health status of this connection
    healthy: AtomicBool,
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
                    return Ok(Self {
                        stream,
                        id,
                        healthy: AtomicBool::new(true),
                    });
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

        serde_json::from_str(&line)
            .map_err(|e| PluginError::PluginError(format!("Failed to parse response: {e}")))
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

    /// Check if connection is healthy
    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    /// Mark connection as unhealthy
    pub fn mark_unhealthy(&self) {
        self.healthy.store(false, Ordering::Relaxed);
    }
}

/// Connection pool for reusing socket connections.
/// Uses lock-free SegQueue for O(1) connection acquisition and release.
pub struct ConnectionPool {
    socket_path: String,
    /// Available connections (lock-free stack)
    available: Arc<SegQueue<PoolConnection>>,
    /// Maximum number of connections
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
            available: Arc::new(SegQueue::new()),
            max_connections,
            next_id: Arc::new(AtomicUsize::new(0)),
            semaphore: Arc::new(Semaphore::new(max_connections)),
        }
    }

    /// Get a connection from the pool or create a new one.
    /// Uses semaphore for proper concurrency limiting.
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

        // O(1) lock-free pop from available connections
        loop {
            match self.available.pop() {
                Some(conn) => {
                    if conn.is_healthy() {
                        tracing::debug!(connection_id = conn.id(), "Reusing connection from pool");
                        return Ok(PooledConnection {
                            conn: Some(conn),
                            pool: self,
                            _permit: permit,
                        });
                    }
                    tracing::debug!(connection_id = conn.id(), "Dropping unhealthy connection");
                    continue;
                }
                None => {
                    let id = self.next_id.fetch_add(1, Ordering::Relaxed);
                    tracing::debug!(connection_id = id, "Creating new pool connection");

                    let conn = PoolConnection::new(&self.socket_path, id).await?;

                    return Ok(PooledConnection {
                        conn: Some(conn),
                        pool: self,
                        _permit: permit,
                    });
                }
            }
        }
    }

    /// Convenience method for acquiring without pre-acquired permit
    pub async fn acquire(&self) -> Result<PooledConnection<'_>, PluginError> {
        self.acquire_with_permit(None).await
    }

    /// Return a connection to the pool
    pub fn release(&self, conn: PoolConnection) {
        let conn_id = conn.id();
        let is_healthy = conn.is_healthy();

        if is_healthy {
            self.available.push(conn);
            tracing::debug!(connection_id = conn_id, "Connection returned to pool");
        } else {
            tracing::debug!(connection_id = conn_id, "Connection dropped (unhealthy)");
        }
    }

    /// Clear all connections
    pub async fn clear(&self) {
        while self.available.pop().is_some() {}
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
            match conn.send_request_with_timeout(request, timeout_secs).await {
                Ok(response) => Ok(response),
                Err(e) => {
                    if let Some(ref conn) = self.conn {
                        conn.mark_unhealthy();
                    }
                    Err(e)
                }
            }
        } else {
            Err(PluginError::PluginError(
                "Connection already released".to_string(),
            ))
        }
    }

    /// Mark this connection as unhealthy (won't be returned to pool)
    pub fn mark_unhealthy(&mut self) {
        if let Some(ref conn) = self.conn {
            conn.mark_unhealthy();
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

    #[test]
    fn test_connection_pool_creation() {
        let pool = ConnectionPool::new("/tmp/test.sock".to_string(), 10);
        assert!(pool.available.is_empty());
    }

    #[tokio::test]
    async fn test_connection_pool_clear() {
        let pool = ConnectionPool::new("/tmp/test.sock".to_string(), 10);
        pool.clear().await;
        assert!(pool.available.is_empty());
    }

    #[tokio::test]
    async fn test_connection_pool_semaphore_limits() {
        let pool = ConnectionPool::new("/tmp/test.sock".to_string(), 2);

        let permit1 = pool.semaphore.clone().try_acquire_owned();
        assert!(permit1.is_ok());

        let permit2 = pool.semaphore.clone().try_acquire_owned();
        assert!(permit2.is_ok());

        let permit3 = pool.semaphore.clone().try_acquire_owned();
        assert!(permit3.is_err());
    }

    #[test]
    fn test_pool_connection_health_state() {
        let health = std::sync::atomic::AtomicBool::new(true);
        assert!(health.load(Ordering::Relaxed));

        health.store(false, Ordering::Relaxed);
        assert!(!health.load(Ordering::Relaxed));
    }
}
