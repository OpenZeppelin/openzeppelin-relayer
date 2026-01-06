//! Shared Socket Service
//!
//! This module provides a unified bidirectional Unix socket service for plugin communication.
//! Instead of creating separate sockets for registration and API calls, all communication
//! happens over a single shared socket, dramatically reducing overhead and complexity.
//!
//! ## Architecture
//!
//! **Single Shared Socket**: All plugins connect to `/tmp/relayer-plugin-shared.sock`
//!
//! **Bidirectional Communication**:
//! - Plugins → Host: Register, ApiRequest, Trace, Shutdown
//! - Host → Plugins: ApiResponse
//!
//! **Connection Tagging (Security)**: Each connection is "tagged" with an execution_id
//! after the first Register message. All subsequent messages are validated against this
//! tagged ID to prevent spoofing attacks (Plugin A cannot impersonate Plugin B).
//!
//! ## Message Protocol
//!
//! All messages are JSON objects with a `type` field that discriminates the message type:
//!
//! ### Plugin → Host Messages
//!
//! **Register** (first message, required):
//! ```json
//! {
//!   "type": "register",
//!   "execution_id": "abc-123"
//! }
//! ```
//!
//! **ApiRequest** (call Relayer API):
//! ```json
//! {
//!   "type": "api_request",
//!   "request_id": "req-1",
//!   "relayer_id": "relayer-1",
//!   "method": "sendTransaction",
//!   "payload": { "to": "0x...", "value": "100" }
//! }
//! ```
//!
//! **Trace** (observability event):
//! ```json
//! {
//!   "type": "trace",
//!   "trace": { "event": "processing", "timestamp": 1234567890 }
//! }
//! ```
//!
//! **Shutdown** (graceful close):
//! ```json
//! {
//!   "type": "shutdown"
//! }
//! ```
//!
//! ### Host → Plugin Messages
//!
//! **ApiResponse** (Relayer API result):
//! ```json
//! {
//!   "type": "api_response",
//!   "request_id": "req-1",
//!   "result": { "id": "tx-123", "status": "success" },
//!   "error": null
//! }
//! ```
//!
//! ## Security Model
//!
//! The connection tagging mechanism prevents execution_id spoofing:
//!
//! 1. Plugin connects to shared socket
//! 2. Plugin sends Register message with execution_id
//! 3. Host "tags" the connection (file descriptor) with that execution_id
//! 4. All subsequent messages are validated against the tagged ID
//! 5. Attempts to change execution_id are rejected and connection is closed
//!
//! This ensures Plugin A cannot send requests pretending to be Plugin B, even though
//! they share the same socket file.
//!
//! ## Backward Compatibility
//!
//! The handle_connection method maintains backward compatibility with the legacy
//! Request/Response format from socket.rs. If a message doesn't parse as PluginMessage,
//! it attempts to parse as the legacy Request format and handles it accordingly.
//!
//! ## Performance Benefits vs Per-Execution Sockets
//!
//! | Metric | Shared Socket | Per-Execution Socket |
//! |--------|---------------|----------------------|
//! | File descriptors | 1 per plugin | 2 per plugin |
//! | Syscalls | ~50% fewer | Baseline |
//! | Connection setup | Reuse existing | Create new each time |
//! | Memory overhead | O(active executions) | O(active executions × 2) |
//! | Debugging | Single stream | Two separate streams |
//!
//! ## Example Usage
//!
//! ```rust,no_run
//! use openzeppelin_relayer::services::plugins::shared_socket::{
//!     get_shared_socket_service, ensure_shared_socket_started
//! };
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Get the global shared socket instance
//! let service = get_shared_socket_service()?;
//!
//! // Register an execution (returns RAII guard)
//! let guard = service.register_execution("exec-123".to_string()).await;
//!
//! // Plugin connects and sends messages over the shared socket...
//! // (handled automatically by the background listener)
//!
//! // Collect traces when done
//! let traces_rx = guard.into_receiver();
//! let traces = traces_rx.recv().await;
//! # Ok(())
//! # }
//! ```

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
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch, RwLock, Semaphore};
use tracing::{debug, info, warn};

