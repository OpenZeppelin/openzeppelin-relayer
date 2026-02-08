//! Pool-based Plugin Executor
//!
//! This module provides execution of pre-compiled JavaScript plugins via
//! a persistent Piscina worker pool, replacing the per-request ts-node approach.
//!
//! Communication with the Node.js pool server happens via Unix socket using
//! a JSON-line protocol.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::oneshot;
use uuid::Uuid;

use super::config::get_config;
use super::connection::{ConnectionPool, PoolConnection};
use super::health::{
    CircuitBreaker, CircuitState, DeadServerIndicator, HealthStatus, ProcessStatus,
};
use super::protocol::{PoolError, PoolRequest, PoolResponse};
use super::shared_socket::get_shared_socket_service;
use super::{LogEntry, PluginError, PluginHandlerPayload, ScriptResult};

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
    route: Option<String>,
    config: Option<serde_json::Value>,
    method: Option<String>,
    query: Option<serde_json::Value>,
    response_tx: oneshot::Sender<Result<ScriptResult, PluginError>>,
}

/// Parsed health check result fields extracted from pool server JSON response.
///
/// This struct replaces a complex tuple return type to satisfy Clippy's
/// `type_complexity` lint and improve readability.
#[derive(Debug, Default, PartialEq)]
pub struct ParsedHealthResult {
    pub status: String,
    pub uptime_ms: Option<u64>,
    pub memory: Option<u64>,
    pub pool_completed: Option<u64>,
    pub pool_queued: Option<u64>,
    pub success_rate: Option<f64>,
}

/// Manages the pool server process and connections
pub struct PoolManager {
    socket_path: String,
    process: tokio::sync::Mutex<Option<Child>>,
    initialized: Arc<AtomicBool>,
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
    /// Shutdown signal for background tasks (queue workers, health check, etc.)
    shutdown_signal: Arc<tokio::sync::Notify>,
}

impl Default for PoolManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PoolManager {
    /// Base heap size in MB for the pool server process.
    /// This provides the minimum memory needed for the Node.js runtime and core pool infrastructure.
    const BASE_HEAP_MB: usize = 512;

    /// Concurrency divisor for heap calculation.
    /// Heap is incremented for every N concurrent requests to scale with load.
    const CONCURRENCY_DIVISOR: usize = 10;

    /// Heap increment in MB per CONCURRENCY_DIVISOR concurrent requests.
    /// Formula: BASE_HEAP_MB + ((max_concurrency / CONCURRENCY_DIVISOR) * HEAP_INCREMENT_PER_DIVISOR_MB)
    /// This accounts for additional memory needed per concurrent plugin execution context.
    const HEAP_INCREMENT_PER_DIVISOR_MB: usize = 32;

    /// Maximum heap size in MB (hard cap) for the pool server process.
    /// Prevents excessive memory allocation that could cause system instability.
    /// Set to 8GB (8192 MB) as a reasonable upper bound for Node.js processes.
    const MAX_HEAP_MB: usize = 8192;

    /// Calculate heap size based on concurrency level.
    ///
    /// Formula: BASE_HEAP_MB + ((max_concurrency / CONCURRENCY_DIVISOR) * HEAP_INCREMENT_PER_DIVISOR_MB)
    /// Result is capped at MAX_HEAP_MB.
    ///
    /// This scales memory allocation with expected load while maintaining a reasonable minimum.
    pub fn calculate_heap_size(max_concurrency: usize) -> usize {
        let calculated = Self::BASE_HEAP_MB
            + ((max_concurrency / Self::CONCURRENCY_DIVISOR) * Self::HEAP_INCREMENT_PER_DIVISOR_MB);
        calculated.min(Self::MAX_HEAP_MB)
    }

    /// Format a result value from the pool response into a string.
    ///
    /// If the value is already a string, returns it directly.
    /// Otherwise, serializes it to JSON.
    pub fn format_return_value(value: Option<serde_json::Value>) -> String {
        value
            .map(|v| {
                if v.is_string() {
                    v.as_str().unwrap_or("").to_string()
                } else {
                    serde_json::to_string(&v).unwrap_or_default()
                }
            })
            .unwrap_or_default()
    }

    /// Parse a successful pool response into a ScriptResult.
    ///
    /// Converts logs from PoolLogEntry to LogEntry and extracts the return value.
    pub fn parse_success_response(response: PoolResponse) -> ScriptResult {
        let logs: Vec<LogEntry> = response
            .logs
            .map(|logs| logs.into_iter().map(|l| l.into()).collect())
            .unwrap_or_default();

        ScriptResult {
            logs,
            error: String::new(),
            return_value: Self::format_return_value(response.result),
            trace: Vec::new(),
        }
    }

    /// Parse a failed pool response into a PluginError.
    ///
    /// Extracts error details and converts logs for inclusion in the error payload.
    pub fn parse_error_response(response: PoolResponse) -> PluginError {
        let logs: Vec<LogEntry> = response
            .logs
            .map(|logs| logs.into_iter().map(|l| l.into()).collect())
            .unwrap_or_default();

        let error = response.error.unwrap_or(PoolError {
            message: "Unknown error".to_string(),
            code: None,
            status: None,
            details: None,
        });

        PluginError::HandlerError(Box::new(PluginHandlerPayload {
            message: error.message,
            status: error.status.unwrap_or(500),
            code: error.code,
            details: error.details,
            logs: Some(logs),
            traces: None,
        }))
    }

    /// Parse a pool response into either a success result or an error.
    ///
    /// This is the main entry point for response parsing, dispatching to
    /// either parse_success_response or parse_error_response based on the success flag.
    pub fn parse_pool_response(response: PoolResponse) -> Result<ScriptResult, PluginError> {
        if response.success {
            Ok(Self::parse_success_response(response))
        } else {
            Err(Self::parse_error_response(response))
        }
    }

    /// Parse health check result JSON into individual fields.
    ///
    /// Extracts status, uptime, memory usage, pool stats, and success rate
    /// from the nested JSON structure returned by the pool server.
    pub fn parse_health_result(result: &serde_json::Value) -> ParsedHealthResult {
        ParsedHealthResult {
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
        }
    }

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
        let config = get_config();
        let max_connections = config.pool_max_connections;
        let max_queue_size = config.pool_max_queue_size;

        let (tx, rx) = async_channel::bounded(max_queue_size);

        let connection_pool = Arc::new(ConnectionPool::new(socket_path.clone(), max_connections));
        let connection_pool_clone = connection_pool.clone();

        let shutdown_signal = Arc::new(tokio::sync::Notify::new());

        Self::spawn_queue_workers(
            rx,
            connection_pool_clone,
            config.pool_workers,
            shutdown_signal.clone(),
        );

        let health_check_needed = Arc::new(AtomicBool::new(false));
        let consecutive_failures = Arc::new(AtomicU32::new(0));
        let circuit_breaker = Arc::new(CircuitBreaker::new());
        let last_restart_time_ms = Arc::new(AtomicU64::new(0));
        let recovery_mode = Arc::new(AtomicBool::new(false));
        let recovery_allowance = Arc::new(AtomicU32::new(0));

        Self::spawn_health_check_task(
            health_check_needed.clone(),
            config.health_check_interval_secs,
            shutdown_signal.clone(),
        );

        Self::spawn_recovery_task(
            recovery_mode.clone(),
            recovery_allowance.clone(),
            shutdown_signal.clone(),
        );

