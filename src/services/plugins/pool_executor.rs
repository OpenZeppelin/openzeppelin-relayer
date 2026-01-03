//! Pool-based Plugin Executor
//!
//! This module provides execution of pre-compiled JavaScript plugins via
//! a persistent Piscina worker pool, replacing the per-request ts-node approach.
//!
//! Communication with the Node.js pool server happens via Unix socket using
//! a JSON-line protocol.

use crossbeam::queue::SegQueue;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, RwLock, Semaphore};
use uuid::Uuid;

use super::{
    config::get_config, LogEntry, LogLevel, PluginError, PluginHandlerPayload, ScriptResult,
};

/// Lock-free ring buffer for tracking recent results (sliding window)
struct ResultRingBuffer {
    buffer: Vec<AtomicU8>, // 0 = empty, 1 = success, 2 = failure
    index: AtomicUsize,
    size: usize,
}

impl ResultRingBuffer {
    fn new(size: usize) -> Self {
        let mut buffer = Vec::with_capacity(size);
        for _ in 0..size {
            buffer.push(AtomicU8::new(0));
        }
        Self {
            buffer,
            index: AtomicUsize::new(0),
            size,
        }
    }

    fn record(&self, success: bool) {
        let idx = self.index.fetch_add(1, Ordering::Relaxed) % self.size;
        self.buffer[idx].store(if success { 1 } else { 2 }, Ordering::Relaxed);
    }

    fn failure_rate(&self) -> f32 {
        let mut total = 0;
        let mut failures = 0;

        for slot in &self.buffer {
            match slot.load(Ordering::Relaxed) {
                0 => {}          // Empty slot
                1 => total += 1, // Success
                2 => {
                    total += 1;
                    failures += 1;
                }
                _ => {}
            }
        }

        if total < 10 {
            return 0.0; // Not enough data to make decision
        }

        (failures as f32) / (total as f32)
    }
}

/// Circuit breaker state for automatic degradation under stress
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation - all requests allowed
    Closed,
    /// Degraded - some requests rejected to reduce load
    HalfOpen,
    /// Fully open - most requests rejected, recovery in progress
    Open,
}

/// Circuit breaker for managing pool health and automatic recovery
/// Tracks failure rates and response times to detect GC pressure
pub struct CircuitBreaker {
    /// Current circuit state (encoded as u8 for atomic access)
    /// 0 = Closed, 1 = HalfOpen, 2 = Open
    state: AtomicU32,
    /// Time when circuit opened (for recovery timing)
    opened_at_ms: AtomicU64,
    /// Consecutive successful requests in half-open state
    recovery_successes: AtomicU32,
    /// Average response time in ms (exponential moving average)
    avg_response_time_ms: AtomicU32,
    /// Number of restart attempts
    restart_attempts: AtomicU32,
    /// Sliding window of recent results (100 most recent)
    recent_results: Arc<ResultRingBuffer>,
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

impl CircuitBreaker {
    pub fn new() -> Self {
        Self {
            state: AtomicU32::new(0), // Closed
            opened_at_ms: AtomicU64::new(0),
            recovery_successes: AtomicU32::new(0),
            avg_response_time_ms: AtomicU32::new(0),
            restart_attempts: AtomicU32::new(0),
            recent_results: Arc::new(ResultRingBuffer::new(100)),
        }
    }

    fn state(&self) -> CircuitState {
        match self.state.load(Ordering::Relaxed) {
            0 => CircuitState::Closed,
            1 => CircuitState::HalfOpen,
            _ => CircuitState::Open,
        }
    }

    fn set_state(&self, state: CircuitState) {
        let val = match state {
            CircuitState::Closed => 0,
            CircuitState::HalfOpen => 1,
            CircuitState::Open => 2,
        };
        self.state.store(val, Ordering::Relaxed);
    }

    /// Record a successful request with response time
    pub fn record_success(&self, response_time_ms: u32) {
        self.recent_results.record(true);

        // Update exponential moving average (alpha = 0.1)
        let current = self.avg_response_time_ms.load(Ordering::Relaxed);
        let new_avg = if current == 0 {
            response_time_ms
        } else {
            (current * 9 + response_time_ms) / 10
        };
        self.avg_response_time_ms.store(new_avg, Ordering::Relaxed);

        // Handle state transitions on success
        match self.state() {
            CircuitState::HalfOpen => {
                let successes = self.recovery_successes.fetch_add(1, Ordering::Relaxed) + 1;
                // Require 10 consecutive successes to close circuit
                if successes >= 10 {
                    tracing::info!("Circuit breaker closing - recovery successful");
                    self.set_state(CircuitState::Closed);
                    self.recovery_successes.store(0, Ordering::Relaxed);
                    self.restart_attempts.store(0, Ordering::Relaxed);
                }
            }
            CircuitState::Open => {
                // Check if enough time has passed to try half-open
                self.maybe_transition_to_half_open();
            }
            CircuitState::Closed => {}
        }
    }

    /// Record a failed request
    pub fn record_failure(&self) {
        self.recent_results.record(false);
        self.recovery_successes.store(0, Ordering::Relaxed);

        let failure_rate = self.recent_results.failure_rate();

        match self.state() {
            CircuitState::Closed => {
                // Open circuit if failure rate > 50% (ring buffer requires at least 10 samples)
                if failure_rate > 0.5 {
                    tracing::warn!(
                        failure_rate = %format!("{:.1}%", failure_rate * 100.0),
                        "Circuit breaker opening - high failure rate"
                    );
                    self.open_circuit();
                }
            }
            CircuitState::HalfOpen => {
                // Any failure in half-open sends back to open
                tracing::warn!("Circuit breaker reopening - failure during recovery");
                self.open_circuit();
            }
            CircuitState::Open => {
                // Already open, just check for transition
                self.maybe_transition_to_half_open();
            }
        }
    }

