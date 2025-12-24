//! Pool-based Plugin Executor
//!
//! This module provides execution of pre-compiled JavaScript plugins via
//! a persistent Piscina worker pool, replacing the per-request ts-node approach.
//!
//! Communication with the Node.js pool server happens via Unix socket using
//! a JSON-line protocol.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, RwLock, Semaphore};
use uuid::Uuid;

use super::{LogEntry, LogLevel, PluginError, PluginHandlerPayload, ScriptResult};
use crate::constants::{
    DEFAULT_POOL_MAX_CONNECTIONS, DEFAULT_POOL_MAX_QUEUE_SIZE, DEFAULT_POOL_REQUEST_TIMEOUT_SECS,
};

/// Health status information from the pool server
#[derive(Debug, Clone)]
pub struct HealthStatus {
    pub healthy: bool,
    pub status: String,
    pub uptime_ms: Option<u64>,
    pub memory: Option<u64>,
    pub pool_completed: Option<u64>,
    pub pool_queued: Option<u64>,
    pub success_rate: Option<f64>,
}

/// Message types for pool communication
#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PoolRequest {
    Execute {
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(rename = "pluginId")]
        plugin_id: String,
        #[serde(rename = "compiledCode", skip_serializing_if = "Option::is_none")]
        compiled_code: Option<String>,
        #[serde(rename = "pluginPath", skip_serializing_if = "Option::is_none")]
        plugin_path: Option<String>,
        params: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        headers: Option<HashMap<String, Vec<String>>>,
        #[serde(rename = "socketPath")]
        socket_path: String,
        #[serde(rename = "httpRequestId", skip_serializing_if = "Option::is_none")]
        http_request_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
    },
    Precompile {
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(rename = "pluginId")]
        plugin_id: String,
        #[serde(rename = "pluginPath", skip_serializing_if = "Option::is_none")]
        plugin_path: Option<String>,
        #[serde(rename = "sourceCode", skip_serializing_if = "Option::is_none")]
        source_code: Option<String>,
    },
    Cache {
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(rename = "pluginId")]
        plugin_id: String,
        #[serde(rename = "compiledCode")]
        compiled_code: String,
    },
    Invalidate {
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(rename = "pluginId")]
        plugin_id: String,
    },
    Stats {
        #[serde(rename = "taskId")]
        task_id: String,
    },
    Health {
        #[serde(rename = "taskId")]
        task_id: String,
    },
    Shutdown {
        #[serde(rename = "taskId")]
        task_id: String,
    },
}

#[derive(Deserialize, Debug)]
pub struct PoolResponse {
    #[serde(rename = "taskId")]
    pub task_id: String,
    pub success: bool,
    pub result: Option<serde_json::Value>,
    pub error: Option<PoolError>,
    pub logs: Option<Vec<PoolLogEntry>>,
}

#[derive(Deserialize, Debug)]
pub struct PoolError {
    pub message: String,
    pub code: Option<String>,
    pub status: Option<u16>,
    pub details: Option<serde_json::Value>,
}

#[derive(Deserialize, Debug)]
pub struct PoolLogEntry {
    pub level: String,
    pub message: String,
}

impl From<PoolLogEntry> for LogEntry {
    fn from(entry: PoolLogEntry) -> Self {
        let level = match entry.level.as_str() {
            "error" => LogLevel::Error,
            "warn" => LogLevel::Warn,
            "info" => LogLevel::Info,
            "debug" => LogLevel::Debug,
            "result" => LogLevel::Result,
            _ => LogLevel::Log,
        };
        LogEntry {
            level,
            message: entry.message,
        }
    }
}

/// Pool connection state
struct PoolConnection {
    stream: UnixStream,
    /// Connection ID for tracking
    id: usize,
}

