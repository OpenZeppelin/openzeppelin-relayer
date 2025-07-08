//! This module is the orchestrator of the plugin execution.
//!
//! 1. Initiates a socket connection to the relayer server - socket.rs
//! 2. Executes the plugin script - script_executor.rs
//! 3. Sends the shutdown signal to the relayer server - socket.rs
//! 4. Waits for the relayer server to finish the execution - socket.rs
//! 5. Returns the output of the script - script_executor.rs
//!
use std::sync::Arc;

use crate::services::plugins::{RelayerApi, ScriptExecutor, ScriptResult, SocketService};
use crate::{
    jobs::JobProducerTrait,
    models::{
        NetworkRepoModel, NotificationRepoModel, RelayerRepoModel, SignerRepoModel,
        ThinDataAppState, TransactionRepoModel,
    },
    repositories::{
        NetworkRepository, PluginRepositoryTrait, RelayerRepository, Repository,
        TransactionCounterTrait, TransactionRepository,
    },
};

use super::PluginError;
use async_trait::async_trait;
use tokio::sync::oneshot;

#[cfg(test)]
use mockall::automock;

#[cfg_attr(test, automock)]
#[async_trait]
pub trait PluginRunnerTrait {
    #[allow(clippy::type_complexity)]
    async fn run<
        J: JobProducerTrait + 'static,
        TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
        RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
        NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
        NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
        SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
        TCR: TransactionCounterTrait + Send + Sync + 'static,
        PR: PluginRepositoryTrait + Send + Sync + 'static,
    >(
        &self,
        socket_path: &str,
        script_path: String,
        script_params: String,
        state: Arc<ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>>,
    ) -> Result<ScriptResult, PluginError>;
}

#[derive(Default)]
pub struct PluginRunner;

#[allow(clippy::type_complexity)]
impl PluginRunner {
    async fn run<
        J: JobProducerTrait + 'static,
        TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
        RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
        NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
        NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
        SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
        TCR: TransactionCounterTrait + Send + Sync + 'static,
        PR: PluginRepositoryTrait + Send + Sync + 'static,
    >(
        &self,
        socket_path: &str,
        script_path: String,
        script_params: String,
        state: Arc<ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>>,
    ) -> Result<ScriptResult, PluginError> {
        let socket_service = SocketService::new(socket_path)?;
        let socket_path_clone = socket_service.socket_path().to_string();

        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let server_handle = tokio::spawn(async move {
            let relayer_api = Arc::new(RelayerApi);
            socket_service.listen(shutdown_rx, state, relayer_api).await
        });

        let mut script_result =
            ScriptExecutor::execute_typescript(script_path, socket_path_clone, script_params)
                .await?;

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
    async fn run<
        J: JobProducerTrait + 'static,
        TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
        RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
        NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
        NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
        SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
        TCR: TransactionCounterTrait + Send + Sync + 'static,
        PR: PluginRepositoryTrait + Send + Sync + 'static,
    >(
        &self,
        socket_path: &str,
        script_path: String,
        script_params: String,
        state: Arc<ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>>,
    ) -> Result<ScriptResult, PluginError> {
        self.run(socket_path, script_path, script_params, state)
            .await
    }
}

#[cfg(test)]
mod tests {
    use actix_web::web;
    use std::fs;

    use crate::{
        jobs::MockJobProducerTrait,
        repositories::{
            NetworkRepositoryStorage, NotificationRepositoryStorage, PluginRepositoryStorage,
            RelayerRepositoryStorage, SignerRepositoryStorage, TransactionCounterRepositoryStorage,
            TransactionRepositoryStorage,
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
            console.log(JSON.stringify({ level: 'log', message: 'test' }));
            console.log(JSON.stringify({ level: 'error', message: 'test-error' }));
            console.log(JSON.stringify({ level: 'result', message: 'test-result' }));
        "#;
        fs::write(script_path.clone(), content).unwrap();
        fs::write(ts_config.clone(), TS_CONFIG.as_bytes()).unwrap();

        let state = create_mock_app_state(None, None, None, None).await;

        let plugin_runner = PluginRunner;
        let result = plugin_runner
            .run::<MockJobProducerTrait, TransactionRepositoryStorage, RelayerRepositoryStorage, NetworkRepositoryStorage, NotificationRepositoryStorage, SignerRepositoryStorage, TransactionCounterRepositoryStorage, PluginRepositoryStorage>(
                &socket_path.display().to_string(),
                script_path.display().to_string(),
                "{ \"test\": \"test\" }".to_string(),
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
