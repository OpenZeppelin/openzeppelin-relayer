use std::sync::Arc;

use crate::services::plugins::{ScriptExecutor, ScriptResult, SocketService};
use crate::{jobs::JobProducerTrait, models::AppState};

use super::PluginError;
use actix_web::web;
use tokio::sync::oneshot;

pub struct PluginRunner;

impl PluginRunner {
    pub async fn run<J: JobProducerTrait + 'static>(
        socket_path: &str,
        script_path: String,
        state: Arc<web::ThinData<AppState<J>>>,
    ) -> Result<ScriptResult, PluginError> {
        // Create socket service
        let socket_service = SocketService::new(socket_path)?;
        let socket_path_clone = socket_service.socket_path().to_string();

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        // Start socket listener in background
        let server_handle = tokio::spawn(async move {
            if let Err(e) = socket_service.listen(shutdown_rx, state).await {
                eprintln!("Socket service error: {}", e);
            }
        });

        // Execute the TypeScript script
        let script_result =
            ScriptExecutor::execute_typescript(script_path, socket_path_clone).await?;

        // Signal shutdown and wait for server to finish
        let _ = shutdown_tx.send(());
        let _ = server_handle.await;

        Ok(script_result)
    }
}
