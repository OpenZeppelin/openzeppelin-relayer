//! This module is the orchestrator of the plugin execution.
//!
//! 1. Initiates a socket connection to the relayer server - socket.rs
//! 2. Executes the plugin script - script_executor.rs OR pool_executor.rs
//! 3. Sends the shutdown signal to the relayer server - socket.rs
//! 4. Waits for the relayer server to finish the execution - socket.rs
//! 5. Returns the output of the script - script_executor.rs
//!
//! ## Execution Modes
//!
//! - **ts-node mode** (default): Spawns ts-node per request. Simple but slower.
//! - **Pool mode** (`PLUGIN_USE_POOL=true`): Uses persistent Piscina worker pool.
//!   Faster execution with precompilation and worker reuse.
//!
use std::{collections::HashMap, sync::Arc, time::Duration};

use crate::services::plugins::{
    ensure_shared_socket_started, get_pool_manager, get_shared_socket_service, RelayerApi,
    ScriptExecutor, ScriptResult, SocketService,
};
use crate::{
    jobs::JobProducerTrait,
    models::{
        NetworkRepoModel, NotificationRepoModel, RelayerRepoModel, SignerRepoModel,
        ThinDataAppState, TransactionRepoModel,
    },
    repositories::{
        ApiKeyRepositoryTrait, NetworkRepository, PluginRepositoryTrait, RelayerRepository,
        Repository, TransactionCounterTrait, TransactionRepository,
    },
};

use super::PluginError;
use crate::constants::DEFAULT_TRACE_TIMEOUT_SECS;
use async_trait::async_trait;
use tokio::{sync::oneshot, time::timeout};
use tracing::warn;
use uuid::Uuid;

#[cfg(test)]
use mockall::automock;

/// Check if pool-based execution is enabled via environment variable
fn use_pool_executor() -> bool {
    std::env::var("PLUGIN_USE_POOL")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false)
}

/// Get trace timeout duration - defaults to 5s but configurable
fn get_trace_timeout() -> Duration {
    Duration::from_secs(
        std::env::var("PLUGIN_TRACE_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_TRACE_TIMEOUT_SECS)
    )
}

#[cfg_attr(test, automock)]
#[async_trait]
pub trait PluginRunnerTrait {
    #[allow(clippy::type_complexity, clippy::too_many_arguments)]
    async fn run<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
        &self,
        plugin_id: String,
        socket_path: &str,
        script_path: String,
        timeout_duration: Duration,
        script_params: String,
        http_request_id: Option<String>,
        headers_json: Option<String>,
        state: Arc<ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
    ) -> Result<ScriptResult, PluginError>
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
        AKR: ApiKeyRepositoryTrait + Send + Sync + 'static;
}

#[derive(Default)]
pub struct PluginRunner;

#[async_trait]
impl PluginRunnerTrait for PluginRunner {
    async fn run<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
        &self,
        plugin_id: String,
        socket_path: &str,
        script_path: String,
        timeout_duration: Duration,
        script_params: String,
        http_request_id: Option<String>,
        headers_json: Option<String>,
        state: Arc<ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
    ) -> Result<ScriptResult, PluginError>
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
        // Choose execution mode based on environment variable
        if use_pool_executor() {
            return self
                .run_with_pool(
                    plugin_id,
                    socket_path,
                    script_path,
                    timeout_duration,
                    script_params,
                    http_request_id,
                    headers_json,
                    state,
                )
                .await;
        }

        // Default: ts-node execution
        self.run_with_tsnode(
            plugin_id,
            socket_path,
            script_path,
            timeout_duration,
            script_params,
            http_request_id,
            headers_json,
            state,
        )
        .await
    }
}

