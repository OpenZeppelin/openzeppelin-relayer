//! Shared Socket Service
//!
//! This module provides a shared Unix socket service that handles multiple
//! concurrent plugin executions. Instead of creating a new socket per execution,
//! all plugins connect to a single shared socket, dramatically reducing overhead.
//!
//! The key insight: Each plugin execution creates ONE connection, and responses
//! go back over that same connection. The connection itself provides isolation,
//! so we don't need complex routing - just handle each connection independently.

use super::config::get_config;
use crate::jobs::JobProducerTrait;
use crate::models::{
    NetworkRepoModel, NotificationRepoModel, RelayerRepoModel, SignerRepoModel, ThinDataAppState,
    TransactionRepoModel,
};
use crate::repositories::{
    ApiKeyRepositoryTrait, NetworkRepository, PluginRepositoryTrait, RelayerRepository, Repository,
    TransactionCounterTrait, TransactionRepository,
};
use crate::services::plugins::relayer_api::{RelayerApi, Request};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch, RwLock, Semaphore};
use tracing::{debug, info, warn};

use super::PluginError;

/// Execution context for trace collection
struct ExecutionContext {
    /// Channel to send traces back to the execution
    traces_tx: mpsc::Sender<Vec<serde_json::Value>>,
    /// Creation timestamp for TTL cleanup
    created_at: Instant,
}

/// RAII guard for execution registration that auto-unregisters on drop
pub struct ExecutionGuard {
    execution_id: String,
    executions: Arc<RwLock<HashMap<String, ExecutionContext>>>,
    rx: Option<mpsc::Receiver<Vec<serde_json::Value>>>,
}

impl ExecutionGuard {
    /// Get the trace receiver
    pub fn into_receiver(mut self) -> mpsc::Receiver<Vec<serde_json::Value>> {
        self.rx.take().expect("Receiver already taken")
    }
}

impl Drop for ExecutionGuard {
    fn drop(&mut self) {
        // Auto-unregister on drop (prevents memory leaks)
        let executions = self.executions.clone();
        let execution_id = self.execution_id.clone();
        tokio::spawn(async move {
            let mut map = executions.write().await;
            map.remove(&execution_id);
        });
    }
}

/// Shared socket service that handles multiple concurrent plugin executions
pub struct SharedSocketService {
    /// Socket path
    socket_path: String,
    /// Active execution contexts (execution_id -> ExecutionContext)
    /// RwLock is sufficient for write-once, read-once pattern
    executions: Arc<RwLock<HashMap<String, ExecutionContext>>>,
    /// Whether the listener has been started (instance-level flag)
    started: AtomicBool,
    /// Shutdown signal sender
    shutdown_tx: watch::Sender<bool>,
    /// Semaphore for connection limiting (prevents race conditions)
    connection_semaphore: Arc<Semaphore>,
    /// Connection idle timeout
    idle_timeout: Duration,
    /// Read timeout per line
    read_timeout: Duration,
}

