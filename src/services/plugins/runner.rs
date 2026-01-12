//! This module is the orchestrator of the plugin execution.
//!
//! 1. Initiates connection to shared socket service - shared_socket.rs
//! 2. Executes the plugin script - script_executor.rs OR pool_executor.rs
//! 3. Collects traces via ExecutionGuard - shared_socket.rs
//! 4. Returns the output of the script - script_executor.rs
//!
//! ## Execution Modes
//!
//! - **Pool mode** (default): Uses persistent Piscina worker pool.
//!   Faster execution with precompilation and worker reuse.
//! - **ts-node mode** (`PLUGIN_USE_POOL=false`): Spawns ts-node per request.
//!   Simple but slower. Uses the shared socket for bidirectional communication.
//!
use std::{collections::HashMap, sync::Arc, time::Duration};

use crate::services::plugins::{
    ensure_shared_socket_started, get_pool_manager, get_shared_socket_service, ScriptExecutor,
    ScriptResult,
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

use super::{config::get_config, PluginError};
use async_trait::async_trait;
use tokio::time::timeout;
use tracing::debug;
use uuid::Uuid;

#[cfg(test)]
use mockall::automock;

/// Check if pool-based execution is enabled via environment variable
/// Pool mode is enabled by default for better performance
fn use_pool_executor() -> bool {
    std::env::var("PLUGIN_USE_POOL")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(true) // Pool mode is now the default
}

/// Get trace timeout duration from centralized config
fn get_trace_timeout() -> Duration {
    Duration::from_millis(get_config().trace_timeout_ms)
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
        route: Option<String>,
        config_json: Option<String>,
        method: Option<String>,
        query_json: Option<String>,
        emit_traces: bool,
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
        route: Option<String>,
        config_json: Option<String>,
        method: Option<String>,
        query_json: Option<String>,
        emit_traces: bool,
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
                    route,
                    config_json,
                    method,
                    query_json,
                    emit_traces,
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
            route,
            config_json,
            method,
            query_json,
            emit_traces,
            state,
        )
        .await
    }
}

impl PluginRunner {
    /// Execute plugin using ts-node with shared socket
    #[allow(clippy::too_many_arguments)]
    async fn run_with_tsnode<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
        &self,
        plugin_id: String,
        _socket_path: &str, // Unused - kept for signature compatibility
        script_path: String,
        timeout_duration: Duration,
        script_params: String,
        http_request_id: Option<String>,
        headers_json: Option<String>,
        route: Option<String>,
        config_json: Option<String>,
        method: Option<String>,
        query_json: Option<String>,
        _emit_traces: bool,
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
        // Ensure shared socket is started
        ensure_shared_socket_started(Arc::clone(&state)).await?;

        // Get the shared socket service
        let shared_socket = get_shared_socket_service()?;
        let shared_socket_path = shared_socket.socket_path().to_string();

        // Generate execution_id from http_request_id or plugin_id
        let execution_id = http_request_id
            .clone()
            .unwrap_or_else(|| format!("{}-{}", plugin_id, uuid::Uuid::new_v4()));

        // Register execution (RAII guard auto-unregisters on drop)
        let guard = shared_socket.register_execution(execution_id.clone()).await;

        let exec_outcome = match timeout(
            timeout_duration,
            ScriptExecutor::execute_typescript(
                plugin_id,
                script_path,
                shared_socket_path, // Use shared socket path
                script_params,
                Some(execution_id),
                headers_json,
                route,
                config_json,
                method,
                query_json,
            ),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                return Err(PluginError::ScriptTimeout(timeout_duration.as_secs()));
            }
        };

        // Collect traces from the guard
        let mut traces_rx = guard.into_receiver();
        let traces = match tokio::time::timeout(Duration::from_secs(5), traces_rx.recv()).await {
            Ok(Some(traces)) => traces,
            Ok(None) => Vec::new(),
            Err(_) => {
                debug!("Timeout waiting for traces");
                Vec::new()
            }
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
        route: Option<String>,
        config_json: Option<String>,
        method: Option<String>,
        query_json: Option<String>,
        emit_traces: bool,
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
        let execution_id = http_request_id
            .clone()
            .unwrap_or_else(|| format!("exec-{}", Uuid::new_v4()));

        // Only register for traces if emit_traces is enabled
        // ExecutionGuard will auto-unregister on drop (RAII pattern)
        let mut traces_rx = if emit_traces {
            let guard = shared_socket.register_execution(execution_id.clone()).await;
            Some(guard.into_receiver())
        } else {
            None
        };

        // Execute via pool manager (using shared socket path)
        let pool_manager = get_pool_manager();

        // Parse params as JSON Value
        let params: serde_json::Value = serde_json::from_str(&script_params)
            .unwrap_or(serde_json::Value::String(script_params.clone()));

        // Parse headers if present
        let headers: Option<HashMap<String, Vec<String>>> = headers_json
            .as_ref()
            .and_then(|h| serde_json::from_str(h).ok());

        // Parse config if present
        let config: Option<serde_json::Value> = config_json
            .as_ref()
            .and_then(|c| serde_json::from_str(c).ok());

        // Parse query if present
        let query: Option<serde_json::Value> = query_json
            .as_ref()
            .and_then(|q| serde_json::from_str(q).ok());

        let exec_outcome = match timeout(
            timeout_duration,
            pool_manager.execute_plugin(
                plugin_id.clone(),
                None,              // compiled_code - will be fetched from cache
                Some(script_path), // plugin_path
                params,
                headers,
                shared_socket_path, // Use shared socket path instead of unique one
                http_request_id,
                Some(timeout_duration.as_secs()),
                route,
                config,
                method,
                query,
            ),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                // No need to manually unregister - ExecutionGuard handles it
                return Err(PluginError::ScriptTimeout(timeout_duration.as_secs()));
            }
        };

        // Collect traces only if emit_traces is enabled
        let traces = if let Some(ref mut rx) = traces_rx {
            // Wait for traces with short timeout - they arrive immediately if the plugin used the API
            let trace_timeout = get_trace_timeout().min(timeout_duration);
            match timeout(trace_timeout, rx.recv()).await {
                Ok(Some(traces)) => traces,
                Ok(None) | Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };

        // ExecutionGuard auto-unregisters when traces_rx is dropped

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
        // Use ts-node mode for this test since temp files are outside plugins directory
        std::env::set_var("PLUGIN_USE_POOL", "false");

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
                None,
                None,
                None,
                None,
                false, // emit_traces
                Arc::new(web::ThinData(state)),
            )
            .await;

        // Cleanup env var
        std::env::remove_var("PLUGIN_USE_POOL");

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
        // Use ts-node mode for this test since temp files are outside plugins directory
        std::env::set_var("PLUGIN_USE_POOL", "false");

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
                None,
                None,
                None,
                None,
                false, // emit_traces
                Arc::new(web::ThinData(state)),
            )
            .await;

        // Cleanup env var
        std::env::remove_var("PLUGIN_USE_POOL");

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
