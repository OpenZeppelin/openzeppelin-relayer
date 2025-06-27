//! Plugins service module for handling plugins execution and interaction with relayer

use std::sync::Arc;

use crate::{
    jobs::JobProducerTrait,
    models::{AppState, PluginCallRequest},
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
}

impl From<PluginError> for String {
    fn from(error: PluginError) -> Self {
        error.to_string()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PluginCallResponse {
    pub success: bool,
    pub message: String,
    pub output: String,
    pub error: String,
    pub traces: Vec<String>,
}

#[derive(Default)]
pub struct PluginService<R: PluginRunnerTrait> {
    runner: R,
}

impl<R: PluginRunnerTrait> PluginService<R> {
    pub fn new(runner: R) -> Self {
        Self { runner }
    }

    async fn call_plugin<J: JobProducerTrait + 'static>(
        &self,
        code_path: String,
        _plugin_call_request: PluginCallRequest,
        state: Arc<web::ThinData<AppState<J>>>,
    ) -> Result<PluginCallResponse, PluginError> {
        let socket_path = format!("/tmp/{}.sock", Uuid::new_v4());
        let result = self.runner.run(&socket_path, code_path, state).await;

        match result {
            Ok(script_result) => Ok(PluginCallResponse {
                success: true,
                message: "Plugin called successfully".to_string(),
                output: script_result.output,
                error: script_result.error,
                traces: script_result.trace,
            }),
            Err(e) => Err(PluginError::PluginExecutionError(e.to_string())),
        }
    }
}

#[async_trait]
#[cfg_attr(test, automock)]
pub trait PluginServiceTrait<J: JobProducerTrait + 'static>: Send + Sync {
    fn new(runner: PluginRunner) -> Self;
    async fn call_plugin(
        &self,
        code_path: String,
        plugin_call_request: PluginCallRequest,
        state: Arc<web::ThinData<AppState<J>>>,
    ) -> Result<PluginCallResponse, PluginError>;
}

#[async_trait]
impl<J: JobProducerTrait + 'static> PluginServiceTrait<J> for PluginService<PluginRunner> {
    fn new(runner: PluginRunner) -> Self {
        Self::new(runner)
    }

    async fn call_plugin(
        &self,
        code_path: String,
        plugin_call_request: PluginCallRequest,
        state: Arc<web::ThinData<AppState<J>>>,
    ) -> Result<PluginCallResponse, PluginError> {
        self.call_plugin(code_path, plugin_call_request, state)
            .await
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        jobs::MockJobProducerTrait, models::PluginModel,
        utils::mocks::mockutils::create_mock_app_state,
    };

    use super::*;

    #[tokio::test]
    async fn test_call_plugin() {
        let plugin = PluginModel {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
        };
        let app_state: AppState<MockJobProducerTrait> =
            create_mock_app_state(None, None, None, Some(vec![plugin])).await;

        let mut plugin_runner = MockPluginRunnerTrait::default();

        plugin_runner
            .expect_run::<MockJobProducerTrait>()
            .returning(|_, _, _| {
                Ok(ScriptResult {
                    output: "test-output".to_string(),
                    error: "test-error".to_string(),
                    trace: Vec::new(),
                })
            });

        let plugin_service = PluginService::<MockPluginRunnerTrait>::new(plugin_runner);
        let result = plugin_service
            .call_plugin(
                "test-plugin".to_string(),
                PluginCallRequest {
                    params: serde_json::Value::Null,
                },
                Arc::new(web::ThinData(app_state)),
            )
            .await;
        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_from_plugin_error_to_string() {
        let error = PluginError::PluginExecutionError("test-error".to_string());
        let result: String = error.into();
        assert_eq!(result, "Plugin execution error: test-error");
    }
}