impl SharedSocketService {
    /// Create a new shared socket service
    pub fn new(socket_path: &str) -> Result<Self, PluginError> {
        // Remove existing socket file if it exists (from previous runs or crashed processes)
        let _ = std::fs::remove_file(socket_path);

        let (shutdown_tx, _) = watch::channel(false);

        // Use centralized config
        let config = get_config();
        let idle_timeout = Duration::from_secs(config.socket_idle_timeout_secs);
        let read_timeout = Duration::from_secs(config.socket_read_timeout_secs);
        let max_connections = config.socket_max_connections;

        let executions: Arc<RwLock<HashMap<String, ExecutionContext>>> =
            Arc::new(RwLock::new(HashMap::new()));

        // Spawn background cleanup task for stale executions (prevents memory leaks)
        let executions_clone = executions.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                let now = Instant::now();
                let mut map = executions_clone.write().await;
                // Remove entries older than 5 minutes
                map.retain(|_, ctx| now.duration_since(ctx.created_at) < Duration::from_secs(300));
            }
        });

        Ok(Self {
            socket_path: socket_path.to_string(),
            executions,
            started: AtomicBool::new(false),
            shutdown_tx,
            connection_semaphore: Arc::new(Semaphore::new(max_connections)),
            idle_timeout,
            read_timeout,
        })
    }

    pub fn socket_path(&self) -> &str {
        &self.socket_path
    }

    /// Register an execution and return a guard that auto-unregisters on drop
    /// This prevents memory leaks from forgotten unregister calls
    pub async fn register_execution(&self, execution_id: String) -> ExecutionGuard {
        let (tx, rx) = mpsc::channel(1);
        let mut map = self.executions.write().await;
        map.insert(
            execution_id.clone(),
            ExecutionContext {
                traces_tx: tx,
                created_at: Instant::now(),
            },
        );

        ExecutionGuard {
            execution_id,
            executions: self.executions.clone(),
            rx: Some(rx),
        }
    }

    /// Get current active connection count (semaphore available permits)
    pub fn active_connection_count(&self) -> usize {
        self.connection_semaphore.available_permits()
    }

    /// Signal shutdown to the listener and wait for active connections to drain
    pub async fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        info!("Shared socket service: shutdown signal sent");

        // Wait for active connections to drain (max 30 seconds)
        let max_wait = Duration::from_secs(30);
        let start = Instant::now();

        while start.elapsed() < max_wait {
            let available = self.connection_semaphore.available_permits();
            if available == get_config().socket_max_connections {
                // All permits returned - no active connections
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Remove socket file after connections drained
        let _ = std::fs::remove_file(&self.socket_path);
        info!("Shared socket service: shutdown complete");
    }

    /// Start the shared socket service
    /// This spawns a background task that listens for connections
    /// Safe to call multiple times - will only start once per instance
    pub async fn start<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
        self: Arc<Self>,
        state: Arc<ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
    ) -> Result<(), PluginError>
    where
        J: JobProducerTrait + Send + Sync + 'static,
        RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
        TR: TransactionRepository
            + Repository<TransactionRepoModel, String>
            + Send
            + Sync
            + 'static,
        NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
        NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
        SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
        TCR: TransactionCounterTrait + Send + Sync + 'static,
        PR: PluginRepositoryTrait + Send + Sync + 'static,
        AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
    {
        // Check if already started (instance-level flag)
        if self.started.swap(true, Ordering::Acquire) {
            return Ok(());
        }

        // Create the listener and move it into the task
        let listener = UnixListener::bind(&self.socket_path)
            .map_err(|e| PluginError::SocketError(format!("Failed to bind listener: {}", e)))?;
        let executions = self.executions.clone();
        let relayer_api = Arc::new(RelayerApi);
        let socket_path = self.socket_path.clone();
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let connection_semaphore = self.connection_semaphore.clone();
        let idle_timeout = self.idle_timeout;
        let read_timeout = self.read_timeout;

        debug!(
            "Shared socket service: starting listener on {}",
            socket_path
        );

        // Spawn the listener task
        tokio::spawn(async move {
            debug!("Shared socket service: listener task started");
            loop {
                tokio::select! {
                    // Check for shutdown signal
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            info!("Shared socket service: shutting down listener");
                            break;
                        }
                    }
                    // Accept new connections
                    accept_result = listener.accept() => {
                        match accept_result {
                            Ok((stream, _)) => {
                                // Try to acquire semaphore permit (no race condition!)
                                match connection_semaphore.clone().try_acquire_owned() {
                                    Ok(permit) => {
                                        debug!("Shared socket service: accepted new connection");

                                        let relayer_api_clone = relayer_api.clone();
                                        let state_clone = Arc::clone(&state);
                                        let executions_clone = executions.clone();

                                        tokio::spawn(async move {
                                            // Permit held until task completes (auto-released on drop)
                                            let _permit = permit;

                                            let result = Self::handle_connection_with_timeout(
                                                stream,
                                                relayer_api_clone,
                                                state_clone,
                                                executions_clone,
                                                idle_timeout,
                                                read_timeout,
                                            )
                                            .await;

                                            if let Err(e) = result {
                                                debug!("Connection handler finished with error: {}", e);
                                            }
                                        });
                                    }
                                    Err(_) => {
                                        warn!(
                                            "Connection limit reached, rejecting new connection. \
                                            Consider increasing PLUGIN_MAX_CONCURRENCY or PLUGIN_SOCKET_MAX_CONCURRENT_CONNECTIONS."
                                        );
                                        drop(stream);
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("Error accepting connection: {}", e);
                            }
                        }
                    }
                }
            }

            // Cleanup on shutdown
            let _ = std::fs::remove_file(&socket_path);
            info!("Shared socket service: listener stopped");
        });

        Ok(())
    }

    /// Handle connection with overall idle timeout
    async fn handle_connection_with_timeout<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
        stream: UnixStream,
        relayer_api: Arc<RelayerApi>,
        state: Arc<ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
        executions: Arc<RwLock<HashMap<String, ExecutionContext>>>,
        idle_timeout: Duration,
        read_timeout: Duration,
    ) -> Result<(), PluginError>
    where
        J: JobProducerTrait + Send + Sync + 'static,
        RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
        TR: TransactionRepository
            + Repository<TransactionRepoModel, String>
            + Send
            + Sync
            + 'static,
        NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
        NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
        SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
        TCR: TransactionCounterTrait + Send + Sync + 'static,
        PR: PluginRepositoryTrait + Send + Sync + 'static,
        AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
    {
        // Wrap the entire connection handling with an idle timeout
        match tokio::time::timeout(
            idle_timeout,
            Self::handle_connection(stream, relayer_api, state, executions, read_timeout),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                debug!("Connection idle timeout reached");
                Ok(())
            }
        }
    }

    /// Handle a connection from a plugin
    async fn handle_connection<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
        stream: UnixStream,
        relayer_api: Arc<RelayerApi>,
        state: Arc<ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
        executions: Arc<RwLock<HashMap<String, ExecutionContext>>>,
        read_timeout: Duration,
    ) -> Result<(), PluginError>
    where
        J: JobProducerTrait + Send + Sync + 'static,
        RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
        TR: TransactionRepository
            + Repository<TransactionRepoModel, String>
            + Send
            + Sync
            + 'static,
        NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
        NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
        SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
        TCR: TransactionCounterTrait + Send + Sync + 'static,
        PR: PluginRepositoryTrait + Send + Sync + 'static,
        AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
    {
        let (r, mut w) = stream.into_split();
        let mut reader = BufReader::new(r).lines();
        let mut traces = Vec::new();
        let mut execution_id: Option<String> = None;

        loop {
            // Read line with timeout to prevent hanging connections
            let line = match tokio::time::timeout(read_timeout, reader.next_line()).await {
                Ok(Ok(Some(line))) => line,
                Ok(Ok(None)) => break, // EOF
                Ok(Err(e)) => {
                    warn!("Error reading from connection: {}", e);
                    break;
                }
                Err(_) => {
                    debug!("Read timeout on connection");
                    break;
                }
            };

            debug!("Shared socket service: received message");

            // Parse JSON once and reuse (avoiding double parsing)
            let json_value: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(e) => {
                    warn!("Failed to parse JSON: {}", e);
                    continue;
                }
            };

            // Deserialize into Request from the already-parsed Value
            let request: Request = match serde_json::from_value(json_value.clone()) {
                Ok(req) => req,
                Err(e) => {
                    warn!("Failed to parse request structure: {}", e);
                    continue;
                }
            };

            // Move JSON into traces (no clone needed)
            traces.push(json_value);

            if execution_id.is_none() {
                execution_id = request
                    .http_request_id
                    .clone()
                    .or_else(|| Some(request.request_id.clone()));
            }

            let response = relayer_api.handle_request(request, &state).await;

            let response_str = serde_json::to_string(&response)
                .map_err(|e| PluginError::PluginError(e.to_string()))?
                + "\n";

            if let Err(e) = w.write_all(response_str.as_bytes()).await {
                warn!("Failed to write response: {}", e);
                break;
            }
        }

        // Send traces back to caller if execution context exists
        if let Some(exec_id) = execution_id {
            let map = executions.read().await;
            if let Some(ctx) = map.get(&exec_id) {
                let _ = ctx.traces_tx.send(traces).await;
            }
        }

        debug!("Shared socket service: connection closed");
        Ok(())
    }
}

