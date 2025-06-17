use std::sync::Arc;

use crate::{jobs::JobProducerTrait, models::AppState};

use super::PluginError;
use actix_web::web;
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use tokio::net::UnixListener;
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Request {
    request_id: String,
    _relayer_id: String,
    _method: String,
    _payload: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ScriptResult {
    pub output: String,
    pub error: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    request_id: String,
    result: Option<serde_json::Value>,
    error: Option<String>,
}

pub struct SocketService {
    socket_path: String,
    listener: UnixListener,
}

impl SocketService {
    pub fn from_path(path: &str) -> Result<Self, PluginError> {
        // Remove existing socket file if it exists.
        let _ = std::fs::remove_file(path);
        Ok(Self {
            socket_path: path.to_string(),
            listener: UnixListener::bind(path)
                .map_err(|e| PluginError::SocketError(e.to_string()))?,
        })
    }

    pub async fn run<J: JobProducerTrait + 'static>(
        self,
        path: String,
        state: Arc<web::ThinData<AppState<J>>>,
    ) -> Result<ScriptResult, PluginError> {
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        // clone socket path to use in the spawned task.
        let socket_path = self.socket_path.clone();
        let server_handle = tokio::spawn(async move {
            self.listen(shutdown_rx, state).await.unwrap();
        });

        let output = Command::new("ts-node")
            .arg(path)
            .arg(socket_path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| PluginError::SocketError(e.to_string()))?;

        let _ = shutdown_tx.send(());
        let _ = server_handle.await;

        // Parse execution result
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        Ok(ScriptResult {
            output: stdout.to_string(),
            error: stderr.to_string(),
        })
    }

    async fn listen<J: JobProducerTrait + 'static>(
        &self,
        mut shutdown: oneshot::Receiver<()>,
        state: Arc<web::ThinData<AppState<J>>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            let state = Arc::clone(&state);
            tokio::select! {
                Ok((stream, _)) = self.listener.accept() => {
                    tokio::spawn(Self::handle_connection(stream, state));
                }
                _ = &mut shutdown => {
                    println!("Shutdown signal received. Closing listener.");
                    break;
                }
            }
        }

        Ok(())
    }

    async fn handle_connection<J: JobProducerTrait + 'static>(
        stream: UnixStream,
        state: Arc<web::ThinData<AppState<J>>>,
    ) {
        let (r, mut w) = stream.into_split();
        let mut reader = BufReader::new(r).lines();

        while let Ok(Some(line)) = reader.next_line().await {
            let request: Request = serde_json::from_str(&line).unwrap();
            let response = Self::handle_request(request, &state).await;

            let response_str = serde_json::to_string(&response).unwrap() + "\n";
            let _ = w.write_all(response_str.as_bytes()).await;
        }
    }

    async fn handle_request<J: JobProducerTrait + 'static>(
        request: Request,
        _state: &web::ThinData<AppState<J>>,
    ) -> Response {
        // let relayer_repo_model =
        //     get_relayer_by_id(request.relayer_id.clone(), state).await.unwrap();
        // relayer_repo_model.validate_active_state().unwrap();

        // let network_relayer =
        //     get_network_relayer(request.relayer_id.clone(), state).await.unwrap();

        // let tx_request = NetworkTransactionRequest::from_json(
        //     &relayer_repo_model.network_type,
        //     request.payload.clone(),
        // )
        // .unwrap();

        // tx_request.validate(&relayer_repo_model).unwrap();

        // let transaction = network_relayer.process_transaction_request(tx_request).await.unwrap();
        // let transaction_response: TransactionResponse = transaction.into();
        // let result = serde_json::to_value(request.payload.clone()).unwrap();

        Response {
            request_id: request.request_id,
            result: Some(serde_json::Value::Null),
            error: None,
        }
    }
}
