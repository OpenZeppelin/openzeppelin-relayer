use std::sync::Arc;

use crate::services::plugins::{RelayerApi, ScriptExecutor, ScriptResult, SocketService};
use crate::{jobs::JobProducerTrait, models::AppState};

use super::PluginError;
use actix_web::web;
use async_trait::async_trait;
use tokio::sync::oneshot;

#[cfg(test)]
use mockall::automock;

#[cfg_attr(test, automock)]
#[async_trait]
pub trait PluginRunnerTrait {
    async fn run<J: JobProducerTrait + 'static>(
        &self,
        socket_path: &str,
        script_path: String,
        state: Arc<web::ThinData<AppState<J>>>,
    ) -> Result<ScriptResult, PluginError>;
}

#[derive(Default)]
pub struct PluginRunner;

impl PluginRunner {
    async fn run<J: JobProducerTrait + 'static>(
        &self,
        socket_path: &str,
        script_path: String,
        state: Arc<web::ThinData<AppState<J>>>,
    ) -> Result<ScriptResult, PluginError> {
        let socket_service = SocketService::new(socket_path)?;
        let socket_path_clone = socket_service.socket_path().to_string();

        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let server_handle = tokio::spawn(async move {
            let relayer_api = Arc::new(RelayerApi);
            if let Err(e) = socket_service.listen(shutdown_rx, state, relayer_api).await {
                eprintln!("Socket service error: {}", e);
            }
        });

        let script_result =
            ScriptExecutor::execute_typescript(script_path, socket_path_clone).await?;

        let _ = shutdown_tx.send(());
        let _ = server_handle.await;

        Ok(script_result)
    }
}

#[async_trait]
impl PluginRunnerTrait for PluginRunner {
    async fn run<J: JobProducerTrait + 'static>(
        &self,
        socket_path: &str,
        script_path: String,
        state: Arc<web::ThinData<AppState<J>>>,
    ) -> Result<ScriptResult, PluginError> {
        self.run(socket_path, script_path, state).await
    }
}