    fn open_circuit(&self) {
        self.set_state(CircuitState::Open);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.opened_at_ms.store(now, Ordering::Relaxed);
        self.recovery_successes.store(0, Ordering::Relaxed);
    }

    fn maybe_transition_to_half_open(&self) {
        let opened_at = self.opened_at_ms.load(Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // Wait at least 5 seconds before trying recovery
        // Exponential backoff: 5s, 10s, 20s, 40s, max 60s
        let attempts = self.restart_attempts.load(Ordering::Relaxed);
        let backoff_ms = (5000u64 * (1 << attempts.min(4))).min(60000);

        if now - opened_at >= backoff_ms {
            tracing::info!(
                backoff_ms = backoff_ms,
                "Circuit breaker transitioning to half-open"
            );
            self.set_state(CircuitState::HalfOpen);
            self.restart_attempts.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Check if request should be allowed based on circuit state
    /// If recovery_allowance is provided, use it in HalfOpen state instead of default 10%
    pub fn should_allow_request(&self, recovery_allowance: Option<u32>) -> bool {
        match self.state() {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => {
                // Use recovery allowance if provided, otherwise default to 10%
                let allowance = recovery_allowance.unwrap_or(10);
                (rand::random::<u32>() % 100) < allowance
            }
            CircuitState::Open => {
                // Check if we should transition to half-open
                self.maybe_transition_to_half_open();
                // Recheck state after potential transition
                matches!(self.state(), CircuitState::HalfOpen)
            }
        }
    }

    /// Get current response time average for monitoring
    pub fn avg_response_time(&self) -> u32 {
        self.avg_response_time_ms.load(Ordering::Relaxed)
    }

    /// Force circuit to closed state (for manual recovery)
    pub fn force_close(&self) {
        self.set_state(CircuitState::Closed);
        self.recovery_successes.store(0, Ordering::Relaxed);
        self.restart_attempts.store(0, Ordering::Relaxed);
        // Note: Ring buffer will naturally overwrite old results, no need to clear
    }
}

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
    /// Circuit breaker state (Closed/HalfOpen/Open)
    pub circuit_state: Option<String>,
    /// Average response time in ms
    pub avg_response_time_ms: Option<u32>,
    /// Whether recovery mode is active
    pub recovering: Option<bool>,
    /// Current recovery allowance percentage
    pub recovery_percent: Option<u32>,
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
    /// Health status of this connection
    healthy: AtomicBool,
}

impl PoolConnection {
    async fn new(socket_path: &str, id: usize) -> Result<Self, PluginError> {
        // Retry connection with exponential backoff
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

                    // Log selectively to reduce noise
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
                    // Exponential backoff with cap at 1 second
                    delay_ms = std::cmp::min(delay_ms * 2, 1000);
                }
            }
        }
    }

    async fn send_request(&mut self, request: &PoolRequest) -> Result<PoolResponse, PluginError> {
        let json = serde_json::to_string(request)
            .map_err(|e| PluginError::PluginError(format!("Failed to serialize request: {e}")))?;

        // Write request and flush
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

        // Read response using BufReader on a reference to the stream
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

    /// Send request with timeout
    async fn send_request_with_timeout(
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
}

/// Connection pool for reusing socket connections
/// Uses lock-free SegQueue for O(1) connection acquisition and release
struct ConnectionPool {
    socket_path: String,
    /// Available connections (lock-free stack)
    available: Arc<SegQueue<PoolConnection>>,
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
            available: Arc::new(SegQueue::new()),
            max_connections,
            next_id: Arc::new(AtomicUsize::new(0)),
            semaphore: Arc::new(Semaphore::new(max_connections)),
        }
    }

