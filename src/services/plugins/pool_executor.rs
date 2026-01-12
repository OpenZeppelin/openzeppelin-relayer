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
use tokio::sync::{oneshot, RwLock};
use uuid::Uuid;

use super::config::get_config;
use super::connection::{ConnectionPool, PoolConnection};
use super::health::{
    CircuitBreaker, CircuitState, DeadServerIndicator, HealthStatus, ProcessStatus,
};
use super::protocol::{PoolError, PoolRequest, PoolResponse};
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
    /// Shutdown signal for background tasks (queue workers, health check, etc.)
    shutdown_signal: Arc<tokio::sync::Notify>,
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
                .map(|n| n.get().max(4).min(32))
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
            route,
            config,
            method,
            query,
        };

        let timeout = timeout_secs.unwrap_or(get_config().pool_request_timeout_secs);
        let response = conn.send_request_with_timeout(&request, timeout).await?;

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

    /// Start the pool server if not already running
    pub async fn ensure_started(&self) -> Result<(), PluginError> {
        if *self.initialized.read().await {
            return Ok(());
        }

        let _startup_guard = self.restart_lock.lock().await;

        if *self.initialized.read().await {
            return Ok(());
        }

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

        if !self.health_check_needed.load(Ordering::Relaxed) {
            return Ok(());
        }

        if !self
            .health_check_needed
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return Ok(());
        }

        self.check_and_restart_if_needed().await
    }

    /// Check process status and restart if needed
    async fn check_and_restart_if_needed(&self) -> Result<(), PluginError> {
        let _restart_guard = self.restart_lock.lock().await;

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

        match process_status {
            ProcessStatus::Running => {
                let socket_exists = std::path::Path::new(&self.socket_path).exists();
                if !socket_exists {
                    tracing::warn!(
                        socket_path = %self.socket_path,
                        "Pool server socket file missing, restarting"
                    );
                    self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
                    self.restart_internal().await?;
                    self.consecutive_failures.store(0, Ordering::Relaxed);
                }
            }
            ProcessStatus::Exited | ProcessStatus::Unknown | ProcessStatus::NoProcess => {
                tracing::warn!("Pool server not running, restarting");
                self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
                self.restart_internal().await?;
                self.consecutive_failures.store(0, Ordering::Relaxed);
            }
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

        let calculated_heap = 512 + ((config.max_concurrency / 10) * 32);
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
            socket_path = %socket_path,
            heap_mb = pool_server_heap_mb,
            max_concurrency = config.max_concurrency,
            context = context,
            "Spawning plugin pool server"
        );

        let node_options = format!("--max-old-space-size={} --expose-gc", pool_server_heap_mb);

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
                PluginError::PluginExecutionError(format!("Failed to {} pool server: {e}", context))
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
                        "Timeout waiting for pool server to {}",
                        context
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

        let circuit_breaker = self.circuit_breaker.clone();
        match self.connection_pool.semaphore.clone().try_acquire_owned() {
            Ok(permit) => {
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
                            let pool = self.connection_pool.clone();
                            tokio::spawn(async move {
                                pool.clear().await;
                            });
                        } else {
                            // Plugin executed but returned error - infrastructure is healthy
                            circuit_breaker.record_success(elapsed_ms);
                        }
                    }
                }

                result
            }
            Err(_) => {
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
                        response_rx.await.map_err(|_| {
                            PluginError::PluginExecutionError(
                                "Request queue processor closed".to_string(),
                            )
                        })?
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
                                response_rx.await.map_err(|_| {
                                    PluginError::PluginExecutionError(
                                        "Request queue processor closed".to_string(),
                                    )
                                })?
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
                            let pool = self.connection_pool.clone();
                            tokio::spawn(async move {
                                pool.clear().await;
                            });
                        } else {
                            // Plugin executed but returned error - infrastructure is healthy
                            circuit_breaker.record_success(elapsed_ms);
                        }
                    }
                }

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

        let mut conn =
            match tokio::time::timeout(Duration::from_millis(100), self.connection_pool.acquire())
                .await
            {
                Ok(Ok(c)) => c,
                Ok(Err(e)) => {
                    let err_str = e.to_string();
                    let is_pool_exhausted =
                        err_str.contains("semaphore") || err_str.contains("Connection refused");

                    let (circuit_state, avg_rt, recovering, recovery_pct) = circuit_info();
                    return Ok(HealthStatus {
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
                    let (circuit_state, avg_rt, recovering, recovery_pct) = circuit_info();
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

        self.connection_pool.clear().await;

        {
            let mut process_guard = self.process.lock().await;
            if let Some(mut child) = process_guard.take() {
                let _ = child.kill().await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        Self::cleanup_socket_file(&self.socket_path).await;

        {
            let mut initialized = self.initialized.write().await;
            *initialized = false;
        }

        let mut process_guard = self.process.lock().await;
        if process_guard.is_some() {
            return Ok(());
        }

        let child = Self::spawn_pool_server_process(&self.socket_path, "restart").await?;
        *process_guard = Some(child);

        {
            let mut initialized = self.initialized.write().await;
            *initialized = true;
        }

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
        let mut initialized = self.initialized.write().await;
        if !*initialized {
            return Ok(());
        }

        tracing::info!("Initiating graceful shutdown of plugin pool server");

        self.shutdown_signal.notify_waiters();

        self.connection_pool.clear().await;

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

        *initialized = false;
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

        let mut conn = match PoolConnection::new(&self.socket_path, 999999).await {
            Ok(c) => c,
            Err(e) => {
                return Err(PluginError::PluginExecutionError(format!(
                    "Failed to connect for shutdown: {}",
                    e
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
}