impl PluginRunner {
    /// Execute plugin using ts-node (legacy mode)
    #[allow(clippy::too_many_arguments)]
    async fn run_with_tsnode<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
        &self,
        plugin_id: String,
        socket_path: &str,
        script_path: String,
        timeout_duration: Duration,
        script_params: String,
        http_request_id: Option<String>,
        headers_json: Option<String>,
        state: Arc<ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
    ) -> Result<ScriptResult, PluginError>
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
        let socket_service = SocketService::new(socket_path)?;
        let socket_path_clone = socket_service.socket_path().to_string();

        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let server_handle = tokio::spawn(async move {
            let relayer_api = Arc::new(RelayerApi);
            socket_service.listen(shutdown_rx, state, relayer_api).await
        });

        let exec_outcome = match timeout(
            timeout_duration,
            ScriptExecutor::execute_typescript(
                plugin_id,
                script_path,
                socket_path_clone,
                script_params,
                http_request_id,
                headers_json,
            ),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                // ensures the socket gets closed.
                let _ = shutdown_tx.send(());
                return Err(PluginError::ScriptTimeout(timeout_duration.as_secs()));
            }
        };

        let _ = shutdown_tx.send(());

        let server_handle = server_handle
            .await
            .map_err(|e| PluginError::SocketError(e.to_string()))?;

        let traces = match server_handle {
            Ok(traces) => traces,
            Err(e) => return Err(PluginError::SocketError(e.to_string())),
        };

        match exec_outcome {
            Ok(mut script_result) => {
                // attach traces on success
                script_result.trace = traces;
                Ok(script_result)
            }
            Err(err) => Err(err.with_traces(traces)),
        }
    }

    /// Execute plugin using worker pool (new high-performance mode)
    /// Uses shared socket service for better scalability
    #[allow(clippy::too_many_arguments)]
    async fn run_with_pool<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
        &self,
        plugin_id: String,
        _socket_path: &str, // Unused - we use shared socket instead
        script_path: String,
        timeout_duration: Duration,
        script_params: String,
        http_request_id: Option<String>,
        headers_json: Option<String>,
        state: Arc<ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
    ) -> Result<ScriptResult, PluginError>
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
        // Ensure shared socket service is started
        ensure_shared_socket_started(Arc::clone(&state)).await?;
        
        // Get shared socket service
        let shared_socket = get_shared_socket_service()?;
        let shared_socket_path = shared_socket.socket_path().to_string();

        // Generate execution ID (use http_request_id if available, otherwise generate one)
        let execution_id = http_request_id.clone().unwrap_or_else(|| {
            format!("exec-{}", Uuid::new_v4())
        });

        // Register execution to receive traces
        let mut traces_rx = shared_socket.register_execution(execution_id.clone());

        // Execute via pool manager (using shared socket path)
        let pool_manager = get_pool_manager();

        // Parse params as JSON Value
        let params: serde_json::Value = serde_json::from_str(&script_params)
            .unwrap_or(serde_json::Value::String(script_params.clone()));

        // Parse headers if present
        let headers: Option<HashMap<String, Vec<String>>> = headers_json
            .as_ref()
            .and_then(|h| serde_json::from_str(h).ok());

        let exec_outcome = match timeout(
            timeout_duration,
            pool_manager.execute_plugin(
                plugin_id.clone(),
                None,                   // compiled_code - will be fetched from cache
                Some(script_path),      // plugin_path
                params,
                headers,
                shared_socket_path,     // Use shared socket path instead of unique one
                http_request_id,
                Some(timeout_duration.as_secs()),
            ),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                shared_socket.unregister_execution(&execution_id);
                return Err(PluginError::ScriptTimeout(timeout_duration.as_secs()));
            }
        };

        // Wait for traces with configurable timeout
        // Use minimum of trace_timeout and timeout_duration to not exceed execution timeout
        let trace_timeout = get_trace_timeout().min(timeout_duration);
        let traces = match timeout(trace_timeout, traces_rx.recv()).await {
            Ok(Some(traces)) => traces,
            Ok(None) => {
                warn!("Traces channel closed before receiving traces");
                Vec::new()
            }
            Err(_) => {
                warn!(
                    timeout_secs = trace_timeout.as_secs(),
                    "Timeout waiting for traces - consider increasing PLUGIN_TRACE_TIMEOUT_SECS"
                );
                Vec::new()
            }
        };

        shared_socket.unregister_execution(&execution_id);

        match exec_outcome {
            Ok(mut script_result) => {
                script_result.trace = traces;
                Ok(script_result)
            }
            Err(e) => Err(e.with_traces(traces)),
        }
    }
}

#[cfg(test)]
mod tests {
    use actix_web::web;
    use std::fs;

    use crate::{
        jobs::MockJobProducerTrait,
        repositories::{
            ApiKeyRepositoryStorage, NetworkRepositoryStorage, NotificationRepositoryStorage,
            PluginRepositoryStorage, RelayerRepositoryStorage, SignerRepositoryStorage,
            TransactionCounterRepositoryStorage, TransactionRepositoryStorage,
        },
        services::plugins::LogLevel,
        utils::mocks::mockutils::create_mock_app_state,
    };
    use tempfile::tempdir;