impl PoolConnection {
    async fn new(socket_path: &str, id: usize) -> Result<Self, PluginError> {
        // Retry connection with backoff - socket might not be ready immediately after READY signal
        let mut attempts = 0;
        let max_attempts = 10;
        let mut delay_ms = 10;

        tracing::debug!(connection_id = id, socket_path = %socket_path, "Connecting to pool server");
        loop {
            match UnixStream::connect(socket_path).await {
                Ok(stream) => {
                    tracing::debug!(connection_id = id, "Connected to pool server");
                    return Ok(Self { stream, id });
                }
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_attempts {
                        return Err(PluginError::SocketError(format!(
                            "Failed to connect to pool after {max_attempts} attempts: {e}"
                        )));
                    }
                    tracing::debug!(
                        connection_id = id,
                        attempt = attempts,
                        delay_ms = delay_ms,
                        "Retrying connection to pool server"
                    );
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    delay_ms = std::cmp::min(delay_ms * 2, 500);
                }
            }
        }
    }

    async fn send_request(&mut self, request: &PoolRequest) -> Result<PoolResponse, PluginError> {
        let json = serde_json::to_string(request)
            .map_err(|e| PluginError::PluginError(format!("Failed to serialize request: {e}")))?;

        // Write request and flush
        if let Err(e) = self.stream.write_all(format!("{json}\n").as_bytes()).await {
            return Err(PluginError::SocketError(format!("Failed to send request: {e}")));
        }

        if let Err(e) = self.stream.flush().await {
            return Err(PluginError::SocketError(format!("Failed to flush request: {e}")));
        }

        // Read response using BufReader on a reference to the stream
        let mut reader = BufReader::new(&mut self.stream);
        let mut line = String::new();

        if let Err(e) = reader.read_line(&mut line).await {
            return Err(PluginError::SocketError(format!("Failed to read response: {e}")));
        }

        tracing::debug!(response_len = line.len(), "Received response from pool");

        serde_json::from_str(&line)
            .map_err(|e| PluginError::PluginError(format!("Failed to parse response: {e}")))
    }

    /// Send request with timeout
    async fn send_request_with_timeout(
        &mut self,
        request: &PoolRequest,
        timeout_secs: u64,
    ) -> Result<PoolResponse, PluginError> {
        tokio::time::timeout(Duration::from_secs(timeout_secs), self.send_request(request))
            .await
            .map_err(|_| PluginError::SocketError("Request timed out".to_string()))?
    }
}

/// Connection pool for reusing socket connections
/// Uses DashMap for lock-free connection acquisition and release
struct ConnectionPool {
    socket_path: String,
    /// Available connections (connection_id -> connection)
    connections: Arc<DashMap<usize, PoolConnection>>,
    /// Health tracking for connections (connection_id -> is_healthy)
    health: Arc<DashMap<usize, bool>>,
    /// Maximum number of connections
    max_connections: usize,
    /// Next connection ID (atomic for lock-free increment)
    next_id: Arc<AtomicUsize>,
    /// Semaphore for limiting concurrent connections
    semaphore: Arc<Semaphore>,
}

impl ConnectionPool {
    fn new(socket_path: String, max_connections: usize) -> Self {
        Self {
            socket_path,
            connections: Arc::new(DashMap::new()),
            health: Arc::new(DashMap::new()),
            max_connections,
            next_id: Arc::new(AtomicUsize::new(0)),
            semaphore: Arc::new(Semaphore::new(max_connections)),
        }
    }

    /// Get a connection from the pool or create a new one
    /// Uses semaphore for proper concurrency limiting
    async fn acquire(&self) -> Result<PooledConnection<'_>, PluginError> {
        // Acquire permit first - this limits total concurrent connections
        let permit = self.semaphore.clone().acquire_owned().await.map_err(|_| {
            PluginError::PluginError("Connection semaphore closed".to_string())
        })?;

        // Try to find a healthy connection in the pool
        let mut candidate_ids = Vec::new();
        
        for entry in self.connections.iter() {
            let conn_id = *entry.key();
            let is_healthy = self.health.get(&conn_id).map(|h| *h.value()).unwrap_or(false);
            if is_healthy {
                candidate_ids.push(conn_id);
            }
        }
        