    /// Get a connection from the pool or create a new one
    /// Uses semaphore for proper concurrency limiting
    /// Accepts optional pre-acquired permit for fast path optimization
    async fn acquire_with_permit(
        &self,
        permit: Option<tokio::sync::OwnedSemaphorePermit>,
    ) -> Result<PooledConnection<'_>, PluginError> {
        // Acquire permit if not provided
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
                    // Check if connection is healthy
                    if conn.healthy.load(Ordering::Relaxed) {
                        tracing::debug!(connection_id = conn.id, "Reusing connection from pool");
                        return Ok(PooledConnection {
                            conn: Some(conn),
                            pool: self,
                            _permit: permit,
                        });
                    }
                    // Connection unhealthy, drop it and try next
                    tracing::debug!(connection_id = conn.id, "Dropping unhealthy connection");
                    continue;
                }
                None => {
                    // No available connection, create a new one
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
    async fn acquire(&self) -> Result<PooledConnection<'_>, PluginError> {
        self.acquire_with_permit(None).await
    }

    /// Return a connection to the pool
    fn release(&self, conn: PoolConnection) {
        let conn_id = conn.id;
        let is_healthy = conn.healthy.load(Ordering::Relaxed);

        if is_healthy {
            // O(1) lock-free push
            self.available.push(conn);
            tracing::debug!(connection_id = conn_id, "Connection returned to pool");
        } else {
            tracing::debug!(connection_id = conn_id, "Connection dropped (unhealthy)");
        }
    }

    /// Mark a connection as unhealthy (no-op with new design)
    /// Health is now tracked in the connection itself
    fn mark_unhealthy(&self, _conn_id: usize) {
        // No longer needed - health is in PoolConnection.healthy
        // Connection will be dropped on release if unhealthy
    }

    /// Clear all connections
    async fn clear(&self) {
        // Drain the queue
        while self.available.pop().is_some() {}
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
            Err(PluginError::PluginError(
                "Connection already released".to_string(),
            ))
        }
    }

    /// Mark this connection as unhealthy (won't be returned to pool)
    fn mark_unhealthy(&mut self) {
        if let Some(ref conn) = self.conn {
            conn.healthy.store(false, Ordering::Relaxed);
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
    /// Lock to prevent concurrent restarts (thundering herd)
    restart_lock: tokio::sync::Mutex<()>,
    /// Connection pool for reusing connections
    connection_pool: Arc<ConnectionPool>,
    /// Request queue for throttling/backpressure (multi-consumer channel)
    request_tx: async_channel::Sender<QueuedRequest>,
    /// Actual configured queue size (for error messages)
    max_queue_size: usize,
    /// Flag indicating if health check is needed (set by background task)
    health_check_needed: Arc<AtomicBool>,
    /// Consecutive failure count for health checks
    consecutive_failures: Arc<AtomicU32>,
    /// Circuit breaker for automatic degradation under GC pressure
    circuit_breaker: Arc<CircuitBreaker>,
    /// Last successful restart time (for backoff calculation)
    last_restart_time_ms: Arc<AtomicU64>,
    /// Is currently in recovery mode (gradual ramp-up)
    recovery_mode: Arc<AtomicBool>,
    /// Requests allowed during recovery (gradual increase)
    recovery_allowance: Arc<AtomicU32>,
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
        // Use centralized config with auto-derivation from PLUGIN_MAX_CONCURRENCY
        let config = get_config();
        let max_connections = config.pool_max_connections;
        let max_queue_size = config.pool_max_queue_size;

        // Use async-channel for multi-consumer queue (no mutex needed)
        let (tx, rx) = async_channel::bounded(max_queue_size);

        let connection_pool = Arc::new(ConnectionPool::new(socket_path.clone(), max_connections));
        let connection_pool_clone = connection_pool.clone();

        // Spawn background workers to process queued requests
        Self::spawn_queue_workers(rx, connection_pool_clone, config.pool_workers);

        let health_check_needed = Arc::new(AtomicBool::new(false));
        let consecutive_failures = Arc::new(AtomicU32::new(0));
        let circuit_breaker = Arc::new(CircuitBreaker::new());
        let last_restart_time_ms = Arc::new(AtomicU64::new(0));
        let recovery_mode = Arc::new(AtomicBool::new(false));
        let recovery_allowance = Arc::new(AtomicU32::new(0));

        // Spawn background health check task to avoid atomic ops on hot path
        Self::spawn_health_check_task(
            health_check_needed.clone(),
            config.health_check_interval_secs,
        );

        // Spawn background recovery ramp-up task
        Self::spawn_recovery_task(recovery_mode.clone(), recovery_allowance.clone());

        Self {
            connection_pool,
            socket_path,
            process: tokio::sync::Mutex::new(None),
            initialized: RwLock::new(false),
            restart_lock: tokio::sync::Mutex::new(()),
            request_tx: tx,
            max_queue_size,
            health_check_needed,
            consecutive_failures,
            circuit_breaker,
            last_restart_time_ms,
            recovery_mode,
            recovery_allowance,
        }
    }

    /// Spawn background task to gradually increase recovery allowance
    fn spawn_recovery_task(recovery_mode: Arc<AtomicBool>, recovery_allowance: Arc<AtomicU32>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(500));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                interval.tick().await;

                if recovery_mode.load(Ordering::Relaxed) {
                    let current = recovery_allowance.load(Ordering::Relaxed);
                    // Gradually increase from 10% to 100% over ~15 seconds
                    // 10 -> 20 -> 30 -> ... -> 100
                    if current < 100 {
                        let new_allowance = (current + 10).min(100);
                        recovery_allowance.store(new_allowance, Ordering::Relaxed);
                        tracing::debug!(
                            allowance = new_allowance,
                            "Recovery mode: increasing request allowance"
                        );
                    } else {
                        // Recovery complete
                        recovery_mode.store(false, Ordering::Relaxed);
                        tracing::info!("Recovery mode complete - full capacity restored");
                    }
                }
            }
        });
    }

    /// Spawn background task to set health check flag periodically
    /// This avoids atomic operations on the hot path
    fn spawn_health_check_task(health_check_needed: Arc<AtomicBool>, interval_secs: u64) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                interval.tick().await;
                health_check_needed.store(true, Ordering::Relaxed);
            }
        });
    }

    /// Spawn multiple worker tasks to process queued requests concurrently
    fn spawn_queue_workers(
        rx: async_channel::Receiver<QueuedRequest>,
        connection_pool: Arc<ConnectionPool>,
        configured_workers: usize,
    ) {
        let num_workers = if configured_workers > 0 {
            configured_workers
        } else {
            std::thread::available_parallelism()
                .map(|n| n.get().max(4).min(32))
                .unwrap_or(8)
        };

        tracing::info!(num_workers = num_workers, "Starting request queue workers");

        for worker_id in 0..num_workers {
            let rx_clone = rx.clone();
            let pool_clone = connection_pool.clone();

            tokio::spawn(async move {
                while let Ok(request) = rx_clone.recv().await {
                    let start = std::time::Instant::now();
                    let plugin_id = request.plugin_id.clone();

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
                    )
                    .await;

                    let elapsed = start.elapsed();
                    if let Err(ref e) = result {
                        // Don't warn about shutdown errors - these are expected during graceful shutdown
                        let error_str = format!("{:?}", e);
                        if error_str.contains("shutdown") || error_str.contains("Shutdown") {
                            tracing::debug!(
                                worker_id = worker_id,
                                plugin_id = %plugin_id,
                                "Plugin execution cancelled during shutdown"
                            );
                        } else {
                            tracing::warn!(
                                worker_id = worker_id,
                                plugin_id = %plugin_id,
                                elapsed_ms = elapsed.as_millis() as u64,
                                error = ?e,
                                "Plugin execution failed"
                            );
                        }
                    } else if elapsed.as_secs() > 1 {
                        tracing::debug!(
                            worker_id = worker_id,
                            plugin_id = %plugin_id,
                            elapsed_ms = elapsed.as_millis() as u64,
                            "Slow plugin execution"
                        );
                    }

                    let _ = request.response_tx.send(result);
                }

                tracing::debug!(worker_id = worker_id, "Request queue worker exited");
            });
        }
    }

    /// Execute plugin with optional pre-acquired permit (unified fast/slow path)
    /// This eliminates code duplication between fast path and queued execution
    async fn execute_with_permit(
        connection_pool: &Arc<ConnectionPool>,
        permit: Option<tokio::sync::OwnedSemaphorePermit>,
        plugin_id: String,
        compiled_code: Option<String>,
        plugin_path: Option<String>,
        params: serde_json::Value,
        headers: Option<HashMap<String, Vec<String>>>,
        socket_path: String,
        http_request_id: Option<String>,
        timeout_secs: Option<u64>,
    ) -> Result<ScriptResult, PluginError> {
        let mut conn = connection_pool.acquire_with_permit(permit).await?;

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

        let timeout = timeout_secs.unwrap_or(get_config().pool_request_timeout_secs);
        let response = conn.send_request_with_timeout(&request, timeout).await?;

        // Avoid allocating empty Vec - only allocate if logs exist
        let logs: Vec<LogEntry> = response
            .logs
            .map(|logs| logs.into_iter().map(|l| l.into()).collect())
            .unwrap_or_else(Vec::new);

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

    /// Internal execution method (wrapper for execute_with_permit)
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
        // Delegate to unified execution method (no pre-acquired permit for queued requests)
        Self::execute_with_permit(
            connection_pool,
            None,
            plugin_id,
            compiled_code,
            plugin_path,
            params,
            headers,
            socket_path,
            http_request_id,
            timeout_secs,
        )
        .await
    }

    /// Start the pool server if not already running
    /// Uses startup lock to prevent concurrent starts
    pub async fn ensure_started(&self) -> Result<(), PluginError> {
        // Fast path: check if already initialized
        if *self.initialized.read().await {
            return Ok(());
        }

        // Acquire startup lock to prevent concurrent starts
        let _startup_guard = self.restart_lock.lock().await;

        // Double-check after acquiring lock
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
    /// Uses background task flag to avoid atomic operations on hot path
    async fn ensure_started_and_healthy(&self) -> Result<(), PluginError> {
        self.ensure_started().await?;

        // Check if background task flagged that health check is needed
        // This is a simple load, no compare_exchange - much faster!
        if !self.health_check_needed.load(Ordering::Relaxed) {
            // No health check needed yet
            return Ok(());
        }

        // Try to clear the flag atomically (only one thread does the check)
        if !self
            .health_check_needed
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            // Another thread is already doing the health check
            return Ok(());
        }

        // Check if the process is still running (and reap zombies)
        let process_running = {
            let mut process_guard = self.process.lock().await;
            if let Some(child) = process_guard.as_mut() {
                // Actually check if process is still alive using try_wait()
                // This returns Ok(Some(exit_status)) if process has exited,
                // Ok(None) if still running, or Err if check failed
                match child.try_wait() {
                    Ok(Some(exit_status)) => {
                        // Process has exited - log, clean up, and mark as not running
                        tracing::warn!(
                            exit_status = ?exit_status,
                            "Pool server process has exited"
                        );
                        // Drop the child handle to reap zombie process
                        *process_guard = None;
                        false
                    }
                    Ok(None) => {
                        // Process is still running
                        true
                    }
                    Err(e) => {
                        // Failed to check status - assume dead to be safe
                        tracing::warn!(
                            error = %e,
                            "Failed to check pool server process status, assuming dead"
                        );
                        // Clean up handle
                        *process_guard = None;
                        false
                    }
                }
            } else {
                false
            }
        };

        if !process_running {
            // Process definitely not running - try to restart
            tracing::warn!("Pool server process not running, attempting restart");
            self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
            if let Err(e) = self.restart().await {
                tracing::error!(error = %e, "Failed to restart pool server");
                return Err(PluginError::PluginExecutionError(
                    "Pool server not running and restart failed".to_string(),
                ));
            }
            self.consecutive_failures.store(0, Ordering::Relaxed);
            return Ok(());
        }

        // Process is running - do a lightweight socket check instead of full health check
        // This avoids using connections from the pool under load
        let socket_exists = std::path::Path::new(&self.socket_path).exists();

        if !socket_exists {
            // Socket file gone - server crashed or was killed
            tracing::warn!(
                socket_path = %self.socket_path,
                "Pool server socket file missing, attempting restart"
            );
            self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
            if let Err(e) = self.restart().await {
                tracing::error!(error = %e, "Failed to restart pool server");
                return Err(PluginError::PluginExecutionError(
                    "Pool server socket missing and restart failed".to_string(),
                ));
            }
            self.consecutive_failures.store(0, Ordering::Relaxed);
        }

        Ok(())
    }

    async fn start_pool_server(&self) -> Result<(), PluginError> {
        let mut process_guard = self.process.lock().await;

        if process_guard.is_some() {
            return Ok(());
        }

        // Clean up socket file with retry logic to handle race conditions
        // Multiple attempts may be needed if another process is cleaning up
        let mut attempts = 0;
        let max_cleanup_attempts = 5;
        while attempts < max_cleanup_attempts {
            match std::fs::remove_file(&self.socket_path) {
                Ok(_) => break,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // File doesn't exist, that's fine
                    break;
                }
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_cleanup_attempts {
                        tracing::warn!(
                            socket_path = %self.socket_path,
                            error = %e,
                            "Failed to remove socket file after {} attempts, proceeding anyway",
                            max_cleanup_attempts
                        );
                        break;
                    }
                    // Wait a bit before retrying (exponential backoff)
                    let delay_ms = 10 * (1 << attempts.min(3)); // Max 80ms
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
            }
        }

        // Wait a bit more to ensure socket is fully released by OS
        tokio::time::sleep(Duration::from_millis(50)).await;

        let pool_server_path = std::env::current_dir()
            .map(|cwd| cwd.join("plugins/lib/pool-server.ts").display().to_string())
            .unwrap_or_else(|_| "plugins/lib/pool-server.ts".to_string());

        tracing::info!(socket_path = %self.socket_path, "Starting plugin pool server");

        // Get config values to pass to Node.js pool server
        let config = get_config();

        // Calculate heap size for pool server based on concurrency
        // Pool server needs heap for: worker management, code cache, message buffering
        // Formula: 512MB base + 32MB per 10 concurrent workers
        let calculated_heap = 512 + ((config.max_concurrency / 10) * 32);

        // Cap at 8GB to prevent unreasonable allocations
        let pool_server_heap_mb = calculated_heap.min(8192);

        if calculated_heap > 8192 {
            tracing::warn!(
                calculated_heap_mb = calculated_heap,
                capped_heap_mb = pool_server_heap_mb,
                max_concurrency = config.max_concurrency,
                "Pool server heap calculation exceeded 8GB cap"
            );
        }

        tracing::info!(
            heap_mb = pool_server_heap_mb,
            max_concurrency = config.max_concurrency,
            "Configuring pool server heap size"
        );

        // Node.js options: heap size and expose GC for emergency recovery
        let node_options = format!("--max-old-space-size={} --expose-gc", pool_server_heap_mb);

        let mut child = Command::new("ts-node")
            .arg("--transpile-only")
            .arg(&pool_server_path)
            .arg(&self.socket_path)
            // Pass Node.js flags via NODE_OPTIONS (ts-node doesn't accept them directly)
            .env("NODE_OPTIONS", node_options)
            // Pass derived config to Node.js (single source of truth from Rust)
            .env("PLUGIN_MAX_CONCURRENCY", config.max_concurrency.to_string())
            .env("PLUGIN_POOL_MIN_THREADS", config.nodejs_pool_min_threads.to_string())
            .env("PLUGIN_POOL_MAX_THREADS", config.nodejs_pool_max_threads.to_string())
            .env("PLUGIN_POOL_CONCURRENT_TASKS", config.nodejs_pool_concurrent_tasks.to_string())
            .env("PLUGIN_POOL_IDLE_TIMEOUT", config.nodejs_pool_idle_timeout_ms.to_string())
            .env("PLUGIN_WORKER_HEAP_MB", config.nodejs_worker_heap_mb.to_string())
            .env("PLUGIN_POOL_SOCKET_BACKLOG", config.pool_socket_backlog.to_string())
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
    /// Includes circuit breaker for automatic degradation under GC pressure
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
        // Get recovery allowance if in recovery mode
        let recovery_allowance = if self.recovery_mode.load(Ordering::Relaxed) {
            Some(self.recovery_allowance.load(Ordering::Relaxed))
        } else {
            None
        };

        // Single unified check - circuit breaker handles recovery mode
        if !self
            .circuit_breaker
            .should_allow_request(recovery_allowance)
        {
            let state = self.circuit_breaker.state();
            tracing::warn!(
                plugin_id = %plugin_id,
                circuit_state = ?state,
                recovery_allowance = ?recovery_allowance,
                "Request rejected by circuit breaker"
            );
            return Err(PluginError::PluginExecutionError(
                "Plugin system temporarily unavailable due to high load. Please retry shortly."
                    .to_string(),
            ));
        }

        let start_time = Instant::now();

        // Ensure pool is started and healthy
        self.ensure_started_and_healthy().await?;

        // Try direct execution first (fast path with pre-acquired permit)
        // If semaphore can't be acquired immediately, queue the request (slow path)
        let circuit_breaker = self.circuit_breaker.clone();
        match self.connection_pool.semaphore.clone().try_acquire_owned() {
            Ok(permit) => {
                // Fast path: execute directly with pre-acquired permit
                // This avoids queueing overhead for normal load
                let result = Self::execute_with_permit(
                    &self.connection_pool,
                    Some(permit),
                    plugin_id,
                    compiled_code,
                    plugin_path,
                    params,
                    headers,
                    socket_path,
                    http_request_id,
                    timeout_secs,
                )
                .await;

                // Record result with circuit breaker
                let elapsed_ms = start_time.elapsed().as_millis() as u32;
                match &result {
                    Ok(_) => circuit_breaker.record_success(elapsed_ms),
                    Err(e) => {
                        circuit_breaker.record_failure();
                        // Check if this error indicates the pool server is dead
                        // (EOF while parsing, broken pipe, connection refused, etc.)
                        if Self::is_dead_server_error(e) {
                            tracing::warn!(
                                error = %e,
                                "Detected dead pool server error, triggering health check for restart"
                            );
                            // Set health check flag so next request triggers restart
                            self.health_check_needed.store(true, Ordering::Relaxed);
                            // Also clear the connection pool since connections are dead
                            let pool = self.connection_pool.clone();
                            tokio::spawn(async move {
                                pool.clear().await;
                            });
                        }
                    }
                }

                result
            }
            Err(_) => {
                // Semaphore full - queue the request for backpressure
                // Use blocking send with timeout to handle bursts better
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

                // Try non-blocking send first (fast path)
                let result = match self.request_tx.try_send(queued_request) {
                    Ok(()) => {
                        let queue_len = self.request_tx.len();
                        if queue_len > self.max_queue_size / 2 {
                            tracing::warn!(
                                queue_len = queue_len,
                                max_queue_size = self.max_queue_size,
                                "Plugin queue is over 50% capacity"
                            );
                        }
                        response_rx.await.map_err(|_| {
                            PluginError::PluginExecutionError(
                                "Request queue processor closed".to_string(),
                            )
                        })?
                    }
                    Err(async_channel::TrySendError::Full(req)) => {
                        // Queue is full - try blocking send with timeout
                        // This allows handling bursts without immediate rejection
                        let queue_timeout_ms = get_config().pool_queue_send_timeout_ms;
                        let queue_timeout = Duration::from_millis(queue_timeout_ms);
                        match tokio::time::timeout(queue_timeout, self.request_tx.send(req)).await {
                            Ok(Ok(())) => {
                                // Successfully queued after waiting
                                let queue_len = self.request_tx.len();
                                tracing::debug!(
                                    queue_len = queue_len,
                                    "Request queued after waiting for queue space"
                                );
                                response_rx.await.map_err(|_| {
                                    PluginError::PluginExecutionError(
                                        "Request queue processor closed".to_string(),
                                    )
                                })?
                            }
                            Ok(Err(async_channel::SendError(_))) => {
                                // Channel closed
                                Err(PluginError::PluginExecutionError(
                                    "Plugin execution queue is closed".to_string(),
                                ))
                            }
                            Err(_) => {
                                // Timeout waiting for queue space
                                let queue_len = self.request_tx.len();
                                tracing::error!(
                                    queue_len = queue_len,
                                    max_queue_size = self.max_queue_size,
                                    timeout_ms = queue_timeout.as_millis(),
                                    "Plugin execution queue is FULL - timeout waiting for space"
                                );
                                Err(PluginError::PluginExecutionError(format!(
                                    "Plugin execution queue is full (max: {}) and timeout waiting for space. \
                                    Consider increasing PLUGIN_POOL_MAX_QUEUE_SIZE or PLUGIN_POOL_MAX_CONNECTIONS.",
                                    self.max_queue_size
                                )))
                            }
                        }
                    }
                    Err(async_channel::TrySendError::Closed(_)) => {
                        Err(PluginError::PluginExecutionError(
                            "Plugin execution queue is closed".to_string(),
                        ))
                    }
                };

                // Record result with circuit breaker (for queued path)
                let elapsed_ms = start_time.elapsed().as_millis() as u32;
                match &result {
                    Ok(_) => circuit_breaker.record_success(elapsed_ms),
                    Err(e) => {
                        circuit_breaker.record_failure();
                        // Check if this error indicates the pool server is dead
                        if Self::is_dead_server_error(e) {
                            tracing::warn!(
                                error = %e,
                                "Detected dead pool server error (queued path), triggering health check for restart"
                            );
                            // Set health check flag so next request triggers restart
                            self.health_check_needed.store(true, Ordering::Relaxed);
                            // Also clear the connection pool since connections are dead
                            let pool = self.connection_pool.clone();
                            tokio::spawn(async move {
                                pool.clear().await;
                            });
                        }
                    }
                }

                result
            }
        }
    }

    /// Check if an error indicates the pool server is dead and needs restart
    /// Excludes plugin execution timeouts which are user errors, not server failures
    fn is_dead_server_error(err: &PluginError) -> bool {
        let error_str = err.to_string().to_lowercase();

        // Exclude plugin execution timeouts (not a server issue)
        if error_str.contains("handler timed out")
            || (error_str.contains("plugin") && error_str.contains("timed out"))
        {
            return false;
        }

        // Patterns that indicate the server process is dead or unreachable
        let dead_server_patterns = [
            "eof while parsing",    // JSON parse error - connection closed mid-message
            "broken pipe",          // Write to closed socket
            "connection refused",   // Server not listening
            "connection reset",     // Server forcefully closed
            "not connected",        // Socket not connected
            "failed to connect",    // Connection establishment failed
            "socket file missing",  // Unix socket file deleted
            "no such file",         // Socket file doesn't exist
            "connection timed out", // Connection timeout (not execution timeout)
            "connect timed out",    // Connect operation timeout
        ];

        dead_server_patterns
            .iter()
            .any(|pattern| error_str.contains(pattern))
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

        let response = conn
            .send_request_with_timeout(&request, get_config().pool_request_timeout_secs)
            .await?;

        if response.success {
            response
                .result
                .and_then(|v| {
                    v.get("code")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string())
                })
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

        let response = conn
            .send_request_with_timeout(&request, get_config().pool_request_timeout_secs)
            .await?;

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

        let _ = conn
            .send_request_with_timeout(&request, get_config().pool_request_timeout_secs)
            .await?;
        Ok(())
    }

    /// Health check - verify the pool server is responding
    /// Note: Under heavy load, this may return "healthy: false" due to connection pool exhaustion,
    /// which is NOT the same as the server being down. Use ensure_started_and_healthy() for
    /// automatic recovery which distinguishes between these cases.
    pub async fn health_check(&self) -> Result<HealthStatus, PluginError> {
        // Helper to get circuit breaker info for all responses
        let circuit_info = || {
            let state = match self.circuit_breaker.state() {
                CircuitState::Closed => "closed",
                CircuitState::HalfOpen => "half_open",
                CircuitState::Open => "open",
            };
            (
                Some(state.to_string()),
                Some(self.circuit_breaker.avg_response_time()),
                Some(self.recovery_mode.load(Ordering::Relaxed)),
                Some(self.recovery_allowance.load(Ordering::Relaxed)),
            )
        };

        if !*self.initialized.read().await {
            let (circuit_state, avg_rt, recovering, recovery_pct) = circuit_info();
            return Ok(HealthStatus {
                healthy: false,
                status: "not_initialized".to_string(),
                uptime_ms: None,
                memory: None,
                pool_completed: None,
                pool_queued: None,
                success_rate: None,
                circuit_state,
                avg_response_time_ms: avg_rt,
                recovering,
                recovery_percent: recovery_pct,
            });
        }

        // First, do a fast check - is the socket file present?
        if !std::path::Path::new(&self.socket_path).exists() {
            let (circuit_state, avg_rt, recovering, recovery_pct) = circuit_info();
            return Ok(HealthStatus {
                healthy: false,
                status: "socket_missing".to_string(),
                uptime_ms: None,
                memory: None,
                pool_completed: None,
                pool_queued: None,
                success_rate: None,
                circuit_state,
                avg_response_time_ms: avg_rt,
                recovering,
                recovery_percent: recovery_pct,
            });
        }

        // Try to acquire a connection with a short timeout
        // Use try_acquire to not wait when pool is exhausted
        let mut conn =
            match tokio::time::timeout(Duration::from_millis(100), self.connection_pool.acquire())
                .await
            {
                Ok(Ok(c)) => c,
                Ok(Err(e)) => {
                    // Connection error - but check if it's just pool exhaustion
                    let err_str = e.to_string();
                    let is_pool_exhausted =
                        err_str.contains("semaphore") || err_str.contains("Connection refused");

                    let (circuit_state, avg_rt, recovering, recovery_pct) = circuit_info();
                    return Ok(HealthStatus {
                        // Mark as "healthy" if just pool exhaustion - server is running, just busy
                        healthy: is_pool_exhausted,
                        status: if is_pool_exhausted {
                            format!("pool_exhausted: {}", e)
                        } else {
                            format!("connection_failed: {}", e)
                        },
                        uptime_ms: None,
                        memory: None,
                        pool_completed: None,
                        pool_queued: None,
                        success_rate: None,
                        circuit_state,
                        avg_response_time_ms: avg_rt,
                        recovering,
                        recovery_percent: recovery_pct,
                    });
                }
                Err(_) => {
                    // Timeout acquiring connection - pool is busy, server is likely healthy
                    let (circuit_state, avg_rt, recovering, recovery_pct) = circuit_info();
                    return Ok(HealthStatus {
                        healthy: true, // Server running, just busy
                        status: "pool_busy".to_string(),
                        uptime_ms: None,
                        memory: None,
                        pool_completed: None,
                        pool_queued: None,
                        success_rate: None,
                        circuit_state,
                        avg_response_time_ms: avg_rt,
                        recovering,
                        recovery_percent: recovery_pct,
                    });
                }
            };

        let request = PoolRequest::Health {
            task_id: Uuid::new_v4().to_string(),
        };

        let (circuit_state, avg_rt, recovering, recovery_pct) = circuit_info();

        match conn.send_request_with_timeout(&request, 5).await {
            Ok(response) => {
                if response.success {
                    let result = response.result.unwrap_or_default();
                    Ok(HealthStatus {
                        healthy: true,
                        status: result
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                        uptime_ms: result.get("uptime").and_then(|v| v.as_u64()),
                        memory: result
                            .get("memory")
                            .and_then(|v| v.get("heapUsed"))
                            .and_then(|v| v.as_u64()),
                        pool_completed: result
                            .get("pool")
                            .and_then(|v| v.get("completed"))
                            .and_then(|v| v.as_u64()),
                        pool_queued: result
                            .get("pool")
                            .and_then(|v| v.get("queued"))
                            .and_then(|v| v.as_u64()),
                        success_rate: result
                            .get("execution")
                            .and_then(|v| v.get("successRate"))
                            .and_then(|v| v.as_f64()),
                        circuit_state,
                        avg_response_time_ms: avg_rt,
                        recovering,
                        recovery_percent: recovery_pct,
                    })
                } else {
                    Ok(HealthStatus {
                        healthy: false,
                        status: response
                            .error
                            .map(|e| e.message)
                            .unwrap_or_else(|| "unknown_error".to_string()),
                        uptime_ms: None,
                        memory: None,
                        pool_completed: None,
                        pool_queued: None,
                        success_rate: None,
                        circuit_state,
                        avg_response_time_ms: avg_rt,
                        recovering,
                        recovery_percent: recovery_pct,
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
                    circuit_state,
                    avg_response_time_ms: avg_rt,
                    recovering,
                    recovery_percent: recovery_pct,
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

        // Try to acquire restart lock - if another task is already restarting, wait for it
        // Use try_lock first to avoid thundering herd
        match self.restart_lock.try_lock() {
            Ok(_guard) => {
                // We got the lock - check health again in case another task just finished restart
                let health_recheck = self.health_check().await?;
                if health_recheck.healthy {
                    return Ok(true);
                }

                tracing::warn!(status = %health.status, "Pool server unhealthy, attempting restart");
                self.restart_internal().await?;
            }
            Err(_) => {
                // Another task is restarting - wait for it to complete
                tracing::debug!("Waiting for another task to complete pool server restart");
                let _guard = self.restart_lock.lock().await;
                // Restart completed by other task, check health
            }
        }

        let health_after = self.health_check().await?;
        Ok(health_after.healthy)
    }

    /// Force restart the pool server (public API - acquires lock)
    pub async fn restart(&self) -> Result<(), PluginError> {
        let _guard = self.restart_lock.lock().await;
        self.restart_internal().await
    }

    /// Internal restart without lock (must be called with restart_lock held)
    async fn restart_internal(&self) -> Result<(), PluginError> {
        tracing::info!("Restarting plugin pool server");

        self.connection_pool.clear().await;

        {
            let mut process_guard = self.process.lock().await;
            if let Some(mut child) = process_guard.take() {
                let _ = child.kill().await;
                // Wait a bit for process to fully terminate
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        // Clean up socket file with retry logic
        let mut attempts = 0;
        let max_cleanup_attempts = 5;
        while attempts < max_cleanup_attempts {
            match std::fs::remove_file(&self.socket_path) {
                Ok(_) => break,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    break;
                }
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_cleanup_attempts {
                        tracing::warn!(
                            socket_path = %self.socket_path,
                            error = %e,
                            "Failed to remove socket file during restart after {} attempts",
                            max_cleanup_attempts
                        );
                        break;
                    }
                    let delay_ms = 10 * (1 << attempts.min(3));
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
            }
        }

        // Wait for socket to be fully released
        tokio::time::sleep(Duration::from_millis(100)).await;

        {
            let mut initialized = self.initialized.write().await;
            *initialized = false;
        }

        // Start server (will acquire startup lock, but we already hold restart_lock)
        // Since restart_lock is the same as startup lock, we need to call start_pool_server directly
        let mut process_guard = self.process.lock().await;
        if process_guard.is_some() {
            // Already started by another call
            return Ok(());
        }

        let pool_server_path = std::env::current_dir()
            .map(|cwd| cwd.join("plugins/lib/pool-server.ts").display().to_string())
            .unwrap_or_else(|_| "plugins/lib/pool-server.ts".to_string());

        tracing::info!(socket_path = %self.socket_path, "Restarting plugin pool server");

        // Get config values to pass to Node.js pool server
        let config = get_config();

        // Calculate heap size for pool server based on concurrency
        let calculated_heap = 512 + ((config.max_concurrency / 10) * 32);

        // Cap at 8GB to prevent unreasonable allocations
        let pool_server_heap_mb = calculated_heap.min(8192);

        if calculated_heap > 8192 {
            tracing::warn!(
                calculated_heap_mb = calculated_heap,
                capped_heap_mb = pool_server_heap_mb,
                max_concurrency = config.max_concurrency,
                "Pool server heap calculation exceeded 8GB cap during restart"
            );
        }

        tracing::info!(
            heap_mb = pool_server_heap_mb,
            max_concurrency = config.max_concurrency,
            "Configuring pool server heap size for restart"
        );

        // Node.js options: heap size and expose GC for emergency recovery
        let node_options = format!("--max-old-space-size={} --expose-gc", pool_server_heap_mb);

        let mut child = Command::new("ts-node")
            .arg("--transpile-only")
            .arg(&pool_server_path)
            .arg(&self.socket_path)
            // Pass Node.js flags via NODE_OPTIONS
            .env("NODE_OPTIONS", node_options)
            // Pass derived config to Node.js (single source of truth from Rust)
            .env("PLUGIN_MAX_CONCURRENCY", config.max_concurrency.to_string())
            .env("PLUGIN_POOL_MIN_THREADS", config.nodejs_pool_min_threads.to_string())
            .env("PLUGIN_POOL_MAX_THREADS", config.nodejs_pool_max_threads.to_string())
            .env("PLUGIN_POOL_CONCURRENT_TASKS", config.nodejs_pool_concurrent_tasks.to_string())
            .env("PLUGIN_POOL_IDLE_TIMEOUT", config.nodejs_pool_idle_timeout_ms.to_string())
            .env("PLUGIN_WORKER_HEAP_MB", config.nodejs_worker_heap_mb.to_string())
            .env("PLUGIN_POOL_SOCKET_BACKLOG", config.pool_socket_backlog.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                PluginError::PluginExecutionError(format!("Failed to restart pool server: {e}"))
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
                    tracing::info!("Plugin pool server restarted successfully");
                }
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    return Err(PluginError::PluginExecutionError(
                        "Timeout waiting for pool server to restart".to_string(),
                    ))
                }
            }
        }

        *process_guard = Some(child);

        {
            let mut initialized = self.initialized.write().await;
            *initialized = true;
        }

        // Enable recovery mode - gradually ramp up request allowance
        self.recovery_allowance.store(10, Ordering::Relaxed); // Start at 10%
        self.recovery_mode.store(true, Ordering::Relaxed);

        // Close the circuit breaker after successful restart
        self.circuit_breaker.force_close();

        // Record restart time for backoff calculation
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.last_restart_time_ms.store(now, Ordering::Relaxed);

        tracing::info!("Recovery mode enabled - requests will gradually increase from 10%");

        Ok(())
    }

    /// Get current circuit breaker state for monitoring
    pub fn circuit_state(&self) -> CircuitState {
        self.circuit_breaker.state()
    }

    /// Get average response time in ms (for monitoring)
    pub fn avg_response_time_ms(&self) -> u32 {
        self.circuit_breaker.avg_response_time()
    }

    /// Check if currently in recovery mode
    pub fn is_recovering(&self) -> bool {
        self.recovery_mode.load(Ordering::Relaxed)
    }

    /// Get current recovery allowance percentage (0-100)
    pub fn recovery_allowance_percent(&self) -> u32 {
        self.recovery_allowance.load(Ordering::Relaxed)
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
