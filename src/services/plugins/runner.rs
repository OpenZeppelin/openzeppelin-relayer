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
            socket_service.listen(shutdown_rx, state, relayer_api).await
        });

        let mut script_result =
            ScriptExecutor::execute_typescript(script_path, socket_path_clone).await?;

        let _ = shutdown_tx.send(());

        let server_handle = server_handle
            .await
            .map_err(|e| PluginError::SocketError(e.to_string()))?;

        match server_handle {
            Ok(traces) => {
                script_result.trace = traces;
            }
            Err(e) => {
                return Err(PluginError::SocketError(e.to_string()));
            }
        }

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

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::{
        jobs::MockJobProducerTrait, services::plugins::LogLevel,
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
            console.log(JSON.stringify({ level: 'log', message: 'test' }));
            console.log(JSON.stringify({ level: 'error', message: 'test-error' }));
            console.log(JSON.stringify({ level: 'result', message: 'test-result' }));
        "#;
        fs::write(script_path.clone(), content).unwrap();
        fs::write(ts_config.clone(), TS_CONFIG.as_bytes()).unwrap();

        let state = create_mock_app_state(None, None, None, None).await;

        let plugin_runner = PluginRunner;
        let result = plugin_runner
            .run::<MockJobProducerTrait>(
                &socket_path.display().to_string(),
                script_path.display().to_string(),
                Arc::new(web::ThinData(state)),
            )
            .await;

        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.logs[0].level, LogLevel::Log);
        assert_eq!(result.logs[0].message, "test");
        assert_eq!(result.logs[1].level, LogLevel::Error);
        assert_eq!(result.logs[1].message, "test-error");
        assert_eq!(result.return_value, "test-result");
    }
}