    use super::*;

    static TS_CONFIG: &str = r#"
        {
            "compilerOptions": {
              "target": "es2016",
              "module": "commonjs",
              "esModuleInterop": true,
              "forceConsistentCasingInFileNames": true,
              "strict": true,
              "skipLibCheck": true
            }
          }
    "#;

    #[tokio::test]
    async fn test_run() {
        let temp_dir = tempdir().unwrap();
        let ts_config = temp_dir.path().join("tsconfig.json");
        let script_path = temp_dir.path().join("test_run.ts");
        let socket_path = temp_dir.path().join("test_run.sock");

        let content = r#"
            export async function handler(api: any, params: any) {
                console.log('test');
                console.error('test-error');
                return 'test-result';
            }
        "#;
        fs::write(script_path.clone(), content).unwrap();
        fs::write(ts_config.clone(), TS_CONFIG.as_bytes()).unwrap();

        let state = create_mock_app_state(None, None, None, None, None, None).await;

        let plugin_runner = PluginRunner;
        let plugin_id = "test-plugin".to_string();
        let socket_path_str = socket_path.display().to_string();
        let script_path_str = script_path.display().to_string();
        let result = plugin_runner
            .run::<MockJobProducerTrait, RelayerRepositoryStorage, TransactionRepositoryStorage, NetworkRepositoryStorage, NotificationRepositoryStorage, SignerRepositoryStorage, TransactionCounterRepositoryStorage, PluginRepositoryStorage, ApiKeyRepositoryStorage>(
                plugin_id,
                &socket_path_str,
                script_path_str,
                Duration::from_secs(10),
                "{ \"test\": \"test\" }".to_string(),
                None,
                None,
                Arc::new(web::ThinData(state)),
            )
            .await;
        if matches!(
            result,
            Err(PluginError::SocketError(ref msg)) if msg.contains("Operation not permitted")
        ) {
            eprintln!("skipping test_run due to sandbox socket restrictions");
            return;
        }

        let result = result.expect("runner should complete without error");
        assert_eq!(result.logs[0].level, LogLevel::Log);
        assert_eq!(result.logs[0].message, "test");
        assert_eq!(result.logs[1].level, LogLevel::Error);
        assert_eq!(result.logs[1].message, "test-error");
        assert_eq!(result.return_value, "test-result");
    }

    #[tokio::test]
    async fn test_run_timeout() {
        let temp_dir = tempdir().unwrap();
        let ts_config = temp_dir.path().join("tsconfig.json");
        let script_path = temp_dir.path().join("test_simple_timeout.ts");
        let socket_path = temp_dir.path().join("test_simple_timeout.sock");

        // Script that takes 200ms
        let content = r#"
            function sleep(ms) {
                return new Promise(resolve => setTimeout(resolve, ms));
            }

            async function main() {
                await sleep(200); // 200ms
                console.log(JSON.stringify({ level: 'result', message: 'Should not reach here' }));
            }

            main();
        "#;

        fs::write(script_path.clone(), content).unwrap();
        fs::write(ts_config.clone(), TS_CONFIG.as_bytes()).unwrap();

        let state = create_mock_app_state(None, None, None, None, None, None).await;
        let plugin_runner = PluginRunner;

        // Use 100ms timeout for a 200ms script
        let plugin_id = "test-plugin".to_string();
        let socket_path_str = socket_path.display().to_string();
        let script_path_str = script_path.display().to_string();
        let result = plugin_runner
            .run::<MockJobProducerTrait, RelayerRepositoryStorage, TransactionRepositoryStorage, NetworkRepositoryStorage, NotificationRepositoryStorage, SignerRepositoryStorage, TransactionCounterRepositoryStorage, PluginRepositoryStorage, ApiKeyRepositoryStorage>(
                plugin_id,
                &socket_path_str,
                script_path_str,
                Duration::from_millis(100), // 100ms timeout
                "{}".to_string(),
                None,
                None,
                Arc::new(web::ThinData(state)),
            )
            .await;

        // Should timeout
        if matches!(
            result,
            Err(PluginError::SocketError(ref msg)) if msg.contains("Operation not permitted")
        ) {
            eprintln!("skipping test_run_timeout due to sandbox socket restrictions");
            return;
        }

        let err = result.expect_err("runner should timeout");
        assert!(err.to_string().contains("Script execution timed out after"));
    }
}