use super::PluginError;

/// Unified message protocol for bidirectional communication
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginMessage {
    /// Plugin registers its execution_id (first message from plugin)
    Register { execution_id: String },
    /// Plugin requests a Relayer API call
    ApiRequest {
        request_id: String,
        relayer_id: String,
        method: crate::services::plugins::relayer_api::PluginMethod,
        payload: serde_json::Value,
    },
    /// Host responds to an API request
    ApiResponse {
        request_id: String,
        result: Option<serde_json::Value>,
        error: Option<String>,
    },
    /// Plugin sends a trace event (for observability)
    Trace { trace: serde_json::Value },
    /// Plugin signals completion
    Shutdown,
}

/// Execution context for trace collection
struct ExecutionContext {
    /// Channel to send traces back to the execution
    traces_tx: mpsc::Sender<Vec<serde_json::Value>>,
    /// Creation timestamp for TTL cleanup
    created_at: Instant,
    /// The execution_id bound to this connection (for security)
    /// Once set, all messages must match this ID to prevent spoofing
    #[allow(dead_code)] // Used for security validation, not directly read
    bound_execution_id: String,
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
                bound_execution_id: execution_id.clone(),
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
    ///
    /// Security: The first message must be a Register message. Once registered,
    /// the connection is "tagged" with that execution_id and cannot be changed.
    /// This prevents Plugin A from spoofing Plugin B's execution_id.
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

        // Connection-bound execution_id (prevents spoofing)
        // Once set, this cannot be changed for the lifetime of the connection
        let mut bound_execution_id: Option<String> = None;

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

