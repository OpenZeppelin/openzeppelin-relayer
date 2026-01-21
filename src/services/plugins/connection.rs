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

    #[test]
    fn test_connection_pool_creation() {
        let pool = ConnectionPool::new("/tmp/test.sock".to_string(), 10);
        // Verify semaphore has correct number of permits
        assert_eq!(pool.semaphore.available_permits(), 10);
    }

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

    #[test]
    fn test_connection_id_increment() {
        let pool = ConnectionPool::new("/tmp/test.sock".to_string(), 10);
        assert_eq!(pool.next_connection_id(), 0);
        assert_eq!(pool.next_connection_id(), 1);
        assert_eq!(pool.next_connection_id(), 2);
    }
}
