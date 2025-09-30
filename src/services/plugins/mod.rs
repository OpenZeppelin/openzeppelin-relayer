//! Plugins service module for handling plugins execution and interaction with relayer

use std::sync::Arc;

use crate::observability::request_id::get_request_id;
use crate::{
    jobs::JobProducerTrait,
    models::{
        AppState, NetworkRepoModel, NotificationRepoModel, PluginCallRequest, PluginModel,
        RelayerRepoModel, SignerRepoModel, ThinDataAppState, TransactionRepoModel,
    },
    repositories::{
        ApiKeyRepositoryTrait, NetworkRepository, PluginRepositoryTrait, RelayerRepository,
        Repository, TransactionCounterTrait, TransactionRepository,
    },
};
use actix_web::web;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub mod runner;
pub use runner::*;

pub mod relayer_api;
pub use relayer_api::*;

pub mod script_executor;
pub use script_executor::*;

pub mod socket;
pub use socket::*;

#[cfg(test)]
use mockall::automock;

#[derive(Error, Debug, Serialize)]
pub enum PluginError {
    #[error("Socket error: {0}")]
    SocketError(String),
    #[error("Plugin error: {0}")]
    PluginError(String),
    #[error("Relayer error: {0}")]
    RelayerError(String),
    #[error("Plugin execution error: {0}")]
    PluginExecutionError(String),
    #[error("Script execution timed out after {0} seconds")]
    ScriptTimeout(u64),
    #[error("Invalid method: {0}")]
    InvalidMethod(String),
    #[error("Invalid payload: {0}")]
    InvalidPayload(String),
    #[error("{message}")]
    HandlerError {
        message: String,
        status: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        logs: Option<Vec<LogEntry>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        traces: Option<Vec<serde_json::Value>>,
    },
}

impl From<PluginError> for String {
    fn from(error: PluginError) -> Self {
        error.to_string()
    }
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PluginCallResponse {
    /// The plugin result, parsed as JSON when possible; otherwise a string
    pub result: serde_json::Value,
    /// Optional logs captured during plugin execution
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs: Option<Vec<LogEntry>>,
    /// Optional traces captured during plugin execution
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traces: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PluginHandlerError {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs: Option<Vec<LogEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traces: Option<Vec<serde_json::Value>>,
}

#[derive(Debug)]
pub struct HandlerErrorParts {
    pub message: String,
    pub status: u16,
    pub code: Option<String>,
    pub details: Option<serde_json::Value>,
    pub logs: Option<Vec<LogEntry>>,
    pub traces: Option<Vec<serde_json::Value>>,
}

impl HandlerErrorParts {
    pub fn derived_message(&self) -> String {
        if !self.message.trim().is_empty() {
            return self.message.clone();
        }

        if let Some(logs) = &self.logs {
            if let Some(entry) = logs
                .iter()
                .rev()
                .find(|entry| matches!(entry.level, LogLevel::Error | LogLevel::Warn))
            {
                return entry.message.clone();
            }

            if let Some(entry) = logs.last() {
                return entry.message.clone();
            }
        }

        "Plugin execution failed".to_string()
    }

    pub fn with_visibility(mut self, emit_logs: bool, emit_traces: bool) -> Self {
        if !emit_logs {
            self.logs = None;
        }

        if !emit_traces {
            self.traces = None;
        }

        self
    }
}

pub fn split_handler_error(error: PluginError) -> Result<HandlerErrorParts, PluginError> {
    match error {
        PluginError::HandlerError {
            message,
            status,
            code,
            details,
            logs,
            traces,
        } => Ok(HandlerErrorParts {
            message,
            status,
            code,
            details,
            logs,
            traces,
        }),
        other => Err(other),
    }
}

impl From<HandlerErrorParts> for PluginError {
    fn from(parts: HandlerErrorParts) -> Self {
        let HandlerErrorParts {
            message,
            status,
            code,
            details,
            logs,
            traces,
        } = parts;

        PluginError::HandlerError {
            message,
            status,
            code,
            details,
            logs,
            traces,
        }
    }
}

#[derive(Default)]
pub struct PluginService<R: PluginRunnerTrait> {
    runner: R,
}

impl<R: PluginRunnerTrait> PluginService<R> {
    pub fn new(runner: R) -> Self {
        Self { runner }
    }

    fn resolve_plugin_path(plugin_path: &str) -> String {
        if plugin_path.starts_with("plugins/") {
            plugin_path.to_string()
        } else {
            format!("plugins/{}", plugin_path)
        }
    }

    #[allow(clippy::type_complexity)]
    async fn call_plugin<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
        &self,
        plugin: PluginModel,
        plugin_call_request: PluginCallRequest,
        state: Arc<ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
    ) -> Result<PluginCallResponse, PluginError>
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
        let socket_path = format!("/tmp/{}.sock", Uuid::new_v4());
        let script_path = Self::resolve_plugin_path(&plugin.path);
        let script_params = plugin_call_request.params.to_string();

        let result = self
            .runner
            .run(
                plugin.id.clone(),
                &socket_path,
                script_path,
                plugin.timeout,
                script_params,
                get_request_id(),
                state,
            )
            .await;

        match result {
            Ok(script_result) => {
                // Include logs/traces only if enabled via plugin config
                let logs = if plugin.emit_logs {
                    Some(script_result.logs)
                } else {
                    None
                };
                let traces = if plugin.emit_traces {
                    Some(script_result.trace)
                } else {
                    None
                };

                // Parse return_value string into JSON when possible; otherwise string
                let result = if script_result.return_value.trim() == "undefined" {
                    serde_json::Value::Null
                } else {
                    serde_json::from_str::<serde_json::Value>(&script_result.return_value)
                        .unwrap_or(serde_json::Value::String(script_result.return_value))
                };

                Ok(PluginCallResponse {
                    result,
                    logs,
                    traces,
                })
            }
            Err(e) => {
                match split_handler_error(e) {
                    Ok(parts) => {
                        // Log full context before applying visibility filters
                        let log_count = parts.logs.as_ref().map(|logs| logs.len()).unwrap_or(0);
                        let trace_count = parts
                            .traces
                            .as_ref()
                            .map(|traces| traces.len())
                            .unwrap_or(0);
                        tracing::debug!(
                            status = parts.status,
                            message = %parts.message,
                            code = ?parts.code,
                            details = ?parts.details,
                            log_count,
                            trace_count,
                            "Plugin handler returned error"
                        );

                        let filtered = parts.with_visibility(plugin.emit_logs, plugin.emit_traces);
                        Err(filtered.into())
                    }
                    Err(other) => {
                        // This is an actual execution/infrastructure failure
                        tracing::error!("Plugin execution failed: {:?}", other);
                        Err(other)
                    }
                }
            }
        }
    }
}