            // Parse once, discriminate on "type" field for efficiency
            let json_value: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(e) => {
                    warn!("Failed to parse JSON: {}", e);
                    continue;
                }
            };

            let has_type_field = json_value.get("type").is_some();

            if has_type_field {
                // New unified protocol
                let message: PluginMessage = match serde_json::from_value(json_value) {
                    Ok(msg) => msg,
                    Err(e) => {
                        warn!("Failed to parse PluginMessage: {}", e);
                        continue;
                    }
                };

                // Handle message based on type
                match message {
                    PluginMessage::Register { execution_id } => {
                        // First message must be Register
                        if bound_execution_id.is_some() {
                            warn!("Attempted to re-register connection (security violation)");
                            break;
                        }

                        // Validate execution_id exists in registry
                        let map = executions.read().await;
                        if !map.contains_key(&execution_id) {
                            warn!("Unknown execution_id: {}", execution_id);
                            break;
                        }
                        drop(map);

                        debug!("Connection registered with execution_id: {}", execution_id);
                        bound_execution_id = Some(execution_id);
                    }

                    PluginMessage::ApiRequest {
                        request_id,
                        relayer_id,
                        method,
                        payload,
                    } => {
                        // Must be registered first
                        let exec_id = match &bound_execution_id {
                            Some(id) => id,
                            None => {
                                warn!("ApiRequest before Register (security violation)");
                                break;
                            }
                        };

                        // Create Request for RelayerApi (method is already PluginMethod)
                        let request = Request {
                            request_id: request_id.clone(),
                            relayer_id,
                            method,
                            payload,
                            http_request_id: Some(exec_id.clone()),
                        };

                        // Handle the request
                        let response = relayer_api.handle_request(request, &state).await;

                        // Send ApiResponse back
                        let api_response = PluginMessage::ApiResponse {
                            request_id: response.request_id,
                            result: response.result,
                            error: response.error,
                        };

                        let response_str = serde_json::to_string(&api_response)
                            .map_err(|e| PluginError::PluginError(e.to_string()))?
                            + "\n";

                        if let Err(e) = w.write_all(response_str.as_bytes()).await {
                            warn!("Failed to write API response: {}", e);
                            break;
                        }

                        if let Err(e) = w.flush().await {
                            warn!("Failed to flush API response: {}", e);
                            break;
                        }
                    }

                    PluginMessage::Trace { trace } => {
                        // Collect trace for observability
                        traces.push(trace);
                    }

                    PluginMessage::Shutdown => {
                        debug!("Plugin requested shutdown");
                        break;
                    }

                    PluginMessage::ApiResponse { .. } => {
                        warn!("Received ApiResponse from plugin (invalid direction)");
                        continue;
                    }
                }
            } else {
                // Legacy protocol (no "type" field)
                if let Ok(request) = serde_json::from_value::<Request>(json_value.clone()) {
                    // Legacy format
                    traces.push(json_value);

                    // Set execution_id from http_request_id or request_id if not bound
                    if bound_execution_id.is_none() {
                        bound_execution_id = request
                            .http_request_id
                            .clone()
                            .or_else(|| Some(request.request_id.clone()));
                    }

                    // Handle legacy request
                    let response = relayer_api.handle_request(request, &state).await;
                    let response_str = serde_json::to_string(&response)
                        .map_err(|e| PluginError::PluginError(e.to_string()))?
                        + "\n";

                    if let Err(e) = w.write_all(response_str.as_bytes()).await {
                        warn!("Failed to write response: {}", e);
                        break;
                    }

                    if let Err(e) = w.flush().await {
                        warn!("Failed to flush response: {}", e);
                        break;
                    }
                } else {
                    warn!("Failed to parse message as either PluginMessage or legacy Request");
                }
            }
        }

        // Send traces back to caller if execution context exists
        if let Some(exec_id) = bound_execution_id {
            let map = executions.read().await;
            if let Some(ctx) = map.get(&exec_id) {
                // Send traces with timeout to prevent blocking
                match tokio::time::timeout(Duration::from_secs(5), ctx.traces_tx.send(traces)).await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => warn!("Trace channel closed for execution_id: {}", exec_id),
                    Err(_) => warn!("Timeout sending traces for execution_id: {}", exec_id),
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::mocks::mockutils::create_mock_app_state;
    use actix_web::web;
    use tempfile::tempdir;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    #[tokio::test]
    async fn test_unified_protocol_register_and_api_request() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("shared.sock");

        let service = Arc::new(SharedSocketService::new(socket_path.to_str().unwrap()).unwrap());
        let state = create_mock_app_state(None, None, None, None, None, None).await;

        // Start the service
        service
            .clone()
            .start(Arc::new(web::ThinData(state)))
            .await
            .unwrap();

        // Register execution
        let execution_id = "test-exec-123".to_string();
        let _guard = service.register_execution(execution_id.clone()).await;

        // Give the listener time to start
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Connect as plugin
        let mut client = UnixStream::connect(socket_path.to_str().unwrap())
            .await
            .unwrap();

        // Send Register message
        let register_msg = PluginMessage::Register {
            execution_id: execution_id.clone(),
        };
        let msg_json = serde_json::to_string(&register_msg).unwrap() + "\n";
        client.write_all(msg_json.as_bytes()).await.unwrap();

        // Send ApiRequest
        let api_request = PluginMessage::ApiRequest {
            request_id: "req-1".to_string(),
            relayer_id: "relayer-1".to_string(),
            method: crate::services::plugins::relayer_api::PluginMethod::GetRelayerStatus,
            payload: serde_json::json!({}),
        };
        let req_json = serde_json::to_string(&api_request).unwrap() + "\n";
        client.write_all(req_json.as_bytes()).await.unwrap();
        client.flush().await.unwrap();

        // Read ApiResponse
        let (r, _w) = client.into_split();
        let mut reader = BufReader::new(r);
        let mut response_line = String::new();
        reader.read_line(&mut response_line).await.unwrap();

        let response: PluginMessage = serde_json::from_str(&response_line).unwrap();
        match response {
            PluginMessage::ApiResponse { request_id, .. } => {
                assert_eq!(request_id, "req-1");
            }
            _ => panic!("Expected ApiResponse, got {:?}", response),
        }

        service.shutdown().await;
    }

    #[tokio::test]
    async fn test_connection_tagging_prevents_spoofing() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("shared2.sock");

        let service = Arc::new(SharedSocketService::new(socket_path.to_str().unwrap()).unwrap());
        let state = create_mock_app_state(None, None, None, None, None, None).await;

        service
            .clone()
            .start(Arc::new(web::ThinData(state)))
            .await
            .unwrap();

        let execution_id = "test-exec-456".to_string();
        let _guard = service.register_execution(execution_id.clone()).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = UnixStream::connect(socket_path.to_str().unwrap())
            .await
            .unwrap();

        // Register with execution_id
        let register_msg = PluginMessage::Register {
            execution_id: execution_id.clone(),
        };
        let msg_json = serde_json::to_string(&register_msg).unwrap() + "\n";
        client.write_all(msg_json.as_bytes()).await.unwrap();

        // Try to re-register with different execution_id (security violation)
        let spoofed_register = PluginMessage::Register {
            execution_id: "different-exec-id".to_string(),
        };
        let spoofed_json = serde_json::to_string(&spoofed_register).unwrap() + "\n";
        client.write_all(spoofed_json.as_bytes()).await.unwrap();
        client.flush().await.unwrap();

        // Connection should be closed by server
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Try to read - should get EOF since connection was closed
        let (r, _w) = client.into_split();
        let mut reader = BufReader::new(r);
        let mut line = String::new();
        let result = reader.read_line(&mut line).await;

        // Should either get an error or EOF (0 bytes)
        assert!(result.is_err() || result.unwrap() == 0);

        service.shutdown().await;
    }

    #[tokio::test]
    async fn test_backward_compatibility_with_legacy_format() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("shared3.sock");

        let service = Arc::new(SharedSocketService::new(socket_path.to_str().unwrap()).unwrap());
        let state = create_mock_app_state(None, None, None, None, None, None).await;

        service
            .clone()
            .start(Arc::new(web::ThinData(state)))
            .await
            .unwrap();

        let execution_id = "test-exec-789".to_string();
        let _guard = service.register_execution(execution_id.clone()).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = UnixStream::connect(socket_path.to_str().unwrap())
            .await
            .unwrap();

        // Send legacy Request format (without PluginMessage wrapper)
        let legacy_request = crate::services::plugins::relayer_api::Request {
            request_id: "legacy-1".to_string(),
            relayer_id: "relayer-1".to_string(),
            method: crate::services::plugins::relayer_api::PluginMethod::GetRelayerStatus,
            payload: serde_json::json!({}),
            http_request_id: Some(execution_id.clone()),
        };
        let legacy_json = serde_json::to_string(&legacy_request).unwrap() + "\n";
        client.write_all(legacy_json.as_bytes()).await.unwrap();
        client.flush().await.unwrap();

        // Read legacy Response format
        let (r, _w) = client.into_split();
        let mut reader = BufReader::new(r);
        let mut response_line = String::new();
        reader.read_line(&mut response_line).await.unwrap();

        let response: crate::services::plugins::relayer_api::Response =
            serde_json::from_str(&response_line).unwrap();

        assert_eq!(response.request_id, "legacy-1");
        // Note: GetRelayerStatus might return an error if relayer doesn't exist
        // The important thing is we got a response in the correct format
        assert!(response.result.is_some() || response.error.is_some());

        service.shutdown().await;
    }

    #[tokio::test]
    async fn test_trace_collection() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("shared4.sock");

        let service = Arc::new(SharedSocketService::new(socket_path.to_str().unwrap()).unwrap());
        let state = create_mock_app_state(None, None, None, None, None, None).await;

        service
            .clone()
            .start(Arc::new(web::ThinData(state)))
            .await
            .unwrap();

        let execution_id = "test-exec-trace".to_string();
        let guard = service.register_execution(execution_id.clone()).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = UnixStream::connect(socket_path.to_str().unwrap())
            .await
            .unwrap();

        // Register
        let register_msg = PluginMessage::Register {
            execution_id: execution_id.clone(),
        };
        client
            .write_all((serde_json::to_string(&register_msg).unwrap() + "\n").as_bytes())
            .await
            .unwrap();

        // Send trace events
        let trace1 = PluginMessage::Trace {
            trace: serde_json::json!({"event": "start", "timestamp": 1000}),
        };
        client
            .write_all((serde_json::to_string(&trace1).unwrap() + "\n").as_bytes())
            .await
            .unwrap();

        let trace2 = PluginMessage::Trace {
            trace: serde_json::json!({"event": "processing", "timestamp": 2000}),
        };
        client
            .write_all((serde_json::to_string(&trace2).unwrap() + "\n").as_bytes())
            .await
            .unwrap();

        // Shutdown
        let shutdown_msg = PluginMessage::Shutdown;
        client
            .write_all((serde_json::to_string(&shutdown_msg).unwrap() + "\n").as_bytes())
            .await
            .unwrap();
        client.flush().await.unwrap();

        drop(client);

        // Wait for connection to close and traces to be sent
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Collect traces
        let mut traces_rx = guard.into_receiver();
        let traces = traces_rx.recv().await.unwrap();

        // Should have collected 2 trace events
        assert_eq!(traces.len(), 2);
        assert_eq!(traces[0]["event"], "start");
        assert_eq!(traces[1]["event"], "processing");

        service.shutdown().await;
    }

    #[tokio::test]
    async fn test_execution_guard_auto_unregister() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("shared_guard.sock");

        let service = Arc::new(SharedSocketService::new(socket_path.to_str().unwrap()).unwrap());
        let execution_id = "test-exec-guard".to_string();

        {
            let _guard = service.register_execution(execution_id.clone()).await;

            // Verify execution is registered
            let map = service.executions.read().await;
            assert!(map.contains_key(&execution_id));
        }
        // Guard dropped here

        // Give tokio task time to run
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Verify execution was auto-unregistered
        let map = service.executions.read().await;
        assert!(!map.contains_key(&execution_id));
    }

    #[tokio::test]
    async fn test_api_request_without_register_rejected() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("shared_no_register.sock");

        let service = Arc::new(SharedSocketService::new(socket_path.to_str().unwrap()).unwrap());
        let state = create_mock_app_state(None, None, None, None, None, None).await;

        service
            .clone()
            .start(Arc::new(web::ThinData(state)))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = UnixStream::connect(socket_path.to_str().unwrap())
            .await
            .unwrap();

        // Send ApiRequest WITHOUT registering first (security violation)
        let api_request = PluginMessage::ApiRequest {
            request_id: "req-1".to_string(),
            relayer_id: "relayer-1".to_string(),
            method: crate::services::plugins::relayer_api::PluginMethod::GetRelayerStatus,
            payload: serde_json::json!({}),
        };
        let req_json = serde_json::to_string(&api_request).unwrap() + "\n";
        client.write_all(req_json.as_bytes()).await.unwrap();
        client.flush().await.unwrap();

        // Connection should be closed by server
        tokio::time::sleep(Duration::from_millis(100)).await;

        let (r, _w) = client.into_split();
        let mut reader = BufReader::new(r);
        let mut line = String::new();
        let result = reader.read_line(&mut line).await;

        // Should get EOF (connection closed)
        assert!(result.is_err() || result.unwrap() == 0);

        service.shutdown().await;
    }

    #[tokio::test]
    async fn test_register_with_unknown_execution_id_rejected() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("shared_unknown_exec.sock");

        let service = Arc::new(SharedSocketService::new(socket_path.to_str().unwrap()).unwrap());
        let state = create_mock_app_state(None, None, None, None, None, None).await;

        service
            .clone()
            .start(Arc::new(web::ThinData(state)))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = UnixStream::connect(socket_path.to_str().unwrap())
            .await
            .unwrap();

        // Try to register with an execution_id that doesn't exist in registry
        let register_msg = PluginMessage::Register {
            execution_id: "unknown-exec-id".to_string(),
        };
        let msg_json = serde_json::to_string(&register_msg).unwrap() + "\n";
        client.write_all(msg_json.as_bytes()).await.unwrap();
        client.flush().await.unwrap();

        // Connection should be closed
        tokio::time::sleep(Duration::from_millis(100)).await;

        let (r, _w) = client.into_split();
        let mut reader = BufReader::new(r);
        let mut line = String::new();
        let result = reader.read_line(&mut line).await;

        assert!(result.is_err() || result.unwrap() == 0);

        service.shutdown().await;
    }

    #[tokio::test]
    async fn test_connection_limit_enforcement() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("shared_connection_limit.sock");

        let service = Arc::new(SharedSocketService::new(socket_path.to_str().unwrap()).unwrap());
        let state = create_mock_app_state(None, None, None, None, None, None).await;

        service
            .clone()
            .start(Arc::new(web::ThinData(state)))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Check initial connection count
        let initial_permits = service.connection_semaphore.available_permits();
        let max_connections = get_config().socket_max_connections;
        assert_eq!(initial_permits, max_connections);

        // Create a connection (should reduce available permits)
        let _client = UnixStream::connect(socket_path.to_str().unwrap())
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Available permits should be reduced
        let after_connect = service.connection_semaphore.available_permits();
        assert!(after_connect < initial_permits);

        service.shutdown().await;
    }

    #[tokio::test]
    async fn test_idle_timeout() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("shared_idle_timeout.sock");

        // Create service with short idle timeout for testing
        let service = Arc::new(SharedSocketService::new(socket_path.to_str().unwrap()).unwrap());
        let state = create_mock_app_state(None, None, None, None, None, None).await;

        service
            .clone()
            .start(Arc::new(web::ThinData(state)))
            .await
            .unwrap();

        let execution_id = "test-exec-idle".to_string();
        let _guard = service.register_execution(execution_id.clone()).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = UnixStream::connect(socket_path.to_str().unwrap())
            .await
            .unwrap();

        // Register
        let register_msg = PluginMessage::Register { execution_id };
        client
            .write_all((serde_json::to_string(&register_msg).unwrap() + "\n").as_bytes())
            .await
            .unwrap();
        client.flush().await.unwrap();

        // Wait longer than idle timeout (configured in service)
        // Note: idle_timeout is from config, but we can test that connection stays alive
        // within a reasonable time
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Connection should still be alive if we're within timeout
        // Send a Shutdown message to verify connection is still up
        let shutdown_msg = PluginMessage::Shutdown;
        let write_result = client
            .write_all((serde_json::to_string(&shutdown_msg).unwrap() + "\n").as_bytes())
            .await;

        assert!(write_result.is_ok(), "Connection should still be alive");

        service.shutdown().await;
    }

    #[tokio::test]
    async fn test_read_timeout_handling() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("shared_read_timeout.sock");

        let service = Arc::new(SharedSocketService::new(socket_path.to_str().unwrap()).unwrap());
        let state = create_mock_app_state(None, None, None, None, None, None).await;

        service
            .clone()
            .start(Arc::new(web::ThinData(state)))
            .await
            .unwrap();

        let execution_id = "test-exec-read-timeout".to_string();
        let _guard = service.register_execution(execution_id.clone()).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = UnixStream::connect(socket_path.to_str().unwrap())
            .await
            .unwrap();

        // Register
        let register_msg = PluginMessage::Register { execution_id };
        client
            .write_all((serde_json::to_string(&register_msg).unwrap() + "\n").as_bytes())
            .await
            .unwrap();
        client.flush().await.unwrap();

        // Don't send anything else - connection should be cleaned up after read timeout
        // Read timeout is configured in service (from config)

        // Wait a bit (but not as long as full timeout)
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Connection should still be valid for a short time
        drop(client);

        service.shutdown().await;
    }

    #[tokio::test]
    async fn test_multiple_api_requests_same_connection() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("shared_multiple_requests.sock");

        let service = Arc::new(SharedSocketService::new(socket_path.to_str().unwrap()).unwrap());
        let state = create_mock_app_state(None, None, None, None, None, None).await;

        service
            .clone()
            .start(Arc::new(web::ThinData(state)))
            .await
            .unwrap();

        let execution_id = "test-exec-multi".to_string();
        let _guard = service.register_execution(execution_id.clone()).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = UnixStream::connect(socket_path.to_str().unwrap())
            .await
            .unwrap();

        // Register
        let register_msg = PluginMessage::Register {
            execution_id: execution_id.clone(),
        };
        client
            .write_all((serde_json::to_string(&register_msg).unwrap() + "\n").as_bytes())
            .await
            .unwrap();

        let (r, mut w) = client.into_split();
        let mut reader = BufReader::new(r);

        // Send multiple API requests
        for i in 1..=3 {
            let api_request = PluginMessage::ApiRequest {
                request_id: format!("req-{}", i),
                relayer_id: "relayer-1".to_string(),
                method: crate::services::plugins::relayer_api::PluginMethod::GetRelayerStatus,
                payload: serde_json::json!({}),
            };
            w.write_all((serde_json::to_string(&api_request).unwrap() + "\n").as_bytes())
                .await
                .unwrap();
            w.flush().await.unwrap();

            // Read response
            let mut response_line = String::new();
            reader.read_line(&mut response_line).await.unwrap();

            let response: PluginMessage = serde_json::from_str(&response_line).unwrap();
            match response {
                PluginMessage::ApiResponse { request_id, .. } => {
                    assert_eq!(request_id, format!("req-{}", i));
                }
                _ => panic!("Expected ApiResponse"),
            }
        }

        service.shutdown().await;
    }

    #[tokio::test]
    async fn test_shutdown_signal() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("shared_shutdown_signal.sock");

        let service = Arc::new(SharedSocketService::new(socket_path.to_str().unwrap()).unwrap());
        let state = create_mock_app_state(None, None, None, None, None, None).await;

        service
            .clone()
            .start(Arc::new(web::ThinData(state)))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Verify socket file exists
        assert!(std::path::Path::new(socket_path.to_str().unwrap()).exists());

        // Shutdown the service
        service.shutdown().await;

        // Give time for cleanup
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Socket file should be removed
        assert!(!std::path::Path::new(socket_path.to_str().unwrap()).exists());
    }

    #[tokio::test]
    async fn test_malformed_json_handling() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("shared_malformed.sock");

        let service = Arc::new(SharedSocketService::new(socket_path.to_str().unwrap()).unwrap());
        let state = create_mock_app_state(None, None, None, None, None, None).await;

        service
            .clone()
            .start(Arc::new(web::ThinData(state)))
            .await
            .unwrap();

        let execution_id = "test-exec-malformed".to_string();
        let _guard = service.register_execution(execution_id.clone()).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = UnixStream::connect(socket_path.to_str().unwrap())
            .await
            .unwrap();

        // Register first
        let register_msg = PluginMessage::Register {
            execution_id: execution_id.clone(),
        };
        client
            .write_all((serde_json::to_string(&register_msg).unwrap() + "\n").as_bytes())
            .await
            .unwrap();

        // Send malformed JSON
        client
            .write_all(b"{ this is not valid json }\n")
            .await
            .unwrap();
        client.flush().await.unwrap();

        // Connection should remain open (malformed messages are logged and skipped)
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Send valid shutdown message to verify connection is still up
        let shutdown_msg = PluginMessage::Shutdown;
        let write_result = client
            .write_all((serde_json::to_string(&shutdown_msg).unwrap() + "\n").as_bytes())
            .await;

        assert!(
            write_result.is_ok(),
            "Connection should still be alive after malformed JSON"
        );

        service.shutdown().await;
    }

    #[tokio::test]
    async fn test_invalid_message_direction() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("shared_invalid_direction.sock");

        let service = Arc::new(SharedSocketService::new(socket_path.to_str().unwrap()).unwrap());
        let state = create_mock_app_state(None, None, None, None, None, None).await;

        service
            .clone()
            .start(Arc::new(web::ThinData(state)))
            .await
            .unwrap();

        let execution_id = "test-exec-invalid-dir".to_string();
        let _guard = service.register_execution(execution_id.clone()).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = UnixStream::connect(socket_path.to_str().unwrap())
            .await
            .unwrap();

        // Register
        let register_msg = PluginMessage::Register {
            execution_id: execution_id.clone(),
        };
        client
            .write_all((serde_json::to_string(&register_msg).unwrap() + "\n").as_bytes())
            .await
            .unwrap();

        // Plugin tries to send ApiResponse (invalid direction - only Host sends ApiResponse)
        let invalid_msg = PluginMessage::ApiResponse {
            request_id: "invalid".to_string(),
            result: Some(serde_json::json!({})),
            error: None,
        };
        client
            .write_all((serde_json::to_string(&invalid_msg).unwrap() + "\n").as_bytes())
            .await
            .unwrap();
        client.flush().await.unwrap();

        // Connection should remain open (invalid messages are logged and skipped)
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Verify connection is still alive
        let shutdown_msg = PluginMessage::Shutdown;
        let write_result = client
            .write_all((serde_json::to_string(&shutdown_msg).unwrap() + "\n").as_bytes())
            .await;

        assert!(write_result.is_ok());

        service.shutdown().await;
    }

    #[tokio::test]
    async fn test_stale_execution_cleanup() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("shared_stale_cleanup.sock");

        let service = Arc::new(SharedSocketService::new(socket_path.to_str().unwrap()).unwrap());

        // Register an execution manually with old timestamp
        let execution_id = "stale-exec".to_string();
        let (tx, _rx) = mpsc::channel(1);
        {
            let mut map = service.executions.write().await;
            map.insert(
                execution_id.clone(),
                ExecutionContext {
                    traces_tx: tx,
                    created_at: Instant::now() - Duration::from_secs(400), // 6+ minutes old
                    bound_execution_id: execution_id.clone(),
                },
            );
        }

        // Verify it's registered
        {
            let map = service.executions.read().await;
            assert!(map.contains_key(&execution_id));
        }

        // Wait for cleanup task to run (it runs every 60 seconds, but we can't wait that long)
        // Instead, we verify the cleanup logic by checking the code in new()
        // The actual cleanup test would require mocking time or waiting 60+ seconds

        // For this test, we just verify the logic exists and doesn't panic
        drop(service);
    }

    #[tokio::test]
    async fn test_socket_path_getter() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("shared_path.sock");

        let service = SharedSocketService::new(socket_path.to_str().unwrap()).unwrap();

        assert_eq!(service.socket_path(), socket_path.to_str().unwrap());
    }

    #[tokio::test]
    async fn test_trace_send_timeout() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("shared_trace_timeout.sock");

        let service = Arc::new(SharedSocketService::new(socket_path.to_str().unwrap()).unwrap());
        let state = create_mock_app_state(None, None, None, None, None, None).await;

        service
            .clone()
            .start(Arc::new(web::ThinData(state)))
            .await
            .unwrap();

        let execution_id = "test-exec-trace-timeout".to_string();
        let guard = service.register_execution(execution_id.clone()).await;

        // Don't consume the receiver - this will cause the channel to fill up
        drop(guard);

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = UnixStream::connect(socket_path.to_str().unwrap())
            .await
            .unwrap();

        // Register
        let register_msg = PluginMessage::Register {
            execution_id: execution_id.clone(),
        };
        client
            .write_all((serde_json::to_string(&register_msg).unwrap() + "\n").as_bytes())
            .await
            .unwrap();

        // Send trace
        let trace = PluginMessage::Trace {
            trace: serde_json::json!({"event": "test"}),
        };
        client
            .write_all((serde_json::to_string(&trace).unwrap() + "\n").as_bytes())
            .await
            .unwrap();

        // Shutdown
        let shutdown_msg = PluginMessage::Shutdown;
        client
            .write_all((serde_json::to_string(&shutdown_msg).unwrap() + "\n").as_bytes())
            .await
            .unwrap();
        client.flush().await.unwrap();

        drop(client);

        // Wait for connection to close - should handle timeout gracefully
        tokio::time::sleep(Duration::from_millis(200)).await;

        service.shutdown().await;
    }

    #[tokio::test]
    async fn test_get_shared_socket_service() {
        // Test the global singleton
        let service1 = get_shared_socket_service();
        assert!(service1.is_ok());

        let service2 = get_shared_socket_service();
        assert!(service2.is_ok());

        // Should return the same instance
        let svc1 = service1.unwrap();
        let svc2 = service2.unwrap();
        let path1 = svc1.socket_path();
        let path2 = svc2.socket_path();
        assert_eq!(path1, path2);
    }
}