        // Try to remove one of the candidates
        for conn_id in candidate_ids {
            if let Some((_, conn)) = self.connections.remove(&conn_id) {
                tracing::debug!(connection_id = conn_id, "Reusing connection from pool");
                return Ok(PooledConnection {
                    conn: Some(conn),
                    pool: self,
                    _permit: permit,
                });
            }
        }
        
        // No available connection, create a new one
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(connection_id = id, "Creating new pool connection");
        
        let conn = PoolConnection::new(&self.socket_path, id).await?;
        self.health.insert(id, true);
        
        Ok(PooledConnection {
            conn: Some(conn),
            pool: self,
            _permit: permit,
        })
    }

    /// Return a connection to the pool
    fn release(&self, conn: PoolConnection) {
        let conn_id = conn.id;
        let is_healthy = self.health.get(&conn_id).map(|h| *h.value()).unwrap_or(false);
        let pool_size = self.connections.len();
        
        if is_healthy && pool_size < self.max_connections {
            self.connections.insert(conn_id, conn);
            tracing::debug!(connection_id = conn_id, "Connection returned to pool");
        } else {
            self.health.remove(&conn_id);
            tracing::debug!(connection_id = conn_id, "Connection dropped (unhealthy or pool full)");
        }
    }

    /// Mark a connection as unhealthy
    fn mark_unhealthy(&self, conn_id: usize) {
        self.health.insert(conn_id, false);
        self.connections.remove(&conn_id);
    }

    /// Clear all connections
    async fn clear(&self) {
        self.connections.clear();
        self.health.clear();
    }
}