#[async_trait]
#[cfg_attr(test, automock)]
pub trait PluginServiceTrait<J, TR, RR, NR, NFR, SR, TCR, PR, AKR>: Send + Sync
where
    J: JobProducerTrait + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
    AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
{
    fn new(runner: PluginRunner) -> Self;
    async fn call_plugin(
        &self,
        plugin: PluginModel,
        plugin_call_request: PluginCallRequest,
        state: Arc<web::ThinData<AppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>>,
    ) -> Result<PluginCallResponse, PluginError>;
}

#[async_trait]
impl<J, TR, RR, NR, NFR, SR, TCR, PR, AKR> PluginServiceTrait<J, TR, RR, NR, NFR, SR, TCR, PR, AKR>
    for PluginService<PluginRunner>
where
    J: JobProducerTrait + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
    AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
{
    fn new(runner: PluginRunner) -> Self {
        Self::new(runner)
    }

    async fn call_plugin(
        &self,
        plugin: PluginModel,
        plugin_call_request: PluginCallRequest,
        state: Arc<web::ThinData<AppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>>,
    ) -> Result<PluginCallResponse, PluginError> {
        self.call_plugin(plugin, plugin_call_request, state).await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::{
        constants::DEFAULT_PLUGIN_TIMEOUT_SECONDS,
        jobs::MockJobProducerTrait,
        models::PluginModel,
        repositories::{
            ApiKeyRepositoryStorage, NetworkRepositoryStorage, NotificationRepositoryStorage,
            PluginRepositoryStorage, RelayerRepositoryStorage, SignerRepositoryStorage,
            TransactionCounterRepositoryStorage, TransactionRepositoryStorage,
        },
        utils::mocks::mockutils::create_mock_app_state,
    };

    use super::*;

    #[test]
    fn test_resolve_plugin_path() {
        assert_eq!(
            PluginService::<MockPluginRunnerTrait>::resolve_plugin_path("plugins/examples/test.ts"),
            "plugins/examples/test.ts"
        );

        assert_eq!(
            PluginService::<MockPluginRunnerTrait>::resolve_plugin_path("examples/test.ts"),
            "plugins/examples/test.ts"
        );

        assert_eq!(
            PluginService::<MockPluginRunnerTrait>::resolve_plugin_path("test.ts"),
            "plugins/test.ts"
        );
    }

    #[tokio::test]
    async fn test_call_plugin() {
        let plugin = PluginModel {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
            timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS),
            emit_logs: true,
            emit_traces: false,
            legacy_payload: false,
        };
        let app_state =
            create_mock_app_state(None, None, None, None, Some(vec![plugin.clone()]), None).await;

        let mut plugin_runner = MockPluginRunnerTrait::default();

        plugin_runner
            .expect_run::<MockJobProducerTrait, RelayerRepositoryStorage, TransactionRepositoryStorage, NetworkRepositoryStorage, NotificationRepositoryStorage, SignerRepositoryStorage, TransactionCounterRepositoryStorage, PluginRepositoryStorage, ApiKeyRepositoryStorage>()
            .returning(|_, _, _, _, _, _, _| {
                Ok(ScriptResult {
                    logs: vec![LogEntry {
                        level: LogLevel::Log,
                        message: "test-log".to_string(),
                    }],
                    error: "test-error".to_string(),
                    return_value: "test-result".to_string(),
                    trace: Vec::new(),
                })
            });

        let plugin_service = PluginService::<MockPluginRunnerTrait>::new(plugin_runner);
        let result = plugin_service
            .call_plugin(
                plugin,
                PluginCallRequest {
                    params: serde_json::Value::Null,
                },
                Arc::new(web::ThinData(app_state)),
            )
            .await;
        assert!(result.is_ok());
        let result = result.unwrap();
        // result should be the string since it is not JSON
        assert_eq!(
            result.result,
            serde_json::Value::String("test-result".to_string())
        );
        // emit_logs=true -> logs should be present
        assert!(result.logs.is_some());
    }

    #[tokio::test]
    async fn test_from_plugin_error_to_string() {
        let error = PluginError::PluginExecutionError("test-error".to_string());
        let result: String = error.into();
        assert_eq!(result, "Plugin execution error: test-error");
    }
}