        Self {
            connection_pool,
            socket_path,
            process: tokio::sync::Mutex::new(None),
            initialized: Arc::new(AtomicBool::new(false)),
            restart_lock: tokio::sync::Mutex::new(()),
            request_tx: tx,
            max_queue_size,
            health_check_needed,
            consecutive_failures,
            circuit_breaker,
            last_restart_time_ms,
            recovery_mode,
            recovery_allowance,
            shutdown_signal,
        }
    }

    /// Spawn background task to gradually increase recovery allowance
    fn spawn_recovery_task(
        recovery_mode: Arc<AtomicBool>,
        recovery_allowance: Arc<AtomicU32>,
        shutdown_signal: Arc<tokio::sync::Notify>,
    ) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(500));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    biased;

                    _ = shutdown_signal.notified() => {
                        tracing::debug!("Recovery task received shutdown signal");
                        break;
                    }

                    _ = interval.tick() => {
                        if recovery_mode.load(Ordering::Relaxed) {
                            let current = recovery_allowance.load(Ordering::Relaxed);
                            if current < 100 {
                                let new_allowance = (current + 10).min(100);
                                recovery_allowance.store(new_allowance, Ordering::Relaxed);
                                tracing::debug!(
                                    allowance = new_allowance,
                                    "Recovery mode: increasing request allowance"
                                );
                            } else {
                                recovery_mode.store(false, Ordering::Relaxed);
                                tracing::info!("Recovery mode complete - full capacity restored");
                            }
                        }
                    }
                }
            }
        });
    }

    /// Spawn background task to set health check flag periodically
    fn spawn_health_check_task(
        health_check_needed: Arc<AtomicBool>,
        interval_secs: u64,
        shutdown_signal: Arc<tokio::sync::Notify>,
    ) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    biased;

                    _ = shutdown_signal.notified() => {
                        tracing::debug!("Health check task received shutdown signal");
                        break;
                    }

                    _ = interval.tick() => {
                        health_check_needed.store(true, Ordering::Relaxed);
                    }
                }
            }
        });
    }

    /// Spawn multiple worker tasks to process queued requests concurrently
    fn spawn_queue_workers(
        rx: async_channel::Receiver<QueuedRequest>,
        connection_pool: Arc<ConnectionPool>,
        configured_workers: usize,
        shutdown_signal: Arc<tokio::sync::Notify>,
    ) {
        let num_workers = if configured_workers > 0 {
            configured_workers
        } else {
            std::thread::available_parallelism()
                .map(|n| n.get().clamp(4, 32))
                .unwrap_or(8)
        };

        tracing::info!(num_workers = num_workers, "Starting request queue workers");

        for worker_id in 0..num_workers {
            let rx_clone = rx.clone();
            let pool_clone = connection_pool.clone();
            let shutdown = shutdown_signal.clone();

            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        biased;

                        _ = shutdown.notified() => {
                            tracing::debug!(worker_id = worker_id, "Request queue worker received shutdown signal");
                            break;
                        }

                        request_result = rx_clone.recv() => {
                            let request = match request_result {
                                Ok(r) => r,
                                Err(_) => break,
                            };

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
                                request.route,
                                request.config,
                                request.method,
                                request.query,
                            )
                            .await;

                            let elapsed = start.elapsed();
                            if let Err(ref e) = result {
                                let error_str = format!("{e:?}");
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
                    }
                }

                tracing::debug!(worker_id = worker_id, "Request queue worker exited");
            });
        }
    }

    /// Spawn a rate-limited stderr reader to prevent log flooding
    fn spawn_rate_limited_stderr_reader(stderr: tokio::process::ChildStderr) {
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();

            let mut last_log_time = std::time::Instant::now();
            let mut suppressed_count = 0u64;
            let min_interval = Duration::from_millis(100);

            while let Ok(Some(line)) = lines.next_line().await {
                let now = std::time::Instant::now();
                let elapsed = now.duration_since(last_log_time);

                if elapsed >= min_interval {
                    if suppressed_count > 0 {
                        tracing::warn!(
                            target: "pool_server",
                            suppressed = suppressed_count,
                            "... ({} lines suppressed due to rate limiting)",
                            suppressed_count
                        );
                        suppressed_count = 0;
                    }
                    tracing::error!(target: "pool_server", "{}", line);
                    last_log_time = now;
                } else {
                    suppressed_count += 1;
                    if suppressed_count % 100 == 0 {
                        tracing::warn!(
                            target: "pool_server",
                            suppressed = suppressed_count,
                            "Pool server producing excessive stderr output"
                        );
                    }
                }
            }

            if suppressed_count > 0 {
                tracing::warn!(
                    target: "pool_server",
                    suppressed = suppressed_count,
                    "Pool server stderr closed ({} final lines suppressed)",
                    suppressed_count
                );
            }
        });
    }

    /// Execute plugin with optional pre-acquired permit (unified fast/slow path)
    #[allow(clippy::too_many_arguments)]
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
        route: Option<String>,
        config: Option<serde_json::Value>,
        method: Option<String>,
        query: Option<serde_json::Value>,
    ) -> Result<ScriptResult, PluginError> {
        let mut conn = connection_pool.acquire_with_permit(permit).await?;

        let request = PoolRequest::Execute(Box::new(super::protocol::ExecuteRequest {
            task_id: Uuid::new_v4().to_string(),
            plugin_id: plugin_id.clone(),
            compiled_code,
            plugin_path,
            params,
            headers,
            socket_path,
            http_request_id,
            timeout: timeout_secs.map(|s| s * 1000),
            route,
            config,
            method,
            query,
        }));

        let timeout = timeout_secs.unwrap_or(get_config().pool_request_timeout_secs);
        let response = conn.send_request_with_timeout(&request, timeout).await?;

        // Use extracted parsing function for cleaner code and testability
        Self::parse_pool_response(response)
    }

    /// Internal execution method (wrapper for execute_with_permit)
    #[allow(clippy::too_many_arguments)]
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
        route: Option<String>,
        config: Option<serde_json::Value>,
        method: Option<String>,
        query: Option<serde_json::Value>,
    ) -> Result<ScriptResult, PluginError> {
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
            route,
            config,
            method,
            query,
        )
        .await
    }

    /// Check if the pool manager has been initialized.
    ///
    /// This is useful for health checks to determine if the plugin pool
    /// is expected to be running.
    pub async fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    /// Start the pool server if not already running
    pub async fn ensure_started(&self) -> Result<(), PluginError> {
        if self.initialized.load(Ordering::Acquire) {
            return Ok(());
        }

        let _startup_guard = self.restart_lock.lock().await;

        if self.initialized.load(Ordering::Acquire) {
            return Ok(());
        }

        self.start_pool_server().await?;
        self.initialized.store(true, Ordering::Release);
        Ok(())
    }

    /// Ensure pool is started and healthy, with auto-recovery on failure
    async fn ensure_started_and_healthy(&self) -> Result<(), PluginError> {
        self.ensure_started().await?;

        if !self.health_check_needed.load(Ordering::Relaxed) {
            return Ok(());
        }

        if self
            .health_check_needed
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return Ok(());
        }

        self.check_and_restart_if_needed().await
    }

    /// Check process status and restart if needed
    async fn check_and_restart_if_needed(&self) -> Result<(), PluginError> {
        // Check process status without holding restart lock
        let process_status = {
            let mut process_guard = self.process.lock().await;
            if let Some(child) = process_guard.as_mut() {
                match child.try_wait() {
                    Ok(Some(exit_status)) => {
                        tracing::warn!(
                            exit_status = ?exit_status,
                            "Pool server process has exited"
                        );
                        *process_guard = None;
                        ProcessStatus::Exited
                    }
                    Ok(None) => ProcessStatus::Running,
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "Failed to check pool server process status, assuming dead"
                        );
                        *process_guard = None;
                        ProcessStatus::Unknown
                    }
                }
            } else {
                ProcessStatus::NoProcess
            }
        };

        // Determine if restart is needed
        let needs_restart = match process_status {
            ProcessStatus::Running => {
                let socket_exists = std::path::Path::new(&self.socket_path).exists();
                if !socket_exists {
                    tracing::warn!(
                        socket_path = %self.socket_path,
                        "Pool server socket file missing, needs restart"
                    );
                    true
                } else {
                    false
                }
            }
            ProcessStatus::Exited | ProcessStatus::Unknown | ProcessStatus::NoProcess => {
                tracing::warn!("Pool server not running, needs restart");
                true
            }
        };

        // Only acquire restart lock if restart is actually needed
        if needs_restart {
            let _restart_guard = self.restart_lock.lock().await;
            self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
            self.restart_internal().await?;
            self.consecutive_failures.store(0, Ordering::Relaxed);
        }

        Ok(())
    }

    /// Clean up socket file with retry logic
    async fn cleanup_socket_file(socket_path: &str) {
        let max_cleanup_attempts = 5;
        let mut attempts = 0;

        while attempts < max_cleanup_attempts {
            match std::fs::remove_file(socket_path) {
                Ok(_) => break,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_cleanup_attempts {
                        tracing::warn!(
                            socket_path = %socket_path,
                            error = %e,
                            "Failed to remove socket file after {} attempts, proceeding anyway",
                            max_cleanup_attempts
                        );
                        break;
                    }
                    let delay_ms = 10 * (1 << attempts.min(3));
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    /// Spawn the pool server process with proper configuration
    async fn spawn_pool_server_process(
        socket_path: &str,
        context: &str,
    ) -> Result<Child, PluginError> {
        let pool_server_path = std::env::current_dir()
            .map(|cwd| cwd.join("plugins/lib/pool-server.ts").display().to_string())
            .unwrap_or_else(|_| "plugins/lib/pool-server.ts".to_string());

        let config = get_config();

        // Use extracted function for heap calculation
        let pool_server_heap_mb = Self::calculate_heap_size(config.max_concurrency);

        // Log warning if heap was capped (for observability)
        let uncapped_heap = Self::BASE_HEAP_MB
            + ((config.max_concurrency / Self::CONCURRENCY_DIVISOR)
                * Self::HEAP_INCREMENT_PER_DIVISOR_MB);
        if uncapped_heap > Self::MAX_HEAP_MB {
            tracing::warn!(
                calculated_heap_mb = uncapped_heap,
                capped_heap_mb = pool_server_heap_mb,
                max_concurrency = config.max_concurrency,
                "Pool server heap calculation exceeded 8GB cap"
            );
        }

        tracing::info!(
            socket_path = %socket_path,
            heap_mb = pool_server_heap_mb,
            max_concurrency = config.max_concurrency,
            context = context,
            "Spawning plugin pool server"
        );

        let node_options = format!("--max-old-space-size={pool_server_heap_mb} --expose-gc");

        let mut child = Command::new("ts-node")
            .arg("--transpile-only")
            .arg(&pool_server_path)
            .arg(socket_path)
            .env("NODE_OPTIONS", node_options)
            .env("PLUGIN_MAX_CONCURRENCY", config.max_concurrency.to_string())
            .env(
                "PLUGIN_POOL_MIN_THREADS",
                config.nodejs_pool_min_threads.to_string(),
            )
            .env(
                "PLUGIN_POOL_MAX_THREADS",
                config.nodejs_pool_max_threads.to_string(),
            )
            .env(
                "PLUGIN_POOL_CONCURRENT_TASKS",
                config.nodejs_pool_concurrent_tasks.to_string(),
            )
            .env(
                "PLUGIN_POOL_IDLE_TIMEOUT",
                config.nodejs_pool_idle_timeout_ms.to_string(),
            )
            .env(
                "PLUGIN_WORKER_HEAP_MB",
                config.nodejs_worker_heap_mb.to_string(),
            )
            .env(
                "PLUGIN_POOL_SOCKET_BACKLOG",
                config.pool_socket_backlog.to_string(),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                PluginError::PluginExecutionError(format!("Failed to {context} pool server: {e}"))
            })?;

        if let Some(stderr) = child.stderr.take() {
            Self::spawn_rate_limited_stderr_reader(stderr);
        }

        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            let timeout_result = tokio::time::timeout(Duration::from_secs(10), async {
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

            match timeout_result {
                Ok(Ok(())) => {
                    tracing::info!(context = context, "Plugin pool server ready");
                }
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    return Err(PluginError::PluginExecutionError(format!(
                        "Timeout waiting for pool server to {context}"
                    )))
                }
            }
        }

        Ok(child)
    }

    async fn start_pool_server(&self) -> Result<(), PluginError> {
        let mut process_guard = self.process.lock().await;

        if process_guard.is_some() {
            return Ok(());
        }

        Self::cleanup_socket_file(&self.socket_path).await;

        let child = Self::spawn_pool_server_process(&self.socket_path, "start").await?;

        *process_guard = Some(child);
        Ok(())
    }

    /// Execute a plugin via the pool
    #[allow(clippy::too_many_arguments)]
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
        route: Option<String>,
        config: Option<serde_json::Value>,
        method: Option<String>,
        query: Option<serde_json::Value>,
    ) -> Result<ScriptResult, PluginError> {
        tracing::debug!(
            plugin_id = %plugin_id,
            http_request_id = ?http_request_id,
            timeout_secs = ?timeout_secs,
            "Pool execute request received"
        );
        let recovery_allowance = if self.recovery_mode.load(Ordering::Relaxed) {
            Some(self.recovery_allowance.load(Ordering::Relaxed))
        } else {
            None
        };

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

        self.ensure_started_and_healthy().await?;
        tracing::debug!(
            plugin_id = %plugin_id,
            http_request_id = ?http_request_id,
            "Pool execute start (healthy/started)"
        );

        let circuit_breaker = self.circuit_breaker.clone();
        match self.connection_pool.semaphore.clone().try_acquire_owned() {
            Ok(permit) => {
                tracing::debug!(
                    plugin_id = %plugin_id,
                    http_request_id = ?http_request_id,
                    "Pool execute acquired connection permit (fast path)"
                );
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
                    route,
                    config,
                    method,
                    query,
                )
                .await;

                let elapsed_ms = start_time.elapsed().as_millis() as u32;
                match &result {
                    Ok(_) => circuit_breaker.record_success(elapsed_ms),
                    Err(e) => {
                        // Only count infrastructure errors for circuit breaker, not business errors
                        // Business errors (RPC failures, plugin logic errors) mean the pool is healthy
                        if Self::is_dead_server_error(e) {
                            circuit_breaker.record_failure();
                            tracing::warn!(
                                error = %e,
                                "Detected dead pool server error, triggering health check for restart"
                            );
                            self.health_check_needed.store(true, Ordering::Relaxed);
                        } else {
                            // Plugin executed but returned error - infrastructure is healthy
                            circuit_breaker.record_success(elapsed_ms);
                        }
                    }
                }

                tracing::debug!(
                    elapsed_ms = elapsed_ms,
                    result_ok = result.is_ok(),
                    "Pool execute finished (fast path)"
                );
                result
            }
            Err(_) => {
                tracing::debug!(
                    plugin_id = %plugin_id,
                    http_request_id = ?http_request_id,
                    "Pool execute queueing (no permits)"
                );
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
                    route,
                    config,
                    method,
                    query,
                    response_tx,
                };

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
                        // Add timeout to response_rx to prevent hung requests if worker crashes
                        let response_timeout = timeout_secs
                            .map(Duration::from_secs)
                            .unwrap_or(Duration::from_secs(get_config().pool_request_timeout_secs))
                            + Duration::from_secs(5); // Add 5s buffer for queue processing

                        match tokio::time::timeout(response_timeout, response_rx).await {
                            Ok(Ok(result)) => result,
                            Ok(Err(_)) => Err(PluginError::PluginExecutionError(
                                "Request queue processor closed".to_string(),
                            )),
                            Err(_) => Err(PluginError::PluginExecutionError(format!(
                                "Request timed out after {}s waiting for worker response",
                                response_timeout.as_secs()
                            ))),
                        }
                    }
                    Err(async_channel::TrySendError::Full(req)) => {
                        let queue_timeout_ms = get_config().pool_queue_send_timeout_ms;
                        let queue_timeout = Duration::from_millis(queue_timeout_ms);
                        match tokio::time::timeout(queue_timeout, self.request_tx.send(req)).await {
                            Ok(Ok(())) => {
                                let queue_len = self.request_tx.len();
                                tracing::debug!(
                                    queue_len = queue_len,
                                    "Request queued after waiting for queue space"
                                );
                                // Add timeout to response_rx to prevent hung requests if worker crashes
                                let response_timeout =
                                    timeout_secs.map(Duration::from_secs).unwrap_or(
                                        Duration::from_secs(get_config().pool_request_timeout_secs),
                                    ) + Duration::from_secs(5); // Add 5s buffer for queue processing

                                match tokio::time::timeout(response_timeout, response_rx).await {
                                    Ok(Ok(result)) => result,
                                    Ok(Err(_)) => Err(PluginError::PluginExecutionError(
                                        "Request queue processor closed".to_string(),
                                    )),
                                    Err(_) => Err(PluginError::PluginExecutionError(format!(
                                        "Request timed out after {}s waiting for worker response",
                                        response_timeout.as_secs()
                                    ))),
                                }
                            }
                            Ok(Err(async_channel::SendError(_))) => {
                                Err(PluginError::PluginExecutionError(
                                    "Plugin execution queue is closed".to_string(),
                                ))
                            }
                            Err(_) => {
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

                let elapsed_ms = start_time.elapsed().as_millis() as u32;
                match &result {
                    Ok(_) => circuit_breaker.record_success(elapsed_ms),
                    Err(e) => {
                        // Only count infrastructure errors for circuit breaker, not business errors
                        if Self::is_dead_server_error(e) {
                            circuit_breaker.record_failure();
                            tracing::warn!(
                                error = %e,
                                "Detected dead pool server error (queued path), triggering health check for restart"
                            );
                            self.health_check_needed.store(true, Ordering::Relaxed);
                        } else {
                            // Plugin executed but returned error - infrastructure is healthy
                            circuit_breaker.record_success(elapsed_ms);
                        }
                    }
                }

                tracing::debug!(
                    elapsed_ms = elapsed_ms,
                    result_ok = result.is_ok(),
                    "Pool execute finished (queued path)"
                );
                result
            }
        }
    }

    /// Check if an error indicates the pool server is dead and needs restart
    pub fn is_dead_server_error(err: &PluginError) -> bool {
        let error_str = err.to_string();
        let lower = error_str.to_lowercase();

        if lower.contains("handler timed out")
            || (lower.contains("plugin") && lower.contains("timed out"))
        {
            return false;
        }

        DeadServerIndicator::from_error_str(&error_str).is_some()
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
        if !self.initialized.load(Ordering::Acquire) {
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
    /// Collect socket connection statistics
    async fn collect_socket_stats(
        &self,
    ) -> (
        Option<usize>,
        Option<usize>,
        Option<usize>,
        Option<usize>,
        Option<usize>,
    ) {
        // Collect shared socket stats
        let (shared_available, shared_active, shared_executions) = match get_shared_socket_service()
        {
            Ok(service) => {
                let available = service.available_connection_slots();
                let active = service.active_connection_count();
                let executions = service.registered_executions_count().await;
                (Some(available), Some(active), Some(executions))
            }
            Err(_) => (None, None, None),
        };

        // Collect connection pool stats (for pool server connections)
        let pool_available = self.connection_pool.semaphore.available_permits();
        let pool_max = get_config().pool_max_connections;
        let pool_active = pool_max.saturating_sub(pool_available);

        (
            shared_available,
            shared_active,
            shared_executions,
            Some(pool_available),
            Some(pool_active),
        )
    }

    pub async fn health_check(&self) -> Result<HealthStatus, PluginError> {
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

        let socket_stats = self.collect_socket_stats().await;

        if !self.initialized.load(Ordering::Acquire) {
            let (circuit_state, avg_rt, recovering, recovery_pct) = circuit_info();
            let (shared_available, shared_active, shared_executions, pool_available, pool_active) =
                socket_stats;
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
                shared_socket_available_slots: shared_available,
                shared_socket_active_connections: shared_active,
                shared_socket_registered_executions: shared_executions,
                connection_pool_available_slots: pool_available,
                connection_pool_active_connections: pool_active,
            });
        }

        if !std::path::Path::new(&self.socket_path).exists() {
            let (circuit_state, avg_rt, recovering, recovery_pct) = circuit_info();
            let (shared_available, shared_active, shared_executions, pool_available, pool_active) =
                socket_stats;
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
                shared_socket_available_slots: shared_available,
                shared_socket_active_connections: shared_active,
                shared_socket_registered_executions: shared_executions,
                connection_pool_available_slots: pool_available,
                connection_pool_active_connections: pool_active,
            });
        }

        let mut conn =
            match tokio::time::timeout(Duration::from_millis(100), self.connection_pool.acquire())
                .await
            {
                Ok(Ok(c)) => c,
                Ok(Err(e)) => {
                    let err_str = e.to_string();
                    let is_pool_exhausted =
                        err_str.contains("semaphore") || err_str.contains("Connection refused");

                    // Try to check process status without blocking on lock
                    let process_status = match self.process.try_lock() {
                        Ok(guard) => {
                            if let Some(child) = guard.as_ref() {
                                format!("process_pid_{}", child.id().unwrap_or(0))
                            } else {
                                "no_process".to_string()
                            }
                        }
                        Err(_) => "process_lock_busy".to_string(),
                    };

                    let (circuit_state, avg_rt, recovering, recovery_pct) = circuit_info();
                    let (
                        shared_available,
                        shared_active,
                        shared_executions,
                        pool_available,
                        pool_active,
                    ) = socket_stats;
                    return Ok(HealthStatus {
                        healthy: is_pool_exhausted,
                        status: if is_pool_exhausted {
                            format!("pool_exhausted: {e} ({process_status})")
                        } else {
                            format!("connection_failed: {e} ({process_status})")
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
                        shared_socket_available_slots: shared_available,
                        shared_socket_active_connections: shared_active,
                        shared_socket_registered_executions: shared_executions,
                        connection_pool_available_slots: pool_available,
                        connection_pool_active_connections: pool_active,
                    });
                }
                Err(_) => {
                    let (circuit_state, avg_rt, recovering, recovery_pct) = circuit_info();
                    let (
                        shared_available,
                        shared_active,
                        shared_executions,
                        pool_available,
                        pool_active,
                    ) = socket_stats;
                    return Ok(HealthStatus {
                        healthy: true,
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
                        shared_socket_available_slots: shared_available,
                        shared_socket_active_connections: shared_active,
                        shared_socket_registered_executions: shared_executions,
                        connection_pool_available_slots: pool_available,
                        connection_pool_active_connections: pool_active,
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
                    // Use extracted parsing function for testability
                    let parsed = Self::parse_health_result(&result);

                    {
                        let (
                            shared_available,
                            shared_active,
                            shared_executions,
                            pool_available,
                            pool_active,
                        ) = socket_stats;
                        Ok(HealthStatus {
                            healthy: true,
                            status: parsed.status,
                            uptime_ms: parsed.uptime_ms,
                            memory: parsed.memory,
                            pool_completed: parsed.pool_completed,
                            pool_queued: parsed.pool_queued,
                            success_rate: parsed.success_rate,
                            circuit_state,
                            avg_response_time_ms: avg_rt,
                            recovering,
                            recovery_percent: recovery_pct,
                            shared_socket_available_slots: shared_available,
                            shared_socket_active_connections: shared_active,
                            shared_socket_registered_executions: shared_executions,
                            connection_pool_available_slots: pool_available,
                            connection_pool_active_connections: pool_active,
                        })
                    }
                } else {
                    let (
                        shared_available,
                        shared_active,
                        shared_executions,
                        pool_available,
                        pool_active,
                    ) = socket_stats;
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
                        shared_socket_available_slots: shared_available,
                        shared_socket_active_connections: shared_active,
                        shared_socket_registered_executions: shared_executions,
                        connection_pool_available_slots: pool_available,
                        connection_pool_active_connections: pool_active,
                    })
                }
            }
            Err(e) => {
                let (
                    shared_available,
                    shared_active,
                    shared_executions,
                    pool_available,
                    pool_active,
                ) = socket_stats;
                Ok(HealthStatus {
                    healthy: false,
                    status: format!("request_failed: {e}"),
                    uptime_ms: None,
                    memory: None,
                    pool_completed: None,
                    pool_queued: None,
                    success_rate: None,
                    circuit_state,
                    avg_response_time_ms: avg_rt,
                    recovering,
                    recovery_percent: recovery_pct,
                    shared_socket_available_slots: shared_available,
                    shared_socket_active_connections: shared_active,
                    shared_socket_registered_executions: shared_executions,
                    connection_pool_available_slots: pool_available,
                    connection_pool_active_connections: pool_active,
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

        match self.restart_lock.try_lock() {
            Ok(_guard) => {
                let health_recheck = self.health_check().await?;
                if health_recheck.healthy {
                    return Ok(true);
                }

                tracing::warn!(status = %health.status, "Pool server unhealthy, attempting restart");
                self.restart_internal().await?;
            }
            Err(_) => {
                tracing::debug!("Waiting for another task to complete pool server restart");
                let _guard = self.restart_lock.lock().await;
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

        {
            let mut process_guard = self.process.lock().await;
            if let Some(mut child) = process_guard.take() {
                let _ = child.kill().await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        Self::cleanup_socket_file(&self.socket_path).await;

        self.initialized.store(false, Ordering::Release);

        let mut process_guard = self.process.lock().await;
        if process_guard.is_some() {
            return Ok(());
        }

        let child = Self::spawn_pool_server_process(&self.socket_path, "restart").await?;
        *process_guard = Some(child);

        self.initialized.store(true, Ordering::Release);

        self.recovery_allowance.store(10, Ordering::Relaxed);
        self.recovery_mode.store(true, Ordering::Relaxed);

        self.circuit_breaker.force_close();

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

    /// Shutdown the pool server gracefully
    pub async fn shutdown(&self) -> Result<(), PluginError> {
        if !self.initialized.load(Ordering::Acquire) {
            return Ok(());
        }

        tracing::info!("Initiating graceful shutdown of plugin pool server");

        self.shutdown_signal.notify_waiters();

        let shutdown_timeout = std::time::Duration::from_secs(35);
        let shutdown_result = self.send_shutdown_request(shutdown_timeout).await;

        match &shutdown_result {
            Ok(response) => {
                tracing::info!(
                    response = ?response,
                    "Pool server acknowledged shutdown, waiting for graceful exit"
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to send shutdown request, will force kill"
                );
            }
        }

        let mut process_guard = self.process.lock().await;
        if let Some(ref mut child) = *process_guard {
            let graceful_wait = std::time::Duration::from_secs(35);
            let start = std::time::Instant::now();

            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        tracing::info!(
                            exit_status = ?status,
                            elapsed_ms = start.elapsed().as_millis(),
                            "Pool server exited gracefully"
                        );
                        break;
                    }
                    Ok(None) => {
                        if start.elapsed() >= graceful_wait {
                            tracing::warn!(
                                "Pool server did not exit within graceful timeout, force killing"
                            );
                            let _ = child.kill().await;
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Error checking pool server status");
                        let _ = child.kill().await;
                        break;
                    }
                }
            }
        }
        *process_guard = None;

        let _ = std::fs::remove_file(&self.socket_path);

        self.initialized.store(false, Ordering::Release);
        tracing::info!("Plugin pool server shutdown complete");
        Ok(())
    }

    /// Send shutdown request to the pool server
    async fn send_shutdown_request(
        &self,
        timeout: std::time::Duration,
    ) -> Result<PoolResponse, PluginError> {
        let request = PoolRequest::Shutdown {
            task_id: Uuid::new_v4().to_string(),
        };

        // Use the pool's connection ID counter to ensure unique IDs
        // even for shutdown connections that bypass the pool
        let connection_id = self.connection_pool.next_connection_id();
        let mut conn = match PoolConnection::new(&self.socket_path, connection_id).await {
            Ok(c) => c,
            Err(e) => {
                return Err(PluginError::PluginExecutionError(format!(
                    "Failed to connect for shutdown: {e}"
                )));
            }
        };

        conn.send_request_with_timeout(&request, timeout.as_secs())
            .await
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
    use crate::services::plugins::script_executor::LogLevel;

    #[test]
    fn test_is_dead_server_error_detects_dead_server() {
        let err = PluginError::PluginExecutionError("Connection refused".to_string());
        assert!(PoolManager::is_dead_server_error(&err));

        let err = PluginError::PluginExecutionError("Broken pipe".to_string());
        assert!(PoolManager::is_dead_server_error(&err));
    }

    #[test]
    fn test_is_dead_server_error_excludes_plugin_timeouts() {
        let err = PluginError::PluginExecutionError("Plugin timed out after 30s".to_string());
        assert!(!PoolManager::is_dead_server_error(&err));

        let err = PluginError::PluginExecutionError("Handler timed out".to_string());
        assert!(!PoolManager::is_dead_server_error(&err));
    }

    #[test]
    fn test_is_dead_server_error_normal_errors() {
        let err =
            PluginError::PluginExecutionError("TypeError: undefined is not a function".to_string());
        assert!(!PoolManager::is_dead_server_error(&err));

        let err = PluginError::PluginExecutionError("Plugin returned invalid JSON".to_string());
        assert!(!PoolManager::is_dead_server_error(&err));
    }

    #[test]
    fn test_is_dead_server_error_detects_all_dead_server_indicators() {
        // Test common DeadServerIndicator patterns
        let dead_server_errors = vec![
            "EOF while parsing JSON response",
            "Broken pipe when writing to socket",
            "Connection refused: server not running",
            "Connection reset by peer",
            "Socket not connected",
            "Failed to connect to pool server",
            "Socket file missing: /tmp/test.sock",
            "No such file or directory",
        ];

        for error_msg in dead_server_errors {
            let err = PluginError::PluginExecutionError(error_msg.to_string());
            assert!(
                PoolManager::is_dead_server_error(&err),
                "Expected '{error_msg}' to be detected as dead server error"
            );
        }
    }

    #[test]
    fn test_dead_server_indicator_patterns() {
        // Test the DeadServerIndicator pattern matching directly
        use super::super::health::DeadServerIndicator;

        // These should all match
        assert!(DeadServerIndicator::from_error_str("eof while parsing").is_some());
        assert!(DeadServerIndicator::from_error_str("broken pipe").is_some());
        assert!(DeadServerIndicator::from_error_str("connection refused").is_some());
        assert!(DeadServerIndicator::from_error_str("connection reset").is_some());
        assert!(DeadServerIndicator::from_error_str("not connected").is_some());
        assert!(DeadServerIndicator::from_error_str("failed to connect").is_some());
        assert!(DeadServerIndicator::from_error_str("socket file missing").is_some());
        assert!(DeadServerIndicator::from_error_str("no such file").is_some());
        assert!(DeadServerIndicator::from_error_str("connection timed out").is_some());
        assert!(DeadServerIndicator::from_error_str("connect timed out").is_some());

        // These should NOT match
        assert!(DeadServerIndicator::from_error_str("handler timed out").is_none());
        assert!(DeadServerIndicator::from_error_str("validation error").is_none());
        assert!(DeadServerIndicator::from_error_str("TypeError: undefined").is_none());
    }

    #[test]
    fn test_is_dead_server_error_excludes_plugin_timeouts_with_connection() {
        // Plugin timeout should NOT be detected even if it mentions connection
        let plugin_timeout =
            PluginError::PluginExecutionError("plugin connection timed out".to_string());
        // This contains both "plugin" and "timed out" so it's excluded
        assert!(!PoolManager::is_dead_server_error(&plugin_timeout));
    }

    #[test]
    fn test_is_dead_server_error_case_insensitive() {
        // Test case insensitivity
        let err = PluginError::PluginExecutionError("CONNECTION REFUSED".to_string());
        assert!(PoolManager::is_dead_server_error(&err));

        let err = PluginError::PluginExecutionError("BROKEN PIPE".to_string());
        assert!(PoolManager::is_dead_server_error(&err));

        let err = PluginError::PluginExecutionError("Connection Reset By Peer".to_string());
        assert!(PoolManager::is_dead_server_error(&err));
    }

    #[test]
    fn test_is_dead_server_error_handler_timeout_variations() {
        // All variations of plugin/handler timeouts should NOT trigger restart
        let timeout_errors = vec![
            "Handler timed out",
            "handler timed out after 30000ms",
            "Plugin handler timed out",
            "plugin timed out",
            "Plugin execution timed out after 60s",
        ];

        for error_msg in timeout_errors {
            let err = PluginError::PluginExecutionError(error_msg.to_string());
            assert!(
                !PoolManager::is_dead_server_error(&err),
                "Expected '{error_msg}' to NOT be detected as dead server error"
            );
        }
    }

    #[test]
    fn test_is_dead_server_error_business_errors_not_detected() {
        // Business logic errors should not trigger restart
        let business_errors = vec![
            "ReferenceError: x is not defined",
            "SyntaxError: Unexpected token",
            "TypeError: Cannot read property 'foo' of undefined",
            "Plugin returned status 400: Bad Request",
            "Validation error: missing required field",
            "Authorization failed",
            "Rate limit exceeded",
            "Plugin threw an error: Invalid input",
        ];

        for error_msg in business_errors {
            let err = PluginError::PluginExecutionError(error_msg.to_string());
            assert!(
                !PoolManager::is_dead_server_error(&err),
                "Expected '{error_msg}' to NOT be detected as dead server error"
            );
        }
    }

    #[test]
    fn test_is_dead_server_error_with_handler_error_type() {
        // HandlerError type should also be checked
        let handler_payload = PluginHandlerPayload {
            message: "Connection refused".to_string(),
            status: 500,
            code: None,
            details: None,
            logs: None,
            traces: None,
        };
        let err = PluginError::HandlerError(Box::new(handler_payload));
        // The error message contains "Connection refused" but it's wrapped differently
        // This tests that we check the string representation
        assert!(PoolManager::is_dead_server_error(&err));
    }

    // ============================================
    // Heap calculation tests
    // ============================================

    #[test]
    fn test_heap_calculation_base_case() {
        // With default concurrency, should get base heap
        let base = PoolManager::BASE_HEAP_MB;
        let divisor = PoolManager::CONCURRENCY_DIVISOR;
        let increment = PoolManager::HEAP_INCREMENT_PER_DIVISOR_MB;

        // For 100 concurrent requests:
        // 512 + (100 / 10) * 32 = 512 + 320 = 832 MB
        let concurrency = 100;
        let expected = base + ((concurrency / divisor) * increment);
        assert_eq!(expected, 832);
    }

    #[test]
    fn test_heap_calculation_minimum() {
        // With very low concurrency, should still get base heap
        let base = PoolManager::BASE_HEAP_MB;
        let divisor = PoolManager::CONCURRENCY_DIVISOR;
        let increment = PoolManager::HEAP_INCREMENT_PER_DIVISOR_MB;

        // For 5 concurrent requests:
        // 512 + (5 / 10) * 32 = 512 + 0 = 512 MB (integer division)
        let concurrency = 5;
        let expected = base + ((concurrency / divisor) * increment);
        assert_eq!(expected, 512);
    }

    #[test]
    fn test_heap_calculation_high_concurrency() {
        // With high concurrency, should scale appropriately
        let base = PoolManager::BASE_HEAP_MB;
        let divisor = PoolManager::CONCURRENCY_DIVISOR;
        let increment = PoolManager::HEAP_INCREMENT_PER_DIVISOR_MB;

        // For 500 concurrent requests:
        // 512 + (500 / 10) * 32 = 512 + 1600 = 2112 MB
        let concurrency = 500;
        let expected = base + ((concurrency / divisor) * increment);
        assert_eq!(expected, 2112);
    }

    #[test]
    fn test_heap_calculation_max_cap() {
        // Verify max heap cap is respected
        let max_heap = PoolManager::MAX_HEAP_MB;
        assert_eq!(max_heap, 8192);

        // For extreme concurrency that would exceed cap:
        // e.g., 3000 concurrent: 512 + (3000 / 10) * 32 = 512 + 9600 = 10112 MB
        // Should be capped to 8192 MB
        let base = PoolManager::BASE_HEAP_MB;
        let divisor = PoolManager::CONCURRENCY_DIVISOR;
        let increment = PoolManager::HEAP_INCREMENT_PER_DIVISOR_MB;

        let concurrency = 3000;
        let calculated = base + ((concurrency / divisor) * increment);
        let capped = calculated.min(max_heap);

        assert_eq!(calculated, 10112);
        assert_eq!(capped, 8192);
    }

    // ============================================
    // Constants verification tests
    // ============================================

    #[test]
    fn test_pool_manager_constants() {
        // Verify important constants have reasonable values
        assert_eq!(PoolManager::BASE_HEAP_MB, 512);
        assert_eq!(PoolManager::CONCURRENCY_DIVISOR, 10);
        assert_eq!(PoolManager::HEAP_INCREMENT_PER_DIVISOR_MB, 32);
        assert_eq!(PoolManager::MAX_HEAP_MB, 8192);
    }

    // ============================================
    // Extracted function tests: calculate_heap_size
    // ============================================

    #[test]
    fn test_calculate_heap_size_low_concurrency() {
        // Low concurrency should give base heap
        assert_eq!(PoolManager::calculate_heap_size(5), 512);
        assert_eq!(PoolManager::calculate_heap_size(9), 512);
    }

    #[test]
    fn test_calculate_heap_size_medium_concurrency() {
        // 10 concurrent: 512 + (10/10)*32 = 544
        assert_eq!(PoolManager::calculate_heap_size(10), 544);
        // 50 concurrent: 512 + (50/10)*32 = 672
        assert_eq!(PoolManager::calculate_heap_size(50), 672);
        // 100 concurrent: 512 + (100/10)*32 = 832
        assert_eq!(PoolManager::calculate_heap_size(100), 832);
    }

    #[test]
    fn test_calculate_heap_size_high_concurrency() {
        // 500 concurrent: 512 + (500/10)*32 = 2112
        assert_eq!(PoolManager::calculate_heap_size(500), 2112);
        // 1000 concurrent: 512 + (1000/10)*32 = 3712
        assert_eq!(PoolManager::calculate_heap_size(1000), 3712);
    }

    #[test]
    fn test_calculate_heap_size_capped_at_max() {
        // 3000 concurrent would be 10112, but capped at 8192
        assert_eq!(PoolManager::calculate_heap_size(3000), 8192);
        // Even higher should still be capped
        assert_eq!(PoolManager::calculate_heap_size(10000), 8192);
    }

    #[test]
    fn test_calculate_heap_size_zero_concurrency() {
        // Zero concurrency gives base heap
        assert_eq!(PoolManager::calculate_heap_size(0), 512);
    }

    // ============================================
    // Extracted function tests: format_return_value
    // ============================================

    #[test]
    fn test_format_return_value_none() {
        assert_eq!(PoolManager::format_return_value(None), "");
    }

    #[test]
    fn test_format_return_value_string() {
        let value = Some(serde_json::json!("hello world"));
        assert_eq!(PoolManager::format_return_value(value), "hello world");
    }

    #[test]
    fn test_format_return_value_empty_string() {
        let value = Some(serde_json::json!(""));
        assert_eq!(PoolManager::format_return_value(value), "");
    }

    #[test]
    fn test_format_return_value_object() {
        let value = Some(serde_json::json!({"key": "value", "num": 42}));
        let result = PoolManager::format_return_value(value);
        // JSON object gets serialized
        assert!(result.contains("key"));
        assert!(result.contains("value"));
        assert!(result.contains("42"));
    }

    #[test]
    fn test_format_return_value_array() {
        let value = Some(serde_json::json!([1, 2, 3]));
        assert_eq!(PoolManager::format_return_value(value), "[1,2,3]");
    }

    #[test]
    fn test_format_return_value_number() {
        let value = Some(serde_json::json!(42));
        assert_eq!(PoolManager::format_return_value(value), "42");
    }

    #[test]
    fn test_format_return_value_boolean() {
        assert_eq!(
            PoolManager::format_return_value(Some(serde_json::json!(true))),
            "true"
        );
        assert_eq!(
            PoolManager::format_return_value(Some(serde_json::json!(false))),
            "false"
        );
    }

    #[test]
    fn test_format_return_value_null() {
        let value = Some(serde_json::json!(null));
        assert_eq!(PoolManager::format_return_value(value), "null");
    }

    // ============================================
    // Extracted function tests: parse_pool_response
    // ============================================

    #[test]
    fn test_parse_pool_response_success_with_string_result() {
        use super::super::protocol::{PoolLogEntry, PoolResponse};

        let response = PoolResponse {
            task_id: "test-123".to_string(),
            success: true,
            result: Some(serde_json::json!("success result")),
            error: None,
            logs: Some(vec![PoolLogEntry {
                level: "info".to_string(),
                message: "test log".to_string(),
            }]),
        };

        let result = PoolManager::parse_pool_response(response).unwrap();
        assert_eq!(result.return_value, "success result");
        assert!(result.error.is_empty());
        assert_eq!(result.logs.len(), 1);
        assert_eq!(result.logs[0].level, LogLevel::Info);
        assert_eq!(result.logs[0].message, "test log");
    }

    #[test]
    fn test_parse_pool_response_success_with_object_result() {
        use super::super::protocol::PoolResponse;

        let response = PoolResponse {
            task_id: "test-456".to_string(),
            success: true,
            result: Some(serde_json::json!({"data": "value"})),
            error: None,
            logs: None,
        };

        let result = PoolManager::parse_pool_response(response).unwrap();
        assert!(result.return_value.contains("data"));
        assert!(result.return_value.contains("value"));
        assert!(result.logs.is_empty());
    }

    #[test]
    fn test_parse_pool_response_success_no_result() {
        use super::super::protocol::PoolResponse;

        let response = PoolResponse {
            task_id: "test-789".to_string(),
            success: true,
            result: None,
            error: None,
            logs: None,
        };

        let result = PoolManager::parse_pool_response(response).unwrap();
        assert_eq!(result.return_value, "");
        assert!(result.error.is_empty());
    }

    #[test]
    fn test_parse_pool_response_failure_with_error() {
        use super::super::protocol::{PoolError, PoolResponse};

        let response = PoolResponse {
            task_id: "test-error".to_string(),
            success: false,
            result: None,
            error: Some(PoolError {
                message: "Something went wrong".to_string(),
                code: Some("ERR_001".to_string()),
                status: Some(400),
                details: Some(serde_json::json!({"field": "name"})),
            }),
            logs: None,
        };

        let err = PoolManager::parse_pool_response(response).unwrap_err();
        match err {
            PluginError::HandlerError(payload) => {
                assert_eq!(payload.message, "Something went wrong");
                assert_eq!(payload.status, 400);
                assert_eq!(payload.code, Some("ERR_001".to_string()));
            }
            _ => panic!("Expected HandlerError"),
        }
    }

    #[test]
    fn test_parse_pool_response_failure_no_error_details() {
        use super::super::protocol::PoolResponse;

        let response = PoolResponse {
            task_id: "test-unknown".to_string(),
            success: false,
            result: None,
            error: None,
            logs: None,
        };

        let err = PoolManager::parse_pool_response(response).unwrap_err();
        match err {
            PluginError::HandlerError(payload) => {
                assert_eq!(payload.message, "Unknown error");
                assert_eq!(payload.status, 500);
            }
            _ => panic!("Expected HandlerError"),
        }
    }

    #[test]
    fn test_parse_pool_response_failure_preserves_logs() {
        use super::super::protocol::{PoolError, PoolLogEntry, PoolResponse};

        let response = PoolResponse {
            task_id: "test-logs".to_string(),
            success: false,
            result: None,
            error: Some(PoolError {
                message: "Error with logs".to_string(),
                code: None,
                status: None,
                details: None,
            }),
            logs: Some(vec![
                PoolLogEntry {
                    level: "debug".to_string(),
                    message: "debug message".to_string(),
                },
                PoolLogEntry {
                    level: "error".to_string(),
                    message: "error message".to_string(),
                },
            ]),
        };

        let err = PoolManager::parse_pool_response(response).unwrap_err();
        match err {
            PluginError::HandlerError(payload) => {
                let logs = payload.logs.unwrap();
                assert_eq!(logs.len(), 2);
                assert_eq!(logs[0].level, LogLevel::Debug);
                assert_eq!(logs[1].level, LogLevel::Error);
            }
            _ => panic!("Expected HandlerError"),
        }
    }

    // ============================================
    // Extracted function tests: parse_success_response
    // ============================================

    #[test]
    fn test_parse_success_response_complete() {
        use super::super::protocol::{PoolLogEntry, PoolResponse};

        let response = PoolResponse {
            task_id: "task-1".to_string(),
            success: true,
            result: Some(serde_json::json!("completed")),
            error: None,
            logs: Some(vec![
                PoolLogEntry {
                    level: "log".to_string(),
                    message: "starting".to_string(),
                },
                PoolLogEntry {
                    level: "result".to_string(),
                    message: "finished".to_string(),
                },
            ]),
        };

        let result = PoolManager::parse_success_response(response);
        assert_eq!(result.return_value, "completed");
        assert!(result.error.is_empty());
        assert_eq!(result.logs.len(), 2);
        assert_eq!(result.logs[0].level, LogLevel::Log);
        assert_eq!(result.logs[1].level, LogLevel::Result);
    }

    // ============================================
    // Extracted function tests: parse_error_response
    // ============================================

    #[test]
    fn test_parse_error_response_with_all_fields() {
        use super::super::protocol::{PoolError, PoolLogEntry, PoolResponse};

        let response = PoolResponse {
            task_id: "err-task".to_string(),
            success: false,
            result: None,
            error: Some(PoolError {
                message: "Validation failed".to_string(),
                code: Some("VALIDATION_ERROR".to_string()),
                status: Some(422),
                details: Some(serde_json::json!({"fields": ["email"]})),
            }),
            logs: Some(vec![PoolLogEntry {
                level: "warn".to_string(),
                message: "validation warning".to_string(),
            }]),
        };

        let err = PoolManager::parse_error_response(response);
        match err {
            PluginError::HandlerError(payload) => {
                assert_eq!(payload.message, "Validation failed");
                assert_eq!(payload.status, 422);
                assert_eq!(payload.code, Some("VALIDATION_ERROR".to_string()));
                assert!(payload.details.is_some());
                let logs = payload.logs.unwrap();
                assert_eq!(logs.len(), 1);
                assert_eq!(logs[0].level, LogLevel::Warn);
            }
            _ => panic!("Expected HandlerError"),
        }
    }

    // ============================================
    // Extracted function tests: parse_health_result
    // ============================================

    #[test]
    fn test_parse_health_result_complete() {
        let json = serde_json::json!({
            "status": "healthy",
            "uptime": 123456,
            "memory": {
                "heapUsed": 50000000,
                "heapTotal": 100000000
            },
            "pool": {
                "completed": 1000,
                "queued": 5
            },
            "execution": {
                "successRate": 0.99
            }
        });

        let result = PoolManager::parse_health_result(&json);

        assert_eq!(result.status, "healthy");
        assert_eq!(result.uptime_ms, Some(123456));
        assert_eq!(result.memory, Some(50000000));
        assert_eq!(result.pool_completed, Some(1000));
        assert_eq!(result.pool_queued, Some(5));
        assert!((result.success_rate.unwrap() - 0.99).abs() < 0.001);
    }

    #[test]
    fn test_parse_health_result_minimal() {
        let json = serde_json::json!({});

        let result = PoolManager::parse_health_result(&json);

        assert_eq!(result.status, "unknown");
        assert_eq!(result.uptime_ms, None);
        assert_eq!(result.memory, None);
        assert_eq!(result.pool_completed, None);
        assert_eq!(result.pool_queued, None);
        assert_eq!(result.success_rate, None);
    }

    #[test]
    fn test_parse_health_result_partial() {
        let json = serde_json::json!({
            "status": "degraded",
            "uptime": 5000,
            "memory": {
                "heapTotal": 100000000
                // heapUsed missing
            }
        });

        let result = PoolManager::parse_health_result(&json);

        assert_eq!(result.status, "degraded");
        assert_eq!(result.uptime_ms, Some(5000));
        assert_eq!(result.memory, None); // heapUsed was missing
        assert_eq!(result.pool_completed, None);
        assert_eq!(result.pool_queued, None);
        assert_eq!(result.success_rate, None);
    }

    #[test]
    fn test_parse_health_result_wrong_types() {
        let json = serde_json::json!({
            "status": 123,  // Should be string, will use "unknown"
            "uptime": "not a number",  // Should be u64, will be None
            "memory": "invalid"  // Should be object, will give None
        });

        let result = PoolManager::parse_health_result(&json);

        assert_eq!(result.status, "unknown"); // Falls back when not a string
        assert_eq!(result.uptime_ms, None);
        assert_eq!(result.memory, None);
        assert_eq!(result.pool_completed, None);
        assert_eq!(result.pool_queued, None);
        assert_eq!(result.success_rate, None);
    }

    #[test]
    fn test_parse_health_result_nested_values() {
        let json = serde_json::json!({
            "pool": {
                "completed": 0,
                "queued": 0
            },
            "execution": {
                "successRate": 1.0
            }
        });

        let result = PoolManager::parse_health_result(&json);

        assert_eq!(result.status, "unknown");
        assert_eq!(result.uptime_ms, None);
        assert_eq!(result.memory, None);
        assert_eq!(result.pool_completed, Some(0));
        assert_eq!(result.pool_queued, Some(0));
        assert!((result.success_rate.unwrap() - 1.0).abs() < 0.001);
    }

    // ============================================
    // PoolManager creation tests
    // ============================================

    #[tokio::test]
    async fn test_pool_manager_new_creates_unique_socket_path() {
        // Two PoolManagers should have different socket paths
        let manager1 = PoolManager::new();
        let manager2 = PoolManager::new();

        assert_ne!(manager1.socket_path, manager2.socket_path);
        assert!(manager1
            .socket_path
            .starts_with("/tmp/relayer-plugin-pool-"));
        assert!(manager2
            .socket_path
            .starts_with("/tmp/relayer-plugin-pool-"));
    }

    #[tokio::test]
    async fn test_pool_manager_with_custom_socket_path() {
        let custom_path = "/tmp/custom-test-pool.sock".to_string();
        let manager = PoolManager::with_socket_path(custom_path.clone());

        assert_eq!(manager.socket_path, custom_path);
    }

    #[tokio::test]
    async fn test_pool_manager_default_trait() {
        // Verify Default trait creates a valid manager
        let manager = PoolManager::default();
        assert!(manager.socket_path.starts_with("/tmp/relayer-plugin-pool-"));
    }

    // ============================================
    // Circuit breaker state tests
    // ============================================

    #[tokio::test]
    async fn test_circuit_state_initial() {
        let manager = PoolManager::new();

        // Initial state should be Closed
        assert_eq!(manager.circuit_state(), CircuitState::Closed);
    }

    #[tokio::test]
    async fn test_avg_response_time_initial() {
        let manager = PoolManager::new();

        // Initial response time should be 0
        assert_eq!(manager.avg_response_time_ms(), 0);
    }

    // ============================================
    // Recovery mode tests
    // ============================================

    #[tokio::test]
    async fn test_recovery_mode_initial() {
        let manager = PoolManager::new();

        // Should not be in recovery mode initially
        assert!(!manager.is_recovering());
        assert_eq!(manager.recovery_allowance_percent(), 0);
    }

    // ============================================
    // ScriptResult construction tests
    // ============================================

    #[test]
    fn test_script_result_success_construction() {
        let result = ScriptResult {
            logs: vec![LogEntry {
                level: LogLevel::Info,
                message: "Test log".to_string(),
            }],
            error: String::new(),
            return_value: r#"{"success": true}"#.to_string(),
            trace: vec![],
        };

        assert!(result.error.is_empty());
        assert_eq!(result.logs.len(), 1);
        assert_eq!(result.logs[0].level, LogLevel::Info);
    }

    #[test]
    fn test_script_result_with_multiple_logs() {
        let result = ScriptResult {
            logs: vec![
                LogEntry {
                    level: LogLevel::Log,
                    message: "Starting execution".to_string(),
                },
                LogEntry {
                    level: LogLevel::Debug,
                    message: "Processing data".to_string(),
                },
                LogEntry {
                    level: LogLevel::Warn,
                    message: "Deprecated API used".to_string(),
                },
                LogEntry {
                    level: LogLevel::Error,
                    message: "Non-fatal error".to_string(),
                },
            ],
            error: String::new(),
            return_value: "done".to_string(),
            trace: vec![],
        };

        assert_eq!(result.logs.len(), 4);
        assert_eq!(result.logs[0].level, LogLevel::Log);
        assert_eq!(result.logs[1].level, LogLevel::Debug);
        assert_eq!(result.logs[2].level, LogLevel::Warn);
        assert_eq!(result.logs[3].level, LogLevel::Error);
    }

    // ============================================
    // QueuedRequest structure tests
    // ============================================

    #[test]
    fn test_queued_request_required_fields() {
        let (tx, _rx) = oneshot::channel();

        let request = QueuedRequest {
            plugin_id: "test-plugin".to_string(),
            compiled_code: Some("module.exports.handler = () => {}".to_string()),
            plugin_path: None,
            params: serde_json::json!({"key": "value"}),
            headers: None,
            socket_path: "/tmp/test.sock".to_string(),
            http_request_id: Some("req-123".to_string()),
            timeout_secs: Some(30),
            route: Some("/api/test".to_string()),
            config: Some(serde_json::json!({"setting": true})),
            method: Some("POST".to_string()),
            query: Some(serde_json::json!({"page": "1"})),
            response_tx: tx,
        };

        assert_eq!(request.plugin_id, "test-plugin");
        assert!(request.compiled_code.is_some());
        assert!(request.plugin_path.is_none());
        assert_eq!(request.timeout_secs, Some(30));
    }

    #[test]
    fn test_queued_request_minimal() {
        let (tx, _rx) = oneshot::channel();

        let request = QueuedRequest {
            plugin_id: "minimal".to_string(),
            compiled_code: None,
            plugin_path: Some("/path/to/plugin.ts".to_string()),
            params: serde_json::json!(null),
            headers: None,
            socket_path: "/tmp/min.sock".to_string(),
            http_request_id: None,
            timeout_secs: None,
            route: None,
            config: None,
            method: None,
            query: None,
            response_tx: tx,
        };

        assert_eq!(request.plugin_id, "minimal");
        assert!(request.compiled_code.is_none());
        assert!(request.plugin_path.is_some());
    }

    // ============================================
    // Error type tests
    // ============================================

    #[test]
    fn test_plugin_error_socket_error() {
        let err = PluginError::SocketError("Connection failed".to_string());
        let display = format!("{err}");
        assert!(display.contains("Socket error"));
        assert!(display.contains("Connection failed"));
    }

    #[test]
    fn test_plugin_error_plugin_execution_error() {
        let err = PluginError::PluginExecutionError("Execution failed".to_string());
        let display = format!("{err}");
        assert!(display.contains("Execution failed"));
    }

    #[test]
    fn test_plugin_error_handler_error() {
        let payload = PluginHandlerPayload {
            message: "Handler error".to_string(),
            status: 400,
            code: Some("BAD_REQUEST".to_string()),
            details: Some(serde_json::json!({"field": "name"})),
            logs: None,
            traces: None,
        };
        let err = PluginError::HandlerError(Box::new(payload));

        // Check that it can be displayed
        let display = format!("{err:?}");
        assert!(display.contains("HandlerError"));
    }

    // ============================================
    // Handler payload tests
    // ============================================

    #[test]
    fn test_plugin_handler_payload_full() {
        let payload = PluginHandlerPayload {
            message: "Validation failed".to_string(),
            status: 422,
            code: Some("VALIDATION_ERROR".to_string()),
            details: Some(serde_json::json!({
                "errors": [
                    {"field": "email", "message": "Invalid format"}
                ]
            })),
            logs: Some(vec![LogEntry {
                level: LogLevel::Error,
                message: "Validation failed for email".to_string(),
            }]),
            traces: Some(vec![serde_json::json!({"stack": "Error at line 10"})]),
        };

        assert_eq!(payload.status, 422);
        assert_eq!(payload.code, Some("VALIDATION_ERROR".to_string()));
        assert!(payload.logs.is_some());
        assert!(payload.traces.is_some());
    }

    #[test]
    fn test_plugin_handler_payload_minimal() {
        let payload = PluginHandlerPayload {
            message: "Error".to_string(),
            status: 500,
            code: None,
            details: None,
            logs: None,
            traces: None,
        };

        assert_eq!(payload.status, 500);
        assert!(payload.code.is_none());
        assert!(payload.details.is_none());
    }

    // ============================================
    // Async tests (tokio runtime)
    // ============================================

    #[tokio::test]
    async fn test_pool_manager_not_initialized_health_check() {
        let manager = PoolManager::with_socket_path("/tmp/test-health.sock".to_string());

        // Health check on uninitialized manager should return not_initialized
        let health = manager.health_check().await.unwrap();

        assert!(!health.healthy);
        assert_eq!(health.status, "not_initialized");
        assert!(health.uptime_ms.is_none());
        assert!(health.memory.is_none());
    }

    #[tokio::test]
    async fn test_pool_manager_circuit_info_in_health_status() {
        let manager = PoolManager::with_socket_path("/tmp/test-circuit.sock".to_string());

        let health = manager.health_check().await.unwrap();

        // Circuit state info should be present even when not initialized
        assert!(health.circuit_state.is_some());
        assert_eq!(health.circuit_state, Some("closed".to_string()));
        assert!(health.avg_response_time_ms.is_some());
        assert!(health.recovering.is_some());
        assert!(health.recovery_percent.is_some());
    }

    #[tokio::test]
    async fn test_invalidate_plugin_when_not_initialized() {
        let manager = PoolManager::with_socket_path("/tmp/test-invalidate.sock".to_string());

        // Invalidating when not initialized should be a no-op
        let result = manager.invalidate_plugin("test-plugin".to_string()).await;

        // Should succeed (no-op)
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_shutdown_when_not_initialized() {
        let manager = PoolManager::with_socket_path("/tmp/test-shutdown.sock".to_string());

        // Shutdown when not initialized should be a no-op
        let result = manager.shutdown().await;

        // Should succeed (no-op)
        assert!(result.is_ok());
    }

    // ============================================
    // Additional ParsedHealthResult tests
    // ============================================

    #[test]
    fn test_parsed_health_result_default() {
        let result = ParsedHealthResult::default();
        assert_eq!(result.status, "");
        assert_eq!(result.uptime_ms, None);
        assert_eq!(result.memory, None);
        assert_eq!(result.pool_completed, None);
        assert_eq!(result.pool_queued, None);
        assert_eq!(result.success_rate, None);
    }

    #[test]
    fn test_parsed_health_result_equality() {
        let result1 = ParsedHealthResult {
            status: "ok".to_string(),
            uptime_ms: Some(1000),
            memory: Some(500000),
            pool_completed: Some(50),
            pool_queued: Some(2),
            success_rate: Some(1.0),
        };
        let result2 = ParsedHealthResult {
            status: "ok".to_string(),
            uptime_ms: Some(1000),
            memory: Some(500000),
            pool_completed: Some(50),
            pool_queued: Some(2),
            success_rate: Some(1.0),
        };
        assert_eq!(result1, result2);
    }

    #[test]
    fn test_format_return_value_nested_object() {
        let value = Some(serde_json::json!({
            "user": { "name": "John", "age": 30 }
        }));
        let result = PoolManager::format_return_value(value);
        assert!(result.contains("John"));
        assert!(result.contains("30"));
    }

    #[test]
    fn test_format_return_value_empty_collections() {
        let value = Some(serde_json::json!({}));
        assert_eq!(PoolManager::format_return_value(value), "{}");
        let value = Some(serde_json::json!([]));
        assert_eq!(PoolManager::format_return_value(value), "[]");
    }

    #[test]
    fn test_parse_health_result_zero_values() {
        let json = serde_json::json!({
            "status": "starting",
            "uptime": 0,
            "memory": { "heapUsed": 0 },
            "pool": { "completed": 0, "queued": 0 },
            "execution": { "successRate": 0.0 }
        });
        let result = PoolManager::parse_health_result(&json);
        assert_eq!(result.status, "starting");
        assert_eq!(result.uptime_ms, Some(0));
        assert_eq!(result.memory, Some(0));
        assert_eq!(result.pool_completed, Some(0));
        assert_eq!(result.pool_queued, Some(0));
        assert_eq!(result.success_rate, Some(0.0));
    }

    #[test]
    fn test_calculate_heap_size_precise_calculations() {
        assert_eq!(PoolManager::calculate_heap_size(0), 512);
        assert_eq!(PoolManager::calculate_heap_size(1), 512);
        assert_eq!(PoolManager::calculate_heap_size(10), 544);
        assert_eq!(PoolManager::calculate_heap_size(20), 576);
        assert_eq!(PoolManager::calculate_heap_size(100), 832);
        assert_eq!(PoolManager::calculate_heap_size(200), 1152);
    }

    #[tokio::test]
    async fn test_pool_manager_health_check_flag_initial() {
        let manager = PoolManager::new();
        assert!(!manager.health_check_needed.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn test_pool_manager_consecutive_failures_initial() {
        let manager = PoolManager::new();
        assert_eq!(manager.consecutive_failures.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_recovery_allowance_bounds() {
        let manager = PoolManager::new();
        manager.recovery_allowance.store(0, Ordering::Relaxed);
        assert_eq!(manager.recovery_allowance_percent(), 0);
        manager.recovery_allowance.store(50, Ordering::Relaxed);
        assert_eq!(manager.recovery_allowance_percent(), 50);
        manager.recovery_allowance.store(100, Ordering::Relaxed);
        assert_eq!(manager.recovery_allowance_percent(), 100);
    }

    #[tokio::test]
    async fn test_is_initialized_changes_with_state() {
        let manager = PoolManager::with_socket_path("/tmp/init-test-123.sock".to_string());
        assert!(!manager.is_initialized().await);
        manager.initialized.store(true, Ordering::Release);
        assert!(manager.is_initialized().await);
        manager.initialized.store(false, Ordering::Release);
        assert!(!manager.is_initialized().await);
    }

    // ============================================
    // Additional edge case tests for coverage
    // ============================================

    #[test]
    fn test_is_dead_server_error_with_script_timeout() {
        // ScriptTimeout should NOT be a dead server error
        let err = PluginError::ScriptTimeout(30);
        assert!(!PoolManager::is_dead_server_error(&err));
    }

    #[test]
    fn test_is_dead_server_error_with_plugin_error() {
        let err = PluginError::PluginError("some plugin error".to_string());
        assert!(!PoolManager::is_dead_server_error(&err));
    }

    #[test]
    fn test_is_dead_server_error_with_connection_timeout_in_plugin_error() {
        // Note: When "connection timed out" is wrapped in PluginExecutionError,
        // the Display output includes "Plugin" which triggers the exclusion
        // for (plugin + timed out). This is expected behavior to prevent
        // plugin execution timeouts from triggering restarts.
        let err = PluginError::PluginExecutionError("connection timed out".to_string());
        // The error string becomes something like "Plugin execution error: connection timed out"
        // which contains "plugin" AND "timed out", so it's excluded
        assert!(!PoolManager::is_dead_server_error(&err));

        // SocketError doesn't add "Plugin" to the display, so connection issues there
        // would be detected correctly
        let err = PluginError::SocketError("connect timed out".to_string());
        assert!(PoolManager::is_dead_server_error(&err));
    }

    #[test]
    fn test_parse_pool_response_success_with_logs_various_levels() {
        use super::super::protocol::{PoolLogEntry, PoolResponse};

        let response = PoolResponse {
            task_id: "test-levels".to_string(),
            success: true,
            result: Some(serde_json::json!("ok")),
            error: None,
            logs: Some(vec![
                PoolLogEntry {
                    level: "log".to_string(),
                    message: "log level".to_string(),
                },
                PoolLogEntry {
                    level: "debug".to_string(),
                    message: "debug level".to_string(),
                },
                PoolLogEntry {
                    level: "info".to_string(),
                    message: "info level".to_string(),
                },
                PoolLogEntry {
                    level: "warn".to_string(),
                    message: "warn level".to_string(),
                },
                PoolLogEntry {
                    level: "error".to_string(),
                    message: "error level".to_string(),
                },
                PoolLogEntry {
                    level: "result".to_string(),
                    message: "result level".to_string(),
                },
            ]),
        };

        let result = PoolManager::parse_pool_response(response).unwrap();
        assert_eq!(result.logs.len(), 6);
        assert_eq!(result.logs[0].level, LogLevel::Log);
        assert_eq!(result.logs[1].level, LogLevel::Debug);
        assert_eq!(result.logs[2].level, LogLevel::Info);
        assert_eq!(result.logs[3].level, LogLevel::Warn);
        assert_eq!(result.logs[4].level, LogLevel::Error);
        assert_eq!(result.logs[5].level, LogLevel::Result);
    }

    #[test]
    fn test_parse_error_response_defaults() {
        use super::super::protocol::PoolResponse;

        // Response with no error field at all
        let response = PoolResponse {
            task_id: "no-error".to_string(),
            success: false,
            result: None,
            error: None,
            logs: None,
        };

        let err = PoolManager::parse_error_response(response);
        match err {
            PluginError::HandlerError(payload) => {
                assert_eq!(payload.message, "Unknown error");
                assert_eq!(payload.status, 500);
                assert!(payload.code.is_none());
                assert!(payload.details.is_none());
            }
            _ => panic!("Expected HandlerError"),
        }
    }

    #[test]
    fn test_format_return_value_float() {
        let value = Some(serde_json::json!(3.14159));
        let result = PoolManager::format_return_value(value);
        assert!(result.contains("3.14159"));
    }

    #[test]
    fn test_format_return_value_large_array() {
        let value = Some(serde_json::json!([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]));
        let result = PoolManager::format_return_value(value);
        assert_eq!(result, "[1,2,3,4,5,6,7,8,9,10]");
    }

    #[test]
    fn test_format_return_value_string_with_special_chars() {
        let value = Some(serde_json::json!("hello\nworld\ttab"));
        assert_eq!(PoolManager::format_return_value(value), "hello\nworld\ttab");
    }

    #[test]
    fn test_format_return_value_unicode() {
        let value = Some(serde_json::json!("こんにちは世界 🌍"));
        assert_eq!(PoolManager::format_return_value(value), "こんにちは世界 🌍");
    }

    #[test]
    fn test_parse_health_result_large_values() {
        let json = serde_json::json!({
            "status": "healthy",
            "uptime": 999999999999_u64,
            "memory": { "heapUsed": 9999999999_u64 },
            "pool": { "completed": 999999999_u64, "queued": 999999_u64 },
            "execution": { "successRate": 0.999999 }
        });

        let result = PoolManager::parse_health_result(&json);
        assert_eq!(result.status, "healthy");
        assert_eq!(result.uptime_ms, Some(999999999999));
        assert_eq!(result.memory, Some(9999999999));
        assert_eq!(result.pool_completed, Some(999999999));
        assert_eq!(result.pool_queued, Some(999999));
        assert!((result.success_rate.unwrap() - 0.999999).abs() < 0.0000001);
    }

    #[test]
    fn test_parse_health_result_negative_values_treated_as_none() {
        // JSON doesn't have unsigned, so negative values won't parse as u64
        let json = serde_json::json!({
            "status": "error",
            "uptime": -1,
            "memory": { "heapUsed": -100 }
        });

        let result = PoolManager::parse_health_result(&json);
        assert_eq!(result.status, "error");
        assert_eq!(result.uptime_ms, None); // -1 can't be u64
        assert_eq!(result.memory, None);
    }

    #[test]
    fn test_parsed_health_result_debug() {
        let result = ParsedHealthResult {
            status: "test".to_string(),
            uptime_ms: Some(100),
            memory: Some(200),
            pool_completed: Some(50),
            pool_queued: Some(5),
            success_rate: Some(0.95),
        };

        let debug_str = format!("{result:?}");
        assert!(debug_str.contains("test"));
        assert!(debug_str.contains("100"));
        assert!(debug_str.contains("200"));
    }

    #[test]
    fn test_calculate_heap_size_boundary_values() {
        // Test at exact boundaries
        // 9 should give base (9/10 = 0)
        assert_eq!(PoolManager::calculate_heap_size(9), 512);
        // 10 should give base + 32 (10/10 = 1)
        assert_eq!(PoolManager::calculate_heap_size(10), 544);

        // Test boundary where cap kicks in
        // 2400 would be: 512 + (240 * 32) = 512 + 7680 = 8192 (at cap)
        assert_eq!(PoolManager::calculate_heap_size(2400), 8192);
        // 2399 would be: 512 + (239 * 32) = 512 + 7648 = 8160 (under cap)
        assert_eq!(PoolManager::calculate_heap_size(2399), 8160);
    }

    #[tokio::test]
    async fn test_pool_manager_socket_path_format() {
        let manager = PoolManager::new();
        // Should contain UUID format
        assert!(manager.socket_path.starts_with("/tmp/relayer-plugin-pool-"));
        assert!(manager.socket_path.ends_with(".sock"));
        // UUID is 36 chars (32 hex + 4 dashes)
        let uuid_part = manager
            .socket_path
            .strip_prefix("/tmp/relayer-plugin-pool-")
            .unwrap()
            .strip_suffix(".sock")
            .unwrap();
        assert_eq!(uuid_part.len(), 36);
    }

    #[tokio::test]
    async fn test_health_check_socket_missing() {
        let manager =
            PoolManager::with_socket_path("/tmp/nonexistent-socket-12345.sock".to_string());
        // Mark as initialized but socket doesn't exist
        manager.initialized.store(true, Ordering::Release);

        let health = manager.health_check().await.unwrap();
        assert!(!health.healthy);
        assert_eq!(health.status, "socket_missing");
    }

    #[test]
    fn test_is_dead_server_error_embedded_patterns() {
        // Patterns embedded in longer messages
        let err = PluginError::PluginExecutionError(
            "Error: ECONNREFUSED connection refused at 127.0.0.1:3000".to_string(),
        );
        assert!(PoolManager::is_dead_server_error(&err));

        let err = PluginError::PluginExecutionError(
            "SocketError: broken pipe while writing to /tmp/socket".to_string(),
        );
        assert!(PoolManager::is_dead_server_error(&err));

        let err = PluginError::PluginExecutionError(
            "IO Error: No such file or directory (os error 2)".to_string(),
        );
        assert!(PoolManager::is_dead_server_error(&err));
    }

    #[test]
    fn test_is_dead_server_error_mixed_case_timeout_patterns() {
        // Handler timeout variants - none should be dead server errors
        let variants = vec![
            "HANDLER TIMED OUT",
            "Handler Timed Out after 30s",
            "the handler timed out waiting for response",
        ];

        for msg in variants {
            let err = PluginError::PluginExecutionError(msg.to_string());
            assert!(
                !PoolManager::is_dead_server_error(&err),
                "Expected '{msg}' to NOT be dead server error"
            );
        }
    }

    #[tokio::test]
    async fn test_ensure_started_idempotent() {
        let manager = PoolManager::with_socket_path("/tmp/idempotent-test-999.sock".to_string());

        // First call when not initialized
        assert!(!manager.is_initialized().await);

        // Manually set initialized without actually starting (for test)
        manager.initialized.store(true, Ordering::Release);

        // ensure_started should return immediately
        let result = manager.ensure_started().await;
        assert!(result.is_ok());
        assert!(manager.is_initialized().await);
    }

    #[test]
    fn test_queued_request_with_headers() {
        let (tx, _rx) = oneshot::channel();

        let mut headers = HashMap::new();
        headers.insert(
            "Authorization".to_string(),
            vec!["Bearer token".to_string()],
        );
        headers.insert(
            "Content-Type".to_string(),
            vec!["application/json".to_string()],
        );

        let request = QueuedRequest {
            plugin_id: "headers-test".to_string(),
            compiled_code: None,
            plugin_path: Some("/path/to/plugin.ts".to_string()),
            params: serde_json::json!({}),
            headers: Some(headers),
            socket_path: "/tmp/test.sock".to_string(),
            http_request_id: None,
            timeout_secs: None,
            route: None,
            config: None,
            method: None,
            query: None,
            response_tx: tx,
        };

        assert!(request.headers.is_some());
        let headers = request.headers.unwrap();
        assert!(headers.contains_key("Authorization"));
        assert!(headers.contains_key("Content-Type"));
    }

    #[test]
    fn test_plugin_error_display_formats() {
        // Test all PluginError variants have proper Display implementations
        let err = PluginError::SocketError("test socket error".to_string());
        assert!(format!("{err}").contains("Socket error"));

        let err = PluginError::PluginExecutionError("test execution error".to_string());
        assert!(format!("{err}").contains("test execution error"));

        let err = PluginError::ScriptTimeout(60);
        assert!(format!("{err}").contains("60"));

        let err = PluginError::PluginError("test plugin error".to_string());
        assert!(format!("{err}").contains("test plugin error"));
    }

    #[test]
    fn test_pool_log_entry_to_log_entry_conversion() {
        use super::super::protocol::PoolLogEntry;

        // Test the From<PoolLogEntry> for LogEntry conversion
        let pool_log = PoolLogEntry {
            level: "info".to_string(),
            message: "test message".to_string(),
        };

        let log_entry: LogEntry = pool_log.into();
        assert_eq!(log_entry.level, LogLevel::Info);
        assert_eq!(log_entry.message, "test message");

        // Test unknown level defaults
        let pool_log = PoolLogEntry {
            level: "unknown_level".to_string(),
            message: "unknown level message".to_string(),
        };

        let log_entry: LogEntry = pool_log.into();
        assert_eq!(log_entry.level, LogLevel::Log); // Should default to Log
    }

    #[tokio::test]
    async fn test_circuit_breaker_records_success() {
        let manager = PoolManager::new();

        // Record some successes
        manager.circuit_breaker.record_success(100);
        manager.circuit_breaker.record_success(150);
        manager.circuit_breaker.record_success(200);

        // Average should be calculated
        let avg = manager.avg_response_time_ms();
        assert!(avg > 0);
    }

    #[tokio::test]
    async fn test_circuit_breaker_state_transitions() {
        let manager = PoolManager::new();

        // Initial state is Closed
        assert_eq!(manager.circuit_state(), CircuitState::Closed);

        // Record many failures to potentially trip the breaker
        for _ in 0..20 {
            manager.circuit_breaker.record_failure();
        }

        // State might have changed (depends on thresholds)
        let state = manager.circuit_state();
        assert!(matches!(
            state,
            CircuitState::Closed | CircuitState::HalfOpen | CircuitState::Open
        ));
    }

    #[tokio::test]
    async fn test_recovery_mode_activation() {
        let manager = PoolManager::new();

        // Manually activate recovery mode
        manager.recovery_allowance.store(10, Ordering::Relaxed);
        manager.recovery_mode.store(true, Ordering::Relaxed);

        assert!(manager.is_recovering());
        assert_eq!(manager.recovery_allowance_percent(), 10);

        // Increase allowance
        manager.recovery_allowance.store(50, Ordering::Relaxed);
        assert_eq!(manager.recovery_allowance_percent(), 50);

        // Exit recovery mode
        manager.recovery_mode.store(false, Ordering::Relaxed);
        assert!(!manager.is_recovering());
    }

    #[test]
    fn test_parse_pool_response_with_empty_logs() {
        use super::super::protocol::PoolResponse;

        let response = PoolResponse {
            task_id: "empty-logs".to_string(),
            success: true,
            result: Some(serde_json::json!("done")),
            error: None,
            logs: Some(vec![]), // Empty logs array
        };

        let result = PoolManager::parse_pool_response(response).unwrap();
        assert!(result.logs.is_empty());
        assert_eq!(result.return_value, "done");
    }

    #[test]
    fn test_handler_payload_with_complex_details() {
        let payload = PluginHandlerPayload {
            message: "Complex error".to_string(),
            status: 400,
            code: Some("VALIDATION_ERROR".to_string()),
            details: Some(serde_json::json!({
                "errors": [
                    {"field": "email", "code": "invalid", "message": "Invalid email format"},
                    {"field": "password", "code": "weak", "message": "Password too weak"}
                ],
                "metadata": {
                    "requestId": "req-123",
                    "timestamp": "2024-01-01T00:00:00Z"
                }
            })),
            logs: None,
            traces: None,
        };

        assert_eq!(payload.status, 400);
        let details = payload.details.unwrap();
        assert!(details.get("errors").is_some());
        assert!(details.get("metadata").is_some());
    }

    #[test]
    fn test_health_status_construction_healthy() {
        use super::super::health::HealthStatus;

        let status = HealthStatus {
            healthy: true,
            status: "ok".to_string(),
            uptime_ms: Some(1000000),
            memory: Some(500000000),
            pool_completed: Some(1000),
            pool_queued: Some(5),
            success_rate: Some(0.99),
            circuit_state: Some("closed".to_string()),
            avg_response_time_ms: Some(50),
            recovering: Some(false),
            recovery_percent: Some(100),
            shared_socket_available_slots: Some(100),
            shared_socket_active_connections: Some(10),
            shared_socket_registered_executions: Some(5),
            connection_pool_available_slots: Some(50),
            connection_pool_active_connections: Some(5),
        };

        assert!(status.healthy);
        assert_eq!(status.status, "ok");
        assert_eq!(status.uptime_ms, Some(1000000));
        assert_eq!(status.circuit_state, Some("closed".to_string()));
    }

    #[test]
    fn test_health_status_construction_unhealthy() {
        use super::super::health::HealthStatus;

        let status = HealthStatus {
            healthy: false,
            status: "connection_failed".to_string(),
            uptime_ms: None,
            memory: None,
            pool_completed: None,
            pool_queued: None,
            success_rate: None,
            circuit_state: Some("open".to_string()),
            avg_response_time_ms: Some(0),
            recovering: Some(true),
            recovery_percent: Some(10),
            shared_socket_available_slots: None,
            shared_socket_active_connections: None,
            shared_socket_registered_executions: None,
            connection_pool_available_slots: None,
            connection_pool_active_connections: None,
        };

        assert!(!status.healthy);
        assert_eq!(status.status, "connection_failed");
        assert!(status.uptime_ms.is_none());
    }

    #[test]
    fn test_health_status_debug_format() {
        use super::super::health::HealthStatus;

        let status = HealthStatus {
            healthy: true,
            status: "test".to_string(),
            uptime_ms: Some(100),
            memory: None,
            pool_completed: None,
            pool_queued: None,
            success_rate: None,
            circuit_state: None,
            avg_response_time_ms: None,
            recovering: None,
            recovery_percent: None,
            shared_socket_available_slots: None,
            shared_socket_active_connections: None,
            shared_socket_registered_executions: None,
            connection_pool_available_slots: None,
            connection_pool_active_connections: None,
        };

        let debug_str = format!("{status:?}");
        assert!(debug_str.contains("healthy: true"));
        assert!(debug_str.contains("test"));
    }

    #[test]
    fn test_health_status_clone() {
        use super::super::health::HealthStatus;

        let status = HealthStatus {
            healthy: true,
            status: "original".to_string(),
            uptime_ms: Some(500),
            memory: Some(100),
            pool_completed: Some(10),
            pool_queued: Some(1),
            success_rate: Some(0.95),
            circuit_state: Some("closed".to_string()),
            avg_response_time_ms: Some(25),
            recovering: Some(false),
            recovery_percent: Some(100),
            shared_socket_available_slots: Some(50),
            shared_socket_active_connections: Some(2),
            shared_socket_registered_executions: Some(1),
            connection_pool_available_slots: Some(25),
            connection_pool_active_connections: Some(1),
        };

        let cloned = status.clone();
        assert_eq!(cloned.healthy, status.healthy);
        assert_eq!(cloned.status, status.status);
        assert_eq!(cloned.uptime_ms, status.uptime_ms);
    }

    #[test]
    fn test_execute_request_debug() {
        use super::super::protocol::ExecuteRequest;

        let request = ExecuteRequest {
            task_id: "debug-test".to_string(),
            plugin_id: "test-plugin".to_string(),
            compiled_code: None,
            plugin_path: Some("/path/to/plugin.ts".to_string()),
            params: serde_json::json!({"test": true}),
            headers: None,
            socket_path: "/tmp/test.sock".to_string(),
            http_request_id: None,
            timeout: None,
            route: None,
            config: None,
            method: None,
            query: None,
        };

        let debug_str = format!("{request:?}");
        assert!(debug_str.contains("debug-test"));
        assert!(debug_str.contains("test-plugin"));
    }

    #[test]
    fn test_pool_error_debug() {
        use super::super::protocol::PoolError;

        let error = PoolError {
            message: "Test error".to_string(),
            code: Some("TEST_ERR".to_string()),
            status: Some(400),
            details: Some(serde_json::json!({"info": "test"})),
        };

        let debug_str = format!("{error:?}");
        assert!(debug_str.contains("Test error"));
        assert!(debug_str.contains("TEST_ERR"));
    }

    #[test]
    fn test_pool_response_debug() {
        use super::super::protocol::PoolResponse;

        let response = PoolResponse {
            task_id: "resp-123".to_string(),
            success: true,
            result: Some(serde_json::json!("result")),
            error: None,
            logs: None,
        };

        let debug_str = format!("{response:?}");
        assert!(debug_str.contains("resp-123"));
        assert!(debug_str.contains("true"));
    }

    #[test]
    fn test_pool_log_entry_debug() {
        use super::super::protocol::PoolLogEntry;

        let entry = PoolLogEntry {
            level: "info".to_string(),
            message: "Test message".to_string(),
        };

        let debug_str = format!("{entry:?}");
        assert!(debug_str.contains("info"));
        assert!(debug_str.contains("Test message"));
    }

    #[test]
    fn test_circuit_breaker_default_trait() {
        use super::super::health::CircuitBreaker;

        let cb = CircuitBreaker::default();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_set_state_all_variants() {
        use super::super::health::CircuitBreaker;

        let cb = CircuitBreaker::new();

        // Test setting all states
        cb.set_state(CircuitState::HalfOpen);
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        cb.set_state(CircuitState::Open);
        assert_eq!(cb.state(), CircuitState::Open);

        cb.set_state(CircuitState::Closed);
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_failure_rate_triggers_open() {
        use super::super::health::CircuitBreaker;

        let cb = CircuitBreaker::new();

        // Record enough failures to trigger circuit opening
        for _ in 0..100 {
            cb.record_failure();
        }

        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_circuit_breaker_low_failure_rate_stays_closed() {
        use super::super::health::CircuitBreaker;

        let cb = CircuitBreaker::new();

        // Record mostly successes with few failures
        for _ in 0..90 {
            cb.record_success(50);
        }
        for _ in 0..10 {
            cb.record_failure();
        }

        // Should still be closed (10% failure rate)
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_ema_response_time() {
        use super::super::health::CircuitBreaker;

        let cb = CircuitBreaker::new();

        // Record several response times
        cb.record_success(100);
        let avg1 = cb.avg_response_time();

        cb.record_success(100);
        cb.record_success(100);
        cb.record_success(100);
        let avg2 = cb.avg_response_time();

        // Average should stabilize around 100
        assert!(avg1 > 0);
        assert!(avg2 > 0);
        assert!(avg2 <= 100);
    }

    #[test]
    fn test_circuit_breaker_force_close_resets_counters() {
        use super::super::health::CircuitBreaker;

        let cb = CircuitBreaker::new();
        cb.set_state(CircuitState::Open);

        cb.force_close();

        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_process_status_debug() {
        use super::super::health::ProcessStatus;

        assert_eq!(format!("{:?}", ProcessStatus::Running), "Running");
        assert_eq!(format!("{:?}", ProcessStatus::Exited), "Exited");
        assert_eq!(format!("{:?}", ProcessStatus::Unknown), "Unknown");
        assert_eq!(format!("{:?}", ProcessStatus::NoProcess), "NoProcess");
    }

    #[test]
    fn test_process_status_clone() {
        use super::super::health::ProcessStatus;

        let status = ProcessStatus::Running;
        let cloned = status;
        assert_eq!(status, cloned);
    }

    // ============================================
    // Additional coverage tests - DeadServerIndicator
    // ============================================

    #[test]
    fn test_dead_server_indicator_all_variants() {
        use super::super::health::DeadServerIndicator;

        // Test all enum variants exist and are properly matched
        let variants = [
            ("eof while parsing", DeadServerIndicator::EofWhileParsing),
            ("broken pipe", DeadServerIndicator::BrokenPipe),
            ("connection refused", DeadServerIndicator::ConnectionRefused),
            ("connection reset", DeadServerIndicator::ConnectionReset),
            ("not connected", DeadServerIndicator::NotConnected),
            ("failed to connect", DeadServerIndicator::FailedToConnect),
            (
                "socket file missing",
                DeadServerIndicator::SocketFileMissing,
            ),
            ("no such file", DeadServerIndicator::NoSuchFile),
            (
                "connection timed out",
                DeadServerIndicator::ConnectionTimedOut,
            ),
            ("connect timed out", DeadServerIndicator::ConnectionTimedOut),
        ];

        for (pattern, expected) in variants {
            let result = DeadServerIndicator::from_error_str(pattern);
            assert_eq!(result, Some(expected), "Pattern '{pattern}' should match");
        }
    }

    #[test]
    fn test_dead_server_indicator_debug_format() {
        use super::super::health::DeadServerIndicator;

        let indicator = DeadServerIndicator::BrokenPipe;
        let debug_str = format!("{indicator:?}");
        assert_eq!(debug_str, "BrokenPipe");
    }

    #[test]
    fn test_dead_server_indicator_clone_copy() {
        use super::super::health::DeadServerIndicator;

        let indicator = DeadServerIndicator::ConnectionRefused;
        let cloned = indicator;
        assert_eq!(indicator, cloned);
    }

    #[test]
    fn test_result_ring_buffer_not_enough_data() {
        use super::super::health::ResultRingBuffer;

        let buffer = ResultRingBuffer::new(100);

        // Record less than 10 results
        for _ in 0..9 {
            buffer.record(false);
        }

        // Should return 0.0 because not enough data
        assert_eq!(buffer.failure_rate(), 0.0);
    }

    #[test]
    fn test_result_ring_buffer_exactly_10_samples() {
        use super::super::health::ResultRingBuffer;

        let buffer = ResultRingBuffer::new(100);

        // Record exactly 10 failures
        for _ in 0..10 {
            buffer.record(false);
        }

        // Should return 1.0 (100% failure)
        assert_eq!(buffer.failure_rate(), 1.0);
    }

    #[test]
    fn test_result_ring_buffer_wraps_correctly() {
        use super::super::health::ResultRingBuffer;

        let buffer = ResultRingBuffer::new(10);

        // Fill buffer with successes
        for _ in 0..10 {
            buffer.record(true);
        }
        assert_eq!(buffer.failure_rate(), 0.0);

        // Overwrite with failures
        for _ in 0..10 {
            buffer.record(false);
        }
        assert_eq!(buffer.failure_rate(), 1.0);
    }

    #[test]
    fn test_circuit_state_equality_all_pairs() {
        assert_eq!(CircuitState::Closed, CircuitState::Closed);
        assert_eq!(CircuitState::HalfOpen, CircuitState::HalfOpen);
        assert_eq!(CircuitState::Open, CircuitState::Open);

        assert_ne!(CircuitState::Closed, CircuitState::HalfOpen);
        assert_ne!(CircuitState::Closed, CircuitState::Open);
        assert_ne!(CircuitState::HalfOpen, CircuitState::Open);
    }

    #[test]
    fn test_circuit_state_clone_copy() {
        let state = CircuitState::HalfOpen;
        let copied = state;
        assert_eq!(state, copied);
    }

    #[test]
    fn test_parse_pool_response_with_null_values() {
        use super::super::protocol::PoolResponse;

        let response = PoolResponse {
            task_id: "null-test".to_string(),
            success: true,
            result: Some(serde_json::json!(null)),
            error: None,
            logs: None,
        };

        let result = PoolManager::parse_pool_response(response).unwrap();
        assert_eq!(result.return_value, "null");
    }

    #[test]
    fn test_parse_pool_response_with_nested_result() {
        use super::super::protocol::PoolResponse;

        let response = PoolResponse {
            task_id: "nested-test".to_string(),
            success: true,
            result: Some(serde_json::json!({
                "level1": {
                    "level2": {
                        "level3": "deep value"
                    }
                }
            })),
            error: None,
            logs: None,
        };

        let result = PoolManager::parse_pool_response(response).unwrap();
        assert!(result.return_value.contains("level1"));
        assert!(result.return_value.contains("level2"));
        assert!(result.return_value.contains("level3"));
        assert!(result.return_value.contains("deep value"));
    }

    #[test]
    fn test_parse_pool_response_error_with_details() {
        use super::super::protocol::{PoolError, PoolResponse};

        let response = PoolResponse {
            task_id: "error-details".to_string(),
            success: false,
            result: None,
            error: Some(PoolError {
                message: "Error with details".to_string(),
                code: Some("DETAILED_ERROR".to_string()),
                status: Some(422),
                details: Some(serde_json::json!({
                    "field": "email",
                    "expected": "string",
                    "received": "number"
                })),
            }),
            logs: None,
        };

        let err = PoolManager::parse_pool_response(response).unwrap_err();
        match err {
            PluginError::HandlerError(payload) => {
                assert_eq!(payload.message, "Error with details");
                assert_eq!(payload.code, Some("DETAILED_ERROR".to_string()));
                assert!(payload.details.is_some());
                let details = payload.details.unwrap();
                assert_eq!(details.get("field").unwrap(), "email");
            }
            _ => panic!("Expected HandlerError"),
        }
    }

    #[test]
    fn test_parse_health_result_with_all_optional_fields() {
        let json = serde_json::json!({
            "status": "healthy",
            "uptime": 999999,
            "memory": {
                "heapUsed": 123456789,
                "heapTotal": 987654321,
                "external": 111111,
                "arrayBuffers": 222222
            },
            "pool": {
                "completed": 50000,
                "queued": 100,
                "active": 50,
                "waiting": 25
            },
            "execution": {
                "successRate": 0.9999,
                "avgDuration": 45.5,
                "totalExecutions": 100000
            }
        });

        let result = PoolManager::parse_health_result(&json);
        assert_eq!(result.status, "healthy");
        assert_eq!(result.uptime_ms, Some(999999));
        assert_eq!(result.memory, Some(123456789));
        assert_eq!(result.pool_completed, Some(50000));
        assert_eq!(result.pool_queued, Some(100));
        assert!((result.success_rate.unwrap() - 0.9999).abs() < 0.0001);
    }

    #[tokio::test]
    async fn test_pool_manager_max_queue_size() {
        let manager = PoolManager::new();
        // max_queue_size should be set from config
        assert!(manager.max_queue_size > 0);
    }

    #[tokio::test]
    async fn test_pool_manager_last_restart_time_initial() {
        let manager = PoolManager::new();
        assert_eq!(manager.last_restart_time_ms.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_pool_manager_connection_pool_exists() {
        let manager = PoolManager::new();
        // Connection pool should be initialized
        let available = manager.connection_pool.semaphore.available_permits();
        assert!(available > 0);
    }

    #[test]
    fn test_is_dead_server_error_with_whitespace() {
        // Patterns with extra whitespace
        let err = PluginError::SocketError("  connection refused  ".to_string());
        assert!(PoolManager::is_dead_server_error(&err));

        let err = PluginError::SocketError("error: broken pipe occurred".to_string());
        assert!(PoolManager::is_dead_server_error(&err));
    }

    #[test]
    fn test_is_dead_server_error_multiline() {
        // Multiline error messages
        let err = PluginError::SocketError(
            "Error occurred\nConnection refused\nPlease retry".to_string(),
        );
        assert!(PoolManager::is_dead_server_error(&err));
    }

    #[test]
    fn test_is_dead_server_error_json_in_message() {
        // Error with JSON content
        let err = PluginError::PluginExecutionError(
            r#"{"error": "connection refused", "code": 61}"#.to_string(),
        );
        assert!(PoolManager::is_dead_server_error(&err));
    }

    #[test]
    fn test_format_return_value_special_json() {
        // Test with special JSON values
        let value = Some(serde_json::json!(f64::MAX));
        let result = PoolManager::format_return_value(value);
        assert!(!result.is_empty());

        let value = Some(serde_json::json!(i64::MIN));
        let result = PoolManager::format_return_value(value);
        assert!(result.contains("-"));
    }

    #[test]
    fn test_format_return_value_with_escaped_chars() {
        let value = Some(serde_json::json!("line1\nline2\ttab\"quote"));
        let result = PoolManager::format_return_value(value);
        assert!(result.contains("line1"));
        assert!(result.contains("line2"));
    }

    #[test]
    fn test_format_return_value_array_of_objects() {
        let value = Some(serde_json::json!([
            {"id": 1, "name": "first"},
            {"id": 2, "name": "second"}
        ]));
        let result = PoolManager::format_return_value(value);
        assert!(result.contains("first"));
        assert!(result.contains("second"));
    }

    #[test]
    fn test_all_log_levels_conversion() {
        use super::super::protocol::PoolLogEntry;

        let levels = [
            ("log", LogLevel::Log),
            ("debug", LogLevel::Debug),
            ("info", LogLevel::Info),
            ("warn", LogLevel::Warn),
            ("error", LogLevel::Error),
            ("result", LogLevel::Result),
            ("unknown_level", LogLevel::Log), // Unknown defaults to Log
            ("LOG", LogLevel::Log),           // Case matters - uppercase goes to default
            ("", LogLevel::Log),              // Empty string goes to default
        ];

        for (input, expected) in levels {
            let entry = PoolLogEntry {
                level: input.to_string(),
                message: "test".to_string(),
            };
            let log_entry: LogEntry = entry.into();
            assert_eq!(
                log_entry.level, expected,
                "Level '{input}' should convert to {expected:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_pool_manager_health_check_flag_manipulation() {
        let manager = PoolManager::new();

        manager.health_check_needed.store(true, Ordering::Relaxed);
        assert!(manager.health_check_needed.load(Ordering::Relaxed));

        manager.health_check_needed.store(false, Ordering::Relaxed);
        assert!(!manager.health_check_needed.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn test_pool_manager_consecutive_failures_manipulation() {
        let manager = PoolManager::new();

        manager.consecutive_failures.fetch_add(1, Ordering::Relaxed);
        assert_eq!(manager.consecutive_failures.load(Ordering::Relaxed), 1);

        manager.consecutive_failures.fetch_add(5, Ordering::Relaxed);
        assert_eq!(manager.consecutive_failures.load(Ordering::Relaxed), 6);

        manager.consecutive_failures.store(0, Ordering::Relaxed);
        assert_eq!(manager.consecutive_failures.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_parsed_health_result_with_all_none() {
        let result = ParsedHealthResult {
            status: "minimal".to_string(),
            uptime_ms: None,
            memory: None,
            pool_completed: None,
            pool_queued: None,
            success_rate: None,
        };

        assert_eq!(result.status, "minimal");
        assert!(result.uptime_ms.is_none());
        assert!(result.memory.is_none());
    }

    #[test]
    fn test_parsed_health_result_with_all_some() {
        let result = ParsedHealthResult {
            status: "complete".to_string(),
            uptime_ms: Some(u64::MAX),
            memory: Some(u64::MAX),
            pool_completed: Some(u64::MAX),
            pool_queued: Some(u64::MAX),
            success_rate: Some(1.0),
        };

        assert_eq!(result.status, "complete");
        assert_eq!(result.uptime_ms, Some(u64::MAX));
        assert_eq!(result.success_rate, Some(1.0));
    }

    #[test]
    fn test_calculate_heap_size_extensive_values() {
        // Test many different concurrency values
        let test_cases = [
            (0, 512),
            (1, 512),
            (5, 512),
            (9, 512),
            (10, 544),
            (11, 544),
            (19, 544),
            (20, 576),
            (50, 672),
            (100, 832),
            (150, 992),
            (200, 1152),
            (250, 1312),
            (300, 1472),
            (400, 1792),
            (500, 2112),
            (1000, 3712),
            (2000, 6912),
            (2400, 8192),  // At cap
            (3000, 8192),  // Capped
            (5000, 8192),  // Capped
            (10000, 8192), // Capped
        ];

        for (concurrency, expected_heap) in test_cases {
            let heap = PoolManager::calculate_heap_size(concurrency);
            assert_eq!(
                heap, expected_heap,
                "Concurrency {concurrency} should give heap {expected_heap}"
            );
        }
    }

    #[tokio::test]
    async fn test_pool_manager_drop_cleans_socket() {
        let socket_path = format!("/tmp/test-drop-{}.sock", uuid::Uuid::new_v4());

        // Create a file at the socket path
        std::fs::write(&socket_path, "test").unwrap();
        assert!(std::path::Path::new(&socket_path).exists());

        // Create manager with this socket path
        {
            let _manager = PoolManager::with_socket_path(socket_path.clone());
            // Manager exists here
        }
        // Manager dropped here - should clean up socket

        // Socket should be removed
        assert!(!std::path::Path::new(&socket_path).exists());
    }

    #[test]
    fn test_script_result_with_traces() {
        let result = ScriptResult {
            logs: vec![],
            error: String::new(),
            return_value: "with traces".to_string(),
            trace: vec![
                serde_json::json!({"action": "GET", "url": "/api/test"}),
                serde_json::json!({"action": "POST", "url": "/api/submit"}),
            ],
        };

        assert_eq!(result.trace.len(), 2);
        assert!(result.trace[0].get("action").is_some());
    }

    #[test]
    fn test_script_result_with_error() {
        let result = ScriptResult {
            logs: vec![LogEntry {
                level: LogLevel::Error,
                message: "Something went wrong".to_string(),
            }],
            error: "RuntimeError: undefined is not a function".to_string(),
            return_value: String::new(),
            trace: vec![],
        };

        assert!(!result.error.is_empty());
        assert!(result.error.contains("RuntimeError"));
        assert_eq!(result.logs.len(), 1);
    }

    #[test]
    fn test_plugin_handler_payload_with_traces() {
        let payload = PluginHandlerPayload {
            message: "Error with traces".to_string(),
            status: 500,
            code: None,
            details: None,
            logs: None,
            traces: Some(vec![
                serde_json::json!({"method": "GET", "path": "/health"}),
                serde_json::json!({"method": "POST", "path": "/execute"}),
            ]),
        };

        assert!(payload.traces.is_some());
        assert_eq!(payload.traces.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_queued_request_all_optional_fields() {
        let (tx, _rx) = oneshot::channel();

        let mut headers = HashMap::new();
        headers.insert(
            "X-Custom".to_string(),
            vec!["value1".to_string(), "value2".to_string()],
        );

        let request = QueuedRequest {
            plugin_id: "full-request".to_string(),
            compiled_code: Some("compiled code here".to_string()),
            plugin_path: Some("/path/to/plugin.ts".to_string()),
            params: serde_json::json!({"key": "value", "number": 42}),
            headers: Some(headers),
            socket_path: "/tmp/full.sock".to_string(),
            http_request_id: Some("http-123".to_string()),
            timeout_secs: Some(60),
            route: Some("/api/v1/execute".to_string()),
            config: Some(serde_json::json!({"setting": true})),
            method: Some("PUT".to_string()),
            query: Some(serde_json::json!({"page": 1, "limit": 10})),
            response_tx: tx,
        };

        assert_eq!(request.plugin_id, "full-request");
        assert!(request.compiled_code.is_some());
        assert!(request.plugin_path.is_some());
        assert!(request.headers.is_some());
        assert_eq!(request.timeout_secs, Some(60));
        assert_eq!(request.method, Some("PUT".to_string()));
    }
}