/// RAII wrapper that returns connection to pool on drop
struct PooledConnection<'a> {
    conn: Option<PoolConnection>,
    pool: &'a ConnectionPool,
    /// Semaphore permit - released when dropped
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl<'a> PooledConnection<'a> {
    async fn send_request_with_timeout(
        &mut self,
        request: &PoolRequest,
        timeout_secs: u64,
    ) -> Result<PoolResponse, PluginError> {
        if let Some(ref mut conn) = self.conn {
            match conn.send_request_with_timeout(request, timeout_secs).await {
                Ok(response) => Ok(response),
                Err(e) => {
                    if let Some(conn_id) = self.conn.as_ref().map(|c| c.id) {
                        self.pool.mark_unhealthy(conn_id);
                    }
                    Err(e)
                }
            }
        } else {
            Err(PluginError::PluginError("Connection already released".to_string()))
        }
    }

    /// Mark this connection as unhealthy (won't be returned to pool)
    fn mark_unhealthy(&mut self) {
        if let Some(conn_id) = self.conn.as_ref().map(|c| c.id) {
            self.pool.mark_unhealthy(conn_id);
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

/// Request queue entry for throttling
struct QueuedRequest {
    plugin_id: String,
    compiled_code: Option<String>,
    plugin_path: Option<String>,
    params: serde_json::Value,
    headers: Option<HashMap<String, Vec<String>>>,
    socket_path: String,
    http_request_id: Option<String>,
    timeout_secs: Option<u64>,
    response_tx: oneshot::Sender<Result<ScriptResult, PluginError>>,
}

/// Manages the pool server process and connections
pub struct PoolManager {
    socket_path: String,
    process: tokio::sync::Mutex<Option<Child>>,
    initialized: RwLock<bool>,
    /// Connection pool for reusing connections
    connection_pool: Arc<ConnectionPool>,
    /// Request queue for throttling/backpressure (multi-consumer channel)
    request_tx: async_channel::Sender<QueuedRequest>,
    /// Actual configured queue size (for error messages)
    max_queue_size: usize,
}

impl PoolManager {
    /// Create a new PoolManager with default socket path
    pub fn new() -> Self {
        Self::init(format!("/tmp/relayer-plugin-pool-{}.sock", Uuid::new_v4()))
    }

    /// Create a new PoolManager with custom socket path
    pub fn with_socket_path(socket_path: String) -> Self {
        Self::init(socket_path)
    }

    /// Common initialization logic
    fn init(socket_path: String) -> Self {
        let max_connections = std::env::var("PLUGIN_POOL_MAX_CONNECTIONS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_POOL_MAX_CONNECTIONS);
        let max_queue_size = std::env::var("PLUGIN_POOL_MAX_QUEUE_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_POOL_MAX_QUEUE_SIZE);
        
        // Use async-channel for multi-consumer queue (no mutex needed)
        let (tx, rx) = async_channel::bounded(max_queue_size);
        
        let connection_pool = Arc::new(ConnectionPool::new(socket_path.clone(), max_connections));
        let connection_pool_clone = connection_pool.clone();
        
        // Spawn background workers to process queued requests
        Self::spawn_queue_workers(rx, connection_pool_clone);
        
        Self {
            connection_pool,
            socket_path,
            process: tokio::sync::Mutex::new(None),
            initialized: RwLock::new(false),
            request_tx: tx,
            max_queue_size,
        }
    }
    
    /// Spawn multiple worker tasks to process queued requests concurrently
    fn spawn_queue_workers(
        rx: async_channel::Receiver<QueuedRequest>,
        connection_pool: Arc<ConnectionPool>,
    ) {
        let num_workers = std::env::var("PLUGIN_POOL_WORKERS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map(|n| n.get().max(4).min(32))
                    .unwrap_or(8)
            });
        
        tracing::info!(num_workers = num_workers, "Starting request queue workers");
        
        for worker_id in 0..num_workers {
            let rx_clone = rx.clone();
            let pool_clone = connection_pool.clone();
            
            tokio::spawn(async move {
                while let Ok(request) = rx_clone.recv().await {
                    let result = Self::execute_plugin_internal(
                        &pool_clone,
                        request.plugin_id,
                        request.compiled_code,
                        request.plugin_path,
                        request.params,
                        request.headers,
                        request.socket_path,
                        request.http_request_id,
                        request.timeout_secs,
                    ).await;
                    
                    let _ = request.response_tx.send(result);
                }
                
                tracing::debug!(worker_id = worker_id, "Request queue worker exited");
            });
        }
    }
    
    /// Internal execution method
    async fn execute_plugin_internal(
        connection_pool: &Arc<ConnectionPool>,
        plugin_id: String,
        compiled_code: Option<String>,
        plugin_path: Option<String>,
        params: serde_json::Value,
        headers: Option<HashMap<String, Vec<String>>>,
        socket_path: String,
        http_request_id: Option<String>,
        timeout_secs: Option<u64>,
    ) -> Result<ScriptResult, PluginError> {
        let mut conn = connection_pool.acquire().await?;

        let request = PoolRequest::Execute {
            task_id: Uuid::new_v4().to_string(),
            plugin_id: plugin_id.clone(),
            compiled_code,
            plugin_path,
            params,
            headers,
            socket_path,
            http_request_id,
            timeout: timeout_secs.map(|s| s * 1000),
        };

        let timeout = timeout_secs.unwrap_or(DEFAULT_POOL_REQUEST_TIMEOUT_SECS);
        let response = conn.send_request_with_timeout(&request, timeout).await?;

        let logs: Vec<LogEntry> = response
            .logs
            .unwrap_or_default()
            .into_iter()
            .map(|l| l.into())
            .collect();

        if response.success {
            let return_value = response
                .result
                .map(|v| {
                    if v.is_string() {
                        v.as_str().unwrap_or("").to_string()
                    } else {
                        serde_json::to_string(&v).unwrap_or_default()
                    }
                })
                .unwrap_or_default();

            Ok(ScriptResult {
                logs,
                error: String::new(),
                return_value,
                trace: Vec::new(),
            })
        } else {
            let error = response.error.unwrap_or(PoolError {
                message: "Unknown error".to_string(),
                code: None,
                status: None,
                details: None,
            });

            Err(PluginError::HandlerError(Box::new(PluginHandlerPayload {
                message: error.message,
                status: error.status.unwrap_or(500),
                code: error.code,
                details: error.details,
                logs: Some(logs),
                traces: None,
            })))
        }
    }

    /// Start the pool server if not already running
    pub async fn ensure_started(&self) -> Result<(), PluginError> {
        // Fast path: check if already initialized
        if *self.initialized.read().await {
            return Ok(());
        }

        // Slow path: acquire write lock and start
        let mut initialized = self.initialized.write().await;
        if *initialized {
            return Ok(());
        }

        self.start_pool_server().await?;
        *initialized = true;
        Ok(())
    }
    
    /// Ensure pool is started and healthy, with auto-recovery on failure
    async fn ensure_started_and_healthy(&self) -> Result<(), PluginError> {
        self.ensure_started().await?;
        
        // Opportunistic health check - don't block on every request
        // Check health periodically or on connection errors
        // This is a lightweight check that only runs health_check if we detect issues
        if let Ok(health) = self.health_check().await {
            if !health.healthy {
                tracing::warn!(
                    status = %health.status,
                    "Pool server unhealthy, attempting automatic recovery"
                );
                // Try to restart
                if let Err(e) = self.restart().await {
                    tracing::error!(error = %e, "Failed to restart pool server");
                    return Err(PluginError::PluginExecutionError(format!(
                        "Pool server unhealthy and restart failed: {}", health.status
                    )));
                }
            }
        }
        Ok(())
    }

    async fn start_pool_server(&self) -> Result<(), PluginError> {
        let mut process_guard = self.process.lock().await;

        if process_guard.is_some() {
            return Ok(());
        }

        let pool_server_path = std::env::current_dir()
            .map(|cwd| cwd.join("plugins/lib/pool-server.ts").display().to_string())
            .unwrap_or_else(|_| "plugins/lib/pool-server.ts".to_string());

        tracing::info!(socket_path = %self.socket_path, "Starting plugin pool server");

        let mut child = Command::new("ts-node")
            .arg("--transpile-only")
            .arg(&pool_server_path)
            .arg(&self.socket_path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                PluginError::PluginExecutionError(format!("Failed to start pool server: {e}"))
            })?;

        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::error!(target: "pool_server", "{}", line);
                }
            });
        }

        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            let timeout = tokio::time::timeout(Duration::from_secs(10), async {
                while let Ok(Some(line)) = lines.next_line().await {
                    if line.contains("POOL_SERVER_READY") {
                        return Ok(());
                    }
                }
                Err(PluginError::PluginExecutionError(
                    "Pool server did not send ready signal".to_string(),
                ))
            })
            .await;

            match timeout {
                Ok(Ok(())) => {
                    tracing::info!("Plugin pool server ready");
                }
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    return Err(PluginError::PluginExecutionError(
                        "Timeout waiting for pool server to start".to_string(),
                    ))
                }
            }
        }

        *process_guard = Some(child);
        Ok(())
    }

    /// Execute a plugin via the pool
    /// Uses semaphore-based concurrency limiting with queue fallback
    pub async fn execute_plugin(
        &self,
        plugin_id: String,
        compiled_code: Option<String>,
        plugin_path: Option<String>,
        params: serde_json::Value,
        headers: Option<HashMap<String, Vec<String>>>,
        socket_path: String,
        http_request_id: Option<String>,
        timeout_secs: Option<u64>,
    ) -> Result<ScriptResult, PluginError> {
        // Ensure pool is started and healthy
        self.ensure_started_and_healthy().await?;

        // Try direct execution first (semaphore handles concurrency limiting)
        // If semaphore can't be acquired immediately, queue the request
        match self.connection_pool.semaphore.clone().try_acquire_owned() {
            Ok(permit) => {
                // Direct execution path - faster for normal load
                let mut conn = {
                    // Try to get pooled connection
                    let mut candidate_ids = Vec::new();
                    for entry in self.connection_pool.connections.iter() {
                        let conn_id = *entry.key();
                        let is_healthy = self.connection_pool.health.get(&conn_id).map(|h| *h.value()).unwrap_or(false);
                        if is_healthy {
                            candidate_ids.push(conn_id);
                        }
                    }
                    
                    let mut found_conn = None;
                    for conn_id in candidate_ids {
                        if let Some((_, conn)) = self.connection_pool.connections.remove(&conn_id) {
                            found_conn = Some(conn);
                            break;
                        }
                    }
                    
                    match found_conn {
                        Some(conn) => PooledConnection {
                            conn: Some(conn),
                            pool: &self.connection_pool,
                            _permit: permit,
                        },
                        None => {
                            let id = self.connection_pool.next_id.fetch_add(1, Ordering::Relaxed);
                            let conn = PoolConnection::new(&self.connection_pool.socket_path, id).await?;
                            self.connection_pool.health.insert(id, true);
                            PooledConnection {
                                conn: Some(conn),
                                pool: &self.connection_pool,
                                _permit: permit,
                            }
                        }
                    }
                };

                let request = PoolRequest::Execute {
                    task_id: Uuid::new_v4().to_string(),
                    plugin_id: plugin_id.clone(),
                    compiled_code,
                    plugin_path,
                    params,
                    headers,
                    socket_path,
                    http_request_id,
                    timeout: timeout_secs.map(|s| s * 1000),
                };

                let timeout = timeout_secs.unwrap_or(DEFAULT_POOL_REQUEST_TIMEOUT_SECS);
                let response = conn.send_request_with_timeout(&request, timeout).await?;

                let logs: Vec<LogEntry> = response
                    .logs
                    .unwrap_or_default()
                    .into_iter()
                    .map(|l| l.into())
                    .collect();

                if response.success {
                    let return_value = response
                        .result
                        .map(|v| {
                            if v.is_string() {
                                v.as_str().unwrap_or("").to_string()
                            } else {
                                serde_json::to_string(&v).unwrap_or_default()
                            }
                        })
                        .unwrap_or_default();

                    Ok(ScriptResult {
                        logs,
                        error: String::new(),
                        return_value,
                        trace: Vec::new(),
                    })
                } else {
                    let error = response.error.unwrap_or(PoolError {
                        message: "Unknown error".to_string(),
                        code: None,
                        status: None,
                        details: None,
                    });

                    Err(PluginError::HandlerError(Box::new(PluginHandlerPayload {
                        message: error.message,
                        status: error.status.unwrap_or(500),
                        code: error.code,
                        details: error.details,
                        logs: Some(logs),
                        traces: None,
                    })))
                }
            }
            Err(_) => {
                // Semaphore full - queue the request for backpressure
                let (response_tx, response_rx) = oneshot::channel();
                
                let queued_request = QueuedRequest {
                    plugin_id,
                    compiled_code,
                    plugin_path,
                    params,
                    headers,
                    socket_path,
                    http_request_id,
                    timeout_secs,
                    response_tx,
                };
                
                match self.request_tx.try_send(queued_request) {
                    Ok(()) => {
                        response_rx.await.map_err(|_| {
                            PluginError::PluginExecutionError(
                                "Request queue processor closed".to_string()
                            )
                        })?
                    }
                    Err(async_channel::TrySendError::Full(_)) => {
                        Err(PluginError::PluginExecutionError(format!(
                            "Plugin execution queue is full (max: {}). Please retry later.",
                            self.max_queue_size
                        )))
                    }
                    Err(async_channel::TrySendError::Closed(_)) => {
                        Err(PluginError::PluginExecutionError(
                            "Plugin execution queue is closed".to_string()
                        ))
                    }
                }
            }
        }
    }

    /// Precompile a plugin
    pub async fn precompile_plugin(
        &self,
        plugin_id: String,
        plugin_path: Option<String>,
        source_code: Option<String>,
    ) -> Result<String, PluginError> {
        self.ensure_started().await?;

        let mut conn = self.connection_pool.acquire().await?;

        let request = PoolRequest::Precompile {
            task_id: Uuid::new_v4().to_string(),
            plugin_id: plugin_id.clone(),
            plugin_path,
            source_code,
        };

        let response = conn.send_request_with_timeout(&request, DEFAULT_POOL_REQUEST_TIMEOUT_SECS).await?;

        if response.success {
            response
                .result
                .and_then(|v| v.get("code").and_then(|c| c.as_str()).map(|s| s.to_string()))
                .ok_or_else(|| {
                    PluginError::PluginExecutionError("No compiled code in response".to_string())
                })
        } else {
            let error = response.error.unwrap_or(PoolError {
                message: "Compilation failed".to_string(),
                code: None,
                status: None,
                details: None,
            });
            Err(PluginError::PluginExecutionError(error.message))
        }
    }

    /// Cache compiled code in the pool
    pub async fn cache_compiled_code(
        &self,
        plugin_id: String,
        compiled_code: String,
    ) -> Result<(), PluginError> {
        self.ensure_started().await?;

        let mut conn = self.connection_pool.acquire().await?;

        let request = PoolRequest::Cache {
            task_id: Uuid::new_v4().to_string(),
            plugin_id: plugin_id.clone(),
            compiled_code,
        };

        let response = conn.send_request_with_timeout(&request, DEFAULT_POOL_REQUEST_TIMEOUT_SECS).await?;

        if response.success {
            Ok(())
        } else {
            let error = response.error.unwrap_or(PoolError {
                message: "Cache failed".to_string(),
                code: None,
                status: None,
                details: None,
            });
            Err(PluginError::PluginError(error.message))
        }
    }

    /// Invalidate a cached plugin
    pub async fn invalidate_plugin(&self, plugin_id: String) -> Result<(), PluginError> {
        if !*self.initialized.read().await {
            return Ok(());
        }

        let mut conn = self.connection_pool.acquire().await?;

        let request = PoolRequest::Invalidate {
            task_id: Uuid::new_v4().to_string(),
            plugin_id,
        };

        let _ = conn.send_request_with_timeout(&request, DEFAULT_POOL_REQUEST_TIMEOUT_SECS).await?;
        Ok(())
    }

    /// Health check - verify the pool server is responding
    pub async fn health_check(&self) -> Result<HealthStatus, PluginError> {
        if !*self.initialized.read().await {
            return Ok(HealthStatus {
                healthy: false,
                status: "not_initialized".to_string(),
                uptime_ms: None,
                memory: None,
                pool_completed: None,
                pool_queued: None,
                success_rate: None,
            });
        }

        let mut conn = match self.connection_pool.acquire().await {
            Ok(c) => c,
            Err(e) => {
                return Ok(HealthStatus {
                    healthy: false,
                    status: format!("connection_failed: {}", e),
                    uptime_ms: None,
                    memory: None,
                    pool_completed: None,
                    pool_queued: None,
                    success_rate: None,
                });
            }
        };

        let request = PoolRequest::Health {
            task_id: Uuid::new_v4().to_string(),
        };

        match conn.send_request_with_timeout(&request, 5).await {
            Ok(response) => {
                if response.success {
                    let result = response.result.unwrap_or_default();
                    Ok(HealthStatus {
                        healthy: true,
                        status: result.get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                        uptime_ms: result.get("uptime").and_then(|v| v.as_u64()),
                        memory: result.get("memory")
                            .and_then(|v| v.get("heapUsed"))
                            .and_then(|v| v.as_u64()),
                        pool_completed: result.get("pool")
                            .and_then(|v| v.get("completed"))
                            .and_then(|v| v.as_u64()),
                        pool_queued: result.get("pool")
                            .and_then(|v| v.get("queued"))
                            .and_then(|v| v.as_u64()),
                        success_rate: result.get("execution")
                            .and_then(|v| v.get("successRate"))
                            .and_then(|v| v.as_f64()),
                    })
                } else {
                    Ok(HealthStatus {
                        healthy: false,
                        status: response.error.map(|e| e.message).unwrap_or_else(|| "unknown_error".to_string()),
                        uptime_ms: None,
                        memory: None,
                        pool_completed: None,
                        pool_queued: None,
                        success_rate: None,
                    })
                }
            }
            Err(e) => {
                conn.mark_unhealthy();
                Ok(HealthStatus {
                    healthy: false,
                    status: format!("request_failed: {}", e),
                    uptime_ms: None,
                    memory: None,
                    pool_completed: None,
                    pool_queued: None,
                    success_rate: None,
                })
            }
        }
    }

    /// Check health and restart if unhealthy
    pub async fn ensure_healthy(&self) -> Result<bool, PluginError> {
        let health = self.health_check().await?;
        
        if health.healthy {
            return Ok(true);
        }

        tracing::warn!(status = %health.status, "Pool server unhealthy, attempting restart");

        self.restart().await?;

        let health_after = self.health_check().await?;
        Ok(health_after.healthy)
    }

    /// Force restart the pool server
    pub async fn restart(&self) -> Result<(), PluginError> {
        tracing::info!("Restarting plugin pool server");

        self.connection_pool.clear().await;

        {
            let mut process_guard = self.process.lock().await;
            if let Some(mut child) = process_guard.take() {
                let _ = child.kill().await;
            }
        }

        let _ = std::fs::remove_file(&self.socket_path);

        {
            let mut initialized = self.initialized.write().await;
            *initialized = false;
        }

        self.ensure_started().await
    }

    /// Shutdown the pool server
    pub async fn shutdown(&self) -> Result<(), PluginError> {
        let mut initialized = self.initialized.write().await;
        if !*initialized {
            return Ok(());
        }

        tracing::info!("Shutting down plugin pool server");

        self.connection_pool.clear().await;

        if let Ok(mut conn) = PoolConnection::new(&self.socket_path, 999999).await {
            let request = PoolRequest::Shutdown {
                task_id: Uuid::new_v4().to_string(),
            };
            let _ = conn.send_request(&request).await;
        }

        let mut process_guard = self.process.lock().await;
        if let Some(mut child) = process_guard.take() {
            let _ = child.kill().await;
        }

        let _ = std::fs::remove_file(&self.socket_path);

        *initialized = false;
        Ok(())
    }
}

