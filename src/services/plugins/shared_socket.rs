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
use dashmap::DashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

use super::PluginError;

/// Execution context for trace collection
struct ExecutionContext {
    /// Channel to send traces back to the execution
    traces_tx: mpsc::Sender<Vec<serde_json::Value>>,
}

/// Shared socket service that handles multiple concurrent plugin executions
pub struct SharedSocketService {
    /// Socket path
    socket_path: String,
    /// Active execution contexts (execution_id -> ExecutionContext)
    executions: Arc<DashMap<String, ExecutionContext>>,
    /// Whether the listener has been started (instance-level flag)
    started: AtomicBool,
    /// Shutdown signal sender
    shutdown_tx: watch::Sender<bool>,
    /// Current active connection count
    active_connections: Arc<AtomicUsize>,
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

        Ok(Self {
            socket_path: socket_path.to_string(),
            executions: Arc::new(DashMap::new()),
            started: AtomicBool::new(false),
            shutdown_tx,
            active_connections: Arc::new(AtomicUsize::new(0)),
            idle_timeout,
            read_timeout,
        })
    }

    pub fn socket_path(&self) -> &str {
        &self.socket_path
    }

    /// Register an execution and return a receiver for traces
    pub fn register_execution(
        &self,
        execution_id: String,
    ) -> mpsc::Receiver<Vec<serde_json::Value>> {
        let (tx, rx) = mpsc::channel(1);
        self.executions
            .insert(execution_id, ExecutionContext { traces_tx: tx });
        rx
    }

    /// Unregister an execution
    pub fn unregister_execution(&self, execution_id: &str) {
        self.executions.remove(execution_id);
    }

    /// Get current active connection count
    pub fn active_connection_count(&self) -> usize {
        self.active_connections.load(Ordering::Relaxed)
    }

    /// Signal shutdown to the listener
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        info!("Shared socket service: shutdown signal sent");
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
        let active_connections = self.active_connections.clone();
        let idle_timeout = self.idle_timeout;
        let read_timeout = self.read_timeout;
        let max_connections = get_config().socket_max_connections;

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
                                // Check connection limit
                                let current = active_connections.load(Ordering::Relaxed);
                                if current >= max_connections {
                                    warn!(
                                        current_connections = current,
                                        max_connections = max_connections,
                                        "Connection limit reached, rejecting new connection. \
                                        Consider increasing PLUGIN_MAX_CONCURRENCY or PLUGIN_SOCKET_MAX_CONCURRENT_CONNECTIONS."
                                    );
                                    drop(stream);
                                    continue;
                                }

                                active_connections.fetch_add(1, Ordering::Relaxed);
                                debug!(
                                    active_connections = active_connections.load(Ordering::Relaxed),
                                    "Shared socket service: accepted new connection"
                                );

                                let relayer_api_clone = relayer_api.clone();
                                let state_clone = Arc::clone(&state);
                                let executions_clone = executions.clone();
                                let active_connections_clone = active_connections.clone();

                                tokio::spawn(async move {
                                    let result = Self::handle_connection_with_timeout(
                                        stream,
                                        relayer_api_clone,
                                        state_clone,
                                        executions_clone,
                                        idle_timeout,
                                        read_timeout,
                                    )
                                    .await;

                                    // Always decrement connection count
                                    active_connections_clone.fetch_sub(1, Ordering::Relaxed);

                                    if let Err(e) = result {
                                        debug!("Connection handler finished with error: {}", e);
                                    }
                                });
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
        executions: Arc<DashMap<String, ExecutionContext>>,
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
        executions: Arc<DashMap<String, ExecutionContext>>,
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

            // Parse JSON once and reuse (fix double parsing)
            let json_value: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(e) => {
                    warn!("Failed to parse JSON: {}", e);
                    continue;
                }
            };

            // Store raw JSON for traces
            traces.push(json_value.clone());

            // Deserialize into Request from the already-parsed Value
            let request: Request = match serde_json::from_value(json_value) {
                Ok(req) => req,
                Err(e) => {
                    warn!("Failed to parse request structure: {}", e);
                    continue;
                }
            };

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

        if let Some(exec_id) = execution_id {
            if let Some(ctx) = executions.get(&exec_id) {
                let _ = ctx.traces_tx.send(traces).await;
            }
        }

        debug!("Shared socket service: connection closed");
        Ok(())
    }
}

impl Drop for SharedSocketService {
    fn drop(&mut self) {
        // Signal shutdown and cleanup socket file
        let _ = self.shutdown_tx.send(true);
        let _ = std::fs::remove_file(&self.socket_path);
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