impl Drop for SharedSocketService {
    fn drop(&mut self) {
        // Signal shutdown (cleanup happens in shutdown() method)
        let _ = self.shutdown_tx.send(true);
        // Note: Socket file cleanup happens in shutdown() after connections drain
        // Drop can't be async, so proper cleanup should use shutdown() method
    }
}

/// Global shared socket service instance with proper error handling
static SHARED_SOCKET: std::sync::OnceLock<Result<Arc<SharedSocketService>, String>> =
    std::sync::OnceLock::new();

/// Get or create the global shared socket service
/// Returns error if initialization fails instead of panicking
pub fn get_shared_socket_service() -> Result<Arc<SharedSocketService>, PluginError> {
    let socket_path = "/tmp/relayer-plugin-shared.sock";

    let result = SHARED_SOCKET.get_or_init(|| {
        // Remove existing socket file if it exists (from previous runs)
        let _ = std::fs::remove_file(socket_path);

        match SharedSocketService::new(socket_path) {
            Ok(service) => Ok(Arc::new(service)),
            Err(e) => Err(e.to_string()),
        }
    });

    match result {
        Ok(service) => Ok(service.clone()),
        Err(e) => Err(PluginError::SocketError(format!(
            "Failed to create shared socket service: {}",
            e
        ))),
    }
}

/// Ensure the shared socket service is started
pub async fn ensure_shared_socket_started<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    state: Arc<ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
) -> Result<(), PluginError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
    AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
{
    let service = get_shared_socket_service()?;
    service.start(state).await
}