impl Drop for PoolManager {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Global pool manager instance
static POOL_MANAGER: std::sync::OnceLock<Arc<PoolManager>> = std::sync::OnceLock::new();

/// Get or create the global pool manager
pub fn get_pool_manager() -> Arc<PoolManager> {
    POOL_MANAGER
        .get_or_init(|| Arc::new(PoolManager::new()))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_request_serialization() {
        let request = PoolRequest::Execute {
            task_id: "test-123".to_string(),
            plugin_id: "my-plugin".to_string(),
            compiled_code: Some("console.log('hello')".to_string()),
            plugin_path: None,
            params: serde_json::json!({"key": "value"}),
            headers: None,
            socket_path: "/tmp/test.sock".to_string(),
            http_request_id: Some("req-456".to_string()),
            timeout: Some(30000),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"type\":\"execute\""));
        assert!(json.contains("\"taskId\":\"test-123\""));
        assert!(json.contains("\"pluginId\":\"my-plugin\""));
    }

    #[test]
    fn test_pool_log_entry_conversion() {
        let pool_entry = PoolLogEntry {
            level: "error".to_string(),
            message: "test error".to_string(),
        };

        let log_entry: LogEntry = pool_entry.into();
        assert!(matches!(log_entry.level, LogLevel::Error));
        assert_eq!(log_entry.message, "test error");
    }
}
