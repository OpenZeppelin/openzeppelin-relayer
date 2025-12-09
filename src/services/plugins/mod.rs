//! Plugins service module for handling plugins execution and interaction with relayer

use std::{fmt, sync::Arc};

use crate::observability::request_id::get_request_id;
use crate::{
    jobs::JobProducerTrait,
    models::{
        AppState, NetworkRepoModel, NotificationRepoModel, PluginCallRequest, PluginMetadata,
        PluginModel, RelayerRepoModel, SignerRepoModel, ThinDataAppState, TransactionRepoModel,
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
    #[error("{0}")]
    HandlerError(Box<PluginHandlerPayload>),
}

impl PluginError {
    /// Enriches the error with traces if it's a HandlerError variant.
    /// For other variants, returns the error unchanged.
    pub fn with_traces(self, traces: Vec<serde_json::Value>) -> Self {
        match self {
            PluginError::HandlerError(mut payload) => {
                payload.append_traces(traces);
                PluginError::HandlerError(payload)
            }
            other => other,
        }
    }
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
    /// Optional metadata captured during plugin execution (logs/traces)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<PluginMetadata>,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PluginHandlerError {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug)]
pub struct PluginHandlerResponse {
    pub status: u16,
    pub message: String,
    pub error: PluginHandlerError,
    pub metadata: Option<PluginMetadata>,
}

#[derive(Debug, Serialize)]
pub struct PluginHandlerPayload {
    pub status: u16,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs: Option<Vec<LogEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traces: Option<Vec<serde_json::Value>>,
}

impl PluginHandlerPayload {
    fn append_traces(&mut self, traces: Vec<serde_json::Value>) {
        match &mut self.traces {
            Some(existing) => existing.extend(traces),
            None => self.traces = Some(traces),
        }
    }

    fn into_response(self, emit_logs: bool, emit_traces: bool) -> PluginHandlerResponse {
        let logs = if emit_logs { self.logs } else { None };
        let traces = if emit_traces { self.traces } else { None };
        let message = derive_handler_message(&self.message, logs.as_deref());
        let metadata = build_metadata(logs, traces);

        PluginHandlerResponse {
            status: self.status,
            message,
            error: PluginHandlerError {
                code: self.code,
                details: self.details,
            },
            metadata,
        }
    }
}

impl fmt::Display for PluginHandlerPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

fn derive_handler_message(message: &str, logs: Option<&[LogEntry]>) -> String {
    if !message.trim().is_empty() {
        return message.to_string();
    }

    if let Some(logs) = logs {
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

fn build_metadata(
    logs: Option<Vec<LogEntry>>,
    traces: Option<Vec<serde_json::Value>>,
) -> Option<PluginMetadata> {
    if logs.is_some() || traces.is_some() {
        Some(PluginMetadata { logs, traces })
    } else {
        None
    }
}

#[derive(Debug)]
pub enum PluginCallResult {
    Success(PluginCallResponse),
    Handler(PluginHandlerResponse),
    Fatal(PluginError),
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
            format!("plugins/{plugin_path}")
        }
    }

    #[allow(clippy::type_complexity)]
    async fn call_plugin<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
        &self,
        plugin: PluginModel,
        plugin_call_request: PluginCallRequest,
        state: Arc<ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
    ) -> PluginCallResult
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
        let headers_json = plugin_call_request
            .headers
            .map(|h| serde_json::to_string(&h).unwrap_or_default());

        let result = self
            .runner
            .run(
                plugin.id.clone(),
                &socket_path,
                script_path,
                plugin.timeout,
                script_params,
                get_request_id(),
                headers_json,
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
                let metadata = build_metadata(logs, traces);

                // Parse return_value string into JSON when possible; otherwise string
                let result = if script_result.return_value.trim() == "undefined" {
                    serde_json::Value::Null
                } else {
                    serde_json::from_str::<serde_json::Value>(&script_result.return_value)
                        .unwrap_or(serde_json::Value::String(script_result.return_value))
                };

                PluginCallResult::Success(PluginCallResponse { result, metadata })
            }
            Err(e) => match e {
                PluginError::HandlerError(payload) => {
                    let failure = payload.into_response(plugin.emit_logs, plugin.emit_traces);
                    let has_logs = failure
                        .metadata
                        .as_ref()
                        .and_then(|meta| meta.logs.as_ref())
                        .is_some();
                    let has_traces = failure
                        .metadata
                        .as_ref()
                        .and_then(|meta| meta.traces.as_ref())
                        .is_some();

                    tracing::debug!(
                        status = failure.status,
                        message = %failure.message,
                        code = ?failure.error.code.as_ref(),
                        details = ?failure.error.details.as_ref(),
                        has_logs,
                        has_traces,
                        "Plugin handler returned error"
                    );

                    PluginCallResult::Handler(failure)
                }
                other => {
                    // This is an actual execution/infrastructure failure
                    tracing::error!("Plugin execution failed: {:?}", other);
                    PluginCallResult::Fatal(other)
                }
            },
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
    ) -> PluginCallResult;
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
    ) -> PluginCallResult {
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
        };
        let app_state =
            create_mock_app_state(None, None, None, None, Some(vec![plugin.clone()]), None).await;

        let mut plugin_runner = MockPluginRunnerTrait::default();

        plugin_runner
            .expect_run::<MockJobProducerTrait, RelayerRepositoryStorage, TransactionRepositoryStorage, NetworkRepositoryStorage, NotificationRepositoryStorage, SignerRepositoryStorage, TransactionCounterRepositoryStorage, PluginRepositoryStorage, ApiKeyRepositoryStorage>()
            .returning(|_, _, _, _, _, _, _, _| {
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
        let outcome = plugin_service
            .call_plugin(
                plugin,
                PluginCallRequest {
                    params: serde_json::Value::Null,
                    headers: None,
                },
                Arc::new(web::ThinData(app_state)),
            )
            .await;
        match outcome {
            PluginCallResult::Success(result) => {
                // result should be the string since it is not JSON
                assert_eq!(
                    result.result,
                    serde_json::Value::String("test-result".to_string())
                );
                // emit_logs=true -> logs should be present in metadata
                assert!(result.metadata.and_then(|meta| meta.logs).is_some());
            }
            PluginCallResult::Handler(_) | PluginCallResult::Fatal(_) => {
                panic!("expected success outcome")
            }
        }
    }

    #[tokio::test]
    async fn test_from_plugin_error_to_string() {
        let error = PluginError::PluginExecutionError("test-error".to_string());
        let result: String = error.into();
        assert_eq!(result, "Plugin execution error: test-error");
    }

    #[test]
    fn test_plugin_error_with_traces_handler_error() {
        let payload = PluginHandlerPayload {
            status: 400,
            message: "test message".to_string(),
            code: Some("TEST_CODE".to_string()),
            details: None,
            logs: None,
            traces: Some(vec![serde_json::json!({"trace": "1"})]),
        };
        let error = PluginError::HandlerError(Box::new(payload));
        let new_traces = vec![
            serde_json::json!({"trace": "2"}),
            serde_json::json!({"trace": "3"}),
        ];

        let enriched_error = error.with_traces(new_traces);

        match enriched_error {
            PluginError::HandlerError(payload) => {
                let traces = payload.traces.unwrap();
                assert_eq!(traces.len(), 3);
                assert_eq!(traces[0], serde_json::json!({"trace": "1"}));
                assert_eq!(traces[1], serde_json::json!({"trace": "2"}));
                assert_eq!(traces[2], serde_json::json!({"trace": "3"}));
            }
            _ => panic!("Expected HandlerError variant"),
        }
    }

    #[test]
    fn test_plugin_error_with_traces_other_variants() {
        let error = PluginError::PluginExecutionError("test".to_string());
        let new_traces = vec![serde_json::json!({"trace": "1"})];

        let result = error.with_traces(new_traces);

        match result {
            PluginError::PluginExecutionError(msg) => assert_eq!(msg, "test"),
            _ => panic!("Expected PluginExecutionError variant"),
        }
    }

    #[test]
    fn test_derive_handler_message_with_message() {
        let result = derive_handler_message("Custom error message", None);
        assert_eq!(result, "Custom error message");
    }

    #[test]
    fn test_derive_handler_message_with_error_log() {
        let logs = vec![
            LogEntry {
                level: LogLevel::Log,
                message: "info log".to_string(),
            },
            LogEntry {
                level: LogLevel::Error,
                message: "error log".to_string(),
            },
        ];
        let result = derive_handler_message("", Some(&logs));
        assert_eq!(result, "error log");
    }

    #[test]
    fn test_derive_handler_message_with_warn_log() {
        let logs = vec![
            LogEntry {
                level: LogLevel::Log,
                message: "info log".to_string(),
            },
            LogEntry {
                level: LogLevel::Warn,
                message: "warn log".to_string(),
            },
        ];
        let result = derive_handler_message("", Some(&logs));
        assert_eq!(result, "warn log");
    }

    #[test]
    fn test_derive_handler_message_with_only_info_logs() {
        let logs = vec![
            LogEntry {
                level: LogLevel::Log,
                message: "first log".to_string(),
            },
            LogEntry {
                level: LogLevel::Info,
                message: "last log".to_string(),
            },
        ];
        let result = derive_handler_message("", Some(&logs));
        assert_eq!(result, "last log");
    }

    #[test]
    fn test_derive_handler_message_no_logs() {
        let result = derive_handler_message("", None);
        assert_eq!(result, "Plugin execution failed");
    }

    #[test]
    fn test_build_metadata_with_logs_and_traces() {
        let logs = vec![LogEntry {
            level: LogLevel::Log,
            message: "test".to_string(),
        }];
        let traces = vec![serde_json::json!({"trace": "1"})];

        let result = build_metadata(Some(logs.clone()), Some(traces.clone()));

        assert!(result.is_some());
        let metadata = result.unwrap();
        assert_eq!(metadata.logs.unwrap(), logs);
        assert_eq!(metadata.traces.unwrap(), traces);
    }

    #[test]
    fn test_build_metadata_with_only_logs() {
        let logs = vec![LogEntry {
            level: LogLevel::Log,
            message: "test".to_string(),
        }];

        let result = build_metadata(Some(logs.clone()), None);

        assert!(result.is_some());
        let metadata = result.unwrap();
        assert_eq!(metadata.logs.unwrap(), logs);
        assert!(metadata.traces.is_none());
    }

    #[test]
    fn test_build_metadata_with_only_traces() {
        let traces = vec![serde_json::json!({"trace": "1"})];

        let result = build_metadata(None, Some(traces.clone()));

        assert!(result.is_some());
        let metadata = result.unwrap();
        assert!(metadata.logs.is_none());
        assert_eq!(metadata.traces.unwrap(), traces);
    }

    #[test]
    fn test_build_metadata_with_neither() {
        let result = build_metadata(None, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_plugin_handler_payload_append_traces_to_existing() {
        let mut payload = PluginHandlerPayload {
            status: 400,
            message: "test".to_string(),
            code: None,
            details: None,
            logs: None,
            traces: Some(vec![serde_json::json!({"trace": "1"})]),
        };

        payload.append_traces(vec![serde_json::json!({"trace": "2"})]);

        let traces = payload.traces.unwrap();
        assert_eq!(traces.len(), 2);
        assert_eq!(traces[0], serde_json::json!({"trace": "1"}));
        assert_eq!(traces[1], serde_json::json!({"trace": "2"}));
    }

    #[test]
    fn test_plugin_handler_payload_append_traces_to_none() {
        let mut payload = PluginHandlerPayload {
            status: 400,
            message: "test".to_string(),
            code: None,
            details: None,
            logs: None,
            traces: None,
        };

        payload.append_traces(vec![serde_json::json!({"trace": "1"})]);

        let traces = payload.traces.unwrap();
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0], serde_json::json!({"trace": "1"}));
    }

    #[test]
    fn test_plugin_handler_payload_into_response_with_logs_and_traces() {
        let logs = vec![LogEntry {
            level: LogLevel::Error,
            message: "error message".to_string(),
        }];
        let payload = PluginHandlerPayload {
            status: 400,
            message: "".to_string(),
            code: Some("ERR_CODE".to_string()),
            details: Some(serde_json::json!({"key": "value"})),
            logs: Some(logs.clone()),
            traces: Some(vec![serde_json::json!({"trace": "1"})]),
        };

        let response = payload.into_response(true, true);

        assert_eq!(response.status, 400);
        assert_eq!(response.message, "error message"); // Derived from error log
        assert_eq!(response.error.code, Some("ERR_CODE".to_string()));
        assert!(response.metadata.is_some());
        let metadata = response.metadata.unwrap();
        assert_eq!(metadata.logs.unwrap(), logs);
        assert_eq!(metadata.traces.unwrap().len(), 1);
    }

    #[test]
    fn test_plugin_handler_payload_into_response_without_logs() {
        let logs = vec![LogEntry {
            level: LogLevel::Log,
            message: "test log".to_string(),
        }];
        let payload = PluginHandlerPayload {
            status: 500,
            message: "explicit message".to_string(),
            code: None,
            details: None,
            logs: Some(logs),
            traces: None,
        };

        let response = payload.into_response(false, false);

        assert_eq!(response.status, 500);
        assert_eq!(response.message, "explicit message");
        assert!(response.metadata.is_none()); // emit_logs=false, emit_traces=false
    }

    #[tokio::test]
    async fn test_call_plugin_handler_error() {
        let plugin = PluginModel {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
            timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS),
            emit_logs: true,
            emit_traces: true,
        };
        let app_state =
            create_mock_app_state(None, None, None, None, Some(vec![plugin.clone()]), None).await;

        let mut plugin_runner = MockPluginRunnerTrait::default();

        plugin_runner
            .expect_run::<MockJobProducerTrait, RelayerRepositoryStorage, TransactionRepositoryStorage, NetworkRepositoryStorage, NotificationRepositoryStorage, SignerRepositoryStorage, TransactionCounterRepositoryStorage, PluginRepositoryStorage, ApiKeyRepositoryStorage>()
            .returning(move |_, _, _, _, _, _, _, _| {
                Err(PluginError::HandlerError(Box::new(PluginHandlerPayload {
                    status: 400,
                    message: "Plugin handler error".to_string(),
                    code: Some("VALIDATION_ERROR".to_string()),
                    details: Some(serde_json::json!({"field": "email"})),
                    logs: Some(vec![LogEntry {
                        level: LogLevel::Error,
                        message: "Invalid email".to_string(),
                    }]),
                    traces: Some(vec![serde_json::json!({"step": "validation"})]),
                })))
            });

        let plugin_service = PluginService::<MockPluginRunnerTrait>::new(plugin_runner);
        let outcome = plugin_service
            .call_plugin(
                plugin,
                PluginCallRequest {
                    params: serde_json::Value::Null,
                    headers: None,
                },
                Arc::new(web::ThinData(app_state)),
            )
            .await;

        match outcome {
            PluginCallResult::Handler(response) => {
                assert_eq!(response.status, 400);
                assert_eq!(response.error.code, Some("VALIDATION_ERROR".to_string()));
                assert!(response.metadata.is_some());
                let metadata = response.metadata.unwrap();
                assert!(metadata.logs.is_some());
                assert!(metadata.traces.is_some());
            }
            _ => panic!("Expected Handler result"),
        }
    }

    #[tokio::test]
    async fn test_call_plugin_fatal_error() {
        let plugin = PluginModel {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
            timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS),
            emit_logs: false,
            emit_traces: false,
        };
        let app_state =
            create_mock_app_state(None, None, None, None, Some(vec![plugin.clone()]), None).await;

        let mut plugin_runner = MockPluginRunnerTrait::default();

        plugin_runner
            .expect_run::<MockJobProducerTrait, RelayerRepositoryStorage, TransactionRepositoryStorage, NetworkRepositoryStorage, NotificationRepositoryStorage, SignerRepositoryStorage, TransactionCounterRepositoryStorage, PluginRepositoryStorage, ApiKeyRepositoryStorage>()
            .returning(|_, _, _, _, _, _, _, _| {
                Err(PluginError::PluginExecutionError("Fatal error".to_string()))
            });

        let plugin_service = PluginService::<MockPluginRunnerTrait>::new(plugin_runner);
        let outcome = plugin_service
            .call_plugin(
                plugin,
                PluginCallRequest {
                    params: serde_json::Value::Null,
                    headers: None,
                },
                Arc::new(web::ThinData(app_state)),
            )
            .await;

        match outcome {
            PluginCallResult::Fatal(error) => {
                assert!(matches!(error, PluginError::PluginExecutionError(_)));
            }
            _ => panic!("Expected Fatal result"),
        }
    }

    #[tokio::test]
    async fn test_call_plugin_success_with_json_result() {
        let plugin = PluginModel {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
            timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS),
            emit_logs: true,
            emit_traces: true,
        };
        let app_state =
            create_mock_app_state(None, None, None, None, Some(vec![plugin.clone()]), None).await;

        let mut plugin_runner = MockPluginRunnerTrait::default();

        plugin_runner
            .expect_run::<MockJobProducerTrait, RelayerRepositoryStorage, TransactionRepositoryStorage, NetworkRepositoryStorage, NotificationRepositoryStorage, SignerRepositoryStorage, TransactionCounterRepositoryStorage, PluginRepositoryStorage, ApiKeyRepositoryStorage>()
            .returning(|_, _, _, _, _, _, _, _| {
                Ok(ScriptResult {
                    logs: vec![LogEntry {
                        level: LogLevel::Log,
                        message: "test-log".to_string(),
                    }],
                    error: "".to_string(),
                    return_value: r#"{"result": "success"}"#.to_string(),
                    trace: vec![serde_json::json!({"step": "1"})],
                })
            });

        let plugin_service = PluginService::<MockPluginRunnerTrait>::new(plugin_runner);
        let outcome = plugin_service
            .call_plugin(
                plugin,
                PluginCallRequest {
                    params: serde_json::Value::Null,
                    headers: None,
                },
                Arc::new(web::ThinData(app_state)),
            )
            .await;

        match outcome {
            PluginCallResult::Success(result) => {
                // Should be parsed as JSON object
                assert_eq!(result.result, serde_json::json!({"result": "success"}));
                assert!(result.metadata.is_some());
                let metadata = result.metadata.unwrap();
                assert!(metadata.logs.is_some());
                assert!(metadata.traces.is_some());
            }
            _ => panic!("Expected Success result"),
        }
    }

    #[tokio::test]
    async fn test_call_plugin_success_with_undefined_result() {
        let plugin = PluginModel {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
            timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS),
            emit_logs: false,
            emit_traces: false,
        };
        let app_state =
            create_mock_app_state(None, None, None, None, Some(vec![plugin.clone()]), None).await;

        let mut plugin_runner = MockPluginRunnerTrait::default();

        plugin_runner
            .expect_run::<MockJobProducerTrait, RelayerRepositoryStorage, TransactionRepositoryStorage, NetworkRepositoryStorage, NotificationRepositoryStorage, SignerRepositoryStorage, TransactionCounterRepositoryStorage, PluginRepositoryStorage, ApiKeyRepositoryStorage>()
            .returning(|_, _, _, _, _, _, _, _| {
                Ok(ScriptResult {
                    logs: vec![],
                    error: "".to_string(),
                    return_value: "undefined".to_string(),
                    trace: vec![],
                })
            });

        let plugin_service = PluginService::<MockPluginRunnerTrait>::new(plugin_runner);
        let outcome = plugin_service
            .call_plugin(
                plugin,
                PluginCallRequest {
                    params: serde_json::Value::Null,
                    headers: None,
                },
                Arc::new(web::ThinData(app_state)),
            )
            .await;

        match outcome {
            PluginCallResult::Success(result) => {
                // "undefined" should be converted to null
                assert_eq!(result.result, serde_json::Value::Null);
                // emit_logs=false, emit_traces=false -> no metadata
                assert!(result.metadata.is_none());
            }
            _ => panic!("Expected Success result"),
        }
    }

    #[tokio::test]
    async fn test_call_plugin_with_headers() {
        use std::sync::{Arc as StdArc, Mutex};

        let plugin = PluginModel {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
            timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS),
            emit_logs: false,
            emit_traces: false,
        };
        let app_state =
            create_mock_app_state(None, None, None, None, Some(vec![plugin.clone()]), None).await;

        // Capture the headers_json parameter passed to the runner
        let captured_headers: StdArc<Mutex<Option<String>>> = StdArc::new(Mutex::new(None));
        let captured_headers_clone = captured_headers.clone();

        let mut plugin_runner = MockPluginRunnerTrait::default();

        plugin_runner
            .expect_run::<MockJobProducerTrait, RelayerRepositoryStorage, TransactionRepositoryStorage, NetworkRepositoryStorage, NotificationRepositoryStorage, SignerRepositoryStorage, TransactionCounterRepositoryStorage, PluginRepositoryStorage, ApiKeyRepositoryStorage>()
            .returning(move |_, _, _, _, _, _, headers_json, _| {
                // Capture the headers_json parameter
                *captured_headers_clone.lock().unwrap() = headers_json;
                Ok(ScriptResult {
                    logs: vec![],
                    error: "".to_string(),
                    return_value: "{}".to_string(),
                    trace: vec![],
                })
            });

        // Create request with headers
        let mut headers_map = std::collections::HashMap::new();
        headers_map.insert(
            "x-custom-header".to_string(),
            vec!["custom-value".to_string()],
        );
        headers_map.insert(
            "authorization".to_string(),
            vec!["Bearer token123".to_string()],
        );

        let plugin_service = PluginService::<MockPluginRunnerTrait>::new(plugin_runner);
        let _outcome = plugin_service
            .call_plugin(
                plugin,
                PluginCallRequest {
                    params: serde_json::json!({"test": "data"}),
                    headers: Some(headers_map.clone()),
                },
                Arc::new(web::ThinData(app_state)),
            )
            .await;

        // Verify headers were serialized and passed to the runner
        let captured = captured_headers.lock().unwrap();
        assert!(
            captured.is_some(),
            "headers_json should be passed to runner"
        );

        let headers_json = captured.as_ref().unwrap();
        let parsed: std::collections::HashMap<String, Vec<String>> =
            serde_json::from_str(headers_json).expect("headers_json should be valid JSON");

        assert_eq!(
            parsed.get("x-custom-header"),
            Some(&vec!["custom-value".to_string()])
        );
        assert_eq!(
            parsed.get("authorization"),
            Some(&vec!["Bearer token123".to_string()])
        );
    }

    #[tokio::test]
    async fn test_call_plugin_without_headers() {
        use std::sync::{Arc as StdArc, Mutex};

        let plugin = PluginModel {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
            timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS),
            emit_logs: false,
            emit_traces: false,
        };
        let app_state =
            create_mock_app_state(None, None, None, None, Some(vec![plugin.clone()]), None).await;

        let captured_headers: StdArc<Mutex<Option<String>>> = StdArc::new(Mutex::new(None));
        let captured_headers_clone = captured_headers.clone();

        let mut plugin_runner = MockPluginRunnerTrait::default();

        plugin_runner
            .expect_run::<MockJobProducerTrait, RelayerRepositoryStorage, TransactionRepositoryStorage, NetworkRepositoryStorage, NotificationRepositoryStorage, SignerRepositoryStorage, TransactionCounterRepositoryStorage, PluginRepositoryStorage, ApiKeyRepositoryStorage>()
            .returning(move |_, _, _, _, _, _, headers_json, _| {
                *captured_headers_clone.lock().unwrap() = headers_json;
                Ok(ScriptResult {
                    logs: vec![],
                    error: "".to_string(),
                    return_value: "{}".to_string(),
                    trace: vec![],
                })
            });

        let plugin_service = PluginService::<MockPluginRunnerTrait>::new(plugin_runner);
        let _outcome = plugin_service
            .call_plugin(
                plugin,
                PluginCallRequest {
                    params: serde_json::json!({}),
                    headers: None, // No headers
                },
                Arc::new(web::ThinData(app_state)),
            )
            .await;

        // Verify headers_json is None when no headers provided
        let captured = captured_headers.lock().unwrap();
        assert!(
            captured.is_none(),
            "headers_json should be None when no headers provided"
        );
    }
}
