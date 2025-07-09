//! Plugins service module for handling plugins execution and interaction with relayer

use std::sync::Arc;

use crate::{
    jobs::JobProducerTrait,
    models::{
        AppState, NetworkRepoModel, NotificationRepoModel, PluginCallRequest, PluginModel,
        RelayerRepoModel, SignerRepoModel, ThinDataAppState, TransactionRepoModel,
    },
    repositories::{
        NetworkRepository, PluginRepositoryTrait, RelayerRepository, Repository,
        TransactionCounterTrait, TransactionRepository,
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
}

impl From<PluginError> for String {
    fn from(error: PluginError) -> Self {
        error.to_string()
    }
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PluginCallResponse {
    pub success: bool,
    pub return_value: String,
    pub message: String,
    pub logs: Vec<LogEntry>,
    pub error: String,
    pub traces: Vec<serde_json::Value>,
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
    async fn call_plugin<J, RR, TR, NR, NFR, SR, TCR, PR>(
        &self,
        plugin: PluginModel,
        plugin_call_request: PluginCallRequest,
        state: Arc<ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>>,
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
    {
        let socket_path = format!("/tmp/{}.sock", Uuid::new_v4());
        let script_path = Self::resolve_plugin_path(&plugin.path);
        let script_params = plugin_call_request.params.to_string();

        let result = self
            .runner
            .run(
                &socket_path,
                script_path,
                plugin.timeout,
                script_params,
                state,
            )
            .await;

        match result {
            Ok(script_result) => Ok(PluginCallResponse {
                success: true,
                message: "Plugin called successfully".to_string(),
                return_value: script_result.return_value,
                logs: script_result.logs,
                error: script_result.error,
                traces: script_result.trace,
            }),
            Err(e) => Err(PluginError::PluginExecutionError(e.to_string())),
        }
    }
}

#[async_trait]
#[cfg_attr(test, automock)]
pub trait PluginServiceTrait<J, TR, RR, NR, NFR, SR, TCR, PR>: Send + Sync
where
    J: JobProducerTrait + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
    fn new(runner: PluginRunner) -> Self;
    async fn call_plugin(
        &self,
        plugin: PluginModel,
        plugin_call_request: PluginCallRequest,
        state: Arc<web::ThinData<AppState<J, RR, TR, NR, NFR, SR, TCR, PR>>>,
    ) -> Result<PluginCallResponse, PluginError>;
}

#[async_trait]
impl<J, TR, RR, NR, NFR, SR, TCR, PR> PluginServiceTrait<J, TR, RR, NR, NFR, SR, TCR, PR>
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
{
    fn new(runner: PluginRunner) -> Self {
        Self::new(runner)
    }

    async fn call_plugin(
        &self,
        plugin: PluginModel,
        plugin_call_request: PluginCallRequest,
        state: Arc<web::ThinData<AppState<J, RR, TR, NR, NFR, SR, TCR, PR>>>,
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
            NetworkRepositoryStorage, NotificationRepositoryStorage, PluginRepositoryStorage,
            RelayerRepositoryStorage, SignerRepositoryStorage, TransactionCounterRepositoryStorage,
            TransactionRepositoryStorage,
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
        };
        let app_state: AppState<
            MockJobProducerTrait,
            RelayerRepositoryStorage,
            TransactionRepositoryStorage,
            NetworkRepositoryStorage,
            NotificationRepositoryStorage,
            SignerRepositoryStorage,
            TransactionCounterRepositoryStorage,
            PluginRepositoryStorage,
        > = create_mock_app_state(None, None, None, Some(vec![plugin.clone()]), None).await;

        let mut plugin_runner = MockPluginRunnerTrait::default();

        plugin_runner
            .expect_run::<MockJobProducerTrait, RelayerRepositoryStorage, TransactionRepositoryStorage, NetworkRepositoryStorage, NotificationRepositoryStorage, SignerRepositoryStorage, TransactionCounterRepositoryStorage, PluginRepositoryStorage>()
            .returning(|_, _, _, _, _| {
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
        assert!(result.success);
        assert_eq!(result.return_value, "test-result");
    }

    #[tokio::test]
    async fn test_from_plugin_error_to_string() {
        let error = PluginError::PluginExecutionError("test-error".to_string());
        let result: String = error.into();
        assert_eq!(result, "Plugin execution error: test-error");
    }
}
