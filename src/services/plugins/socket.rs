use crate::{jobs::JobProducerTrait, models::AppState};
use actix_web::web;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;

use super::{
    relayer_api::{RelayerApiTrait, Request},
    PluginError,
};

pub struct SocketService {
    socket_path: String,
    listener: UnixListener,
}

impl SocketService {
    pub fn new(socket_path: &str) -> Result<Self, PluginError> {
        // Remove existing socket file if it exists
        let _ = std::fs::remove_file(socket_path);

        let listener =
            UnixListener::bind(socket_path).map_err(|e| PluginError::SocketError(e.to_string()))?;

        Ok(Self {
            socket_path: socket_path.to_string(),
            listener,
        })
    }

    pub fn socket_path(&self) -> &str {
        &self.socket_path
    }

    pub async fn listen<
        J: JobProducerTrait + 'static,
        R: RelayerApiTrait + 'static + Send + Sync,
    >(
        self,
        shutdown_rx: oneshot::Receiver<()>,
        state: Arc<web::ThinData<AppState<J>>>,
        relayer_api: Arc<R>,
    ) -> Result<Vec<serde_json::Value>, PluginError> {
        let mut shutdown = shutdown_rx;

        let mut traces = Vec::new();

        loop {
            let state = Arc::clone(&state);
            let relayer_api = Arc::clone(&relayer_api);
            tokio::select! {
                Ok((stream, _)) = self.listener.accept() => {
                    let result = tokio::spawn(Self::handle_connection::<J, R>(stream, state, relayer_api))
                        .await
                        .map_err(|e| PluginError::SocketError(e.to_string()))?;

                    match result {
                        Ok(trace) => traces.extend(trace),
                        Err(e) => return Err(e),
                    }
                }
                _ = &mut shutdown => {
                    println!("Shutdown signal received. Closing listener.");
                    break;
                }
            }
        }

        Ok(traces)
    }

    async fn handle_connection<
        J: JobProducerTrait + 'static,
        R: RelayerApiTrait + 'static + Send + Sync,
    >(
        stream: UnixStream,
        state: Arc<web::ThinData<AppState<J>>>,
        relayer_api: Arc<R>,
    ) -> Result<Vec<serde_json::Value>, PluginError> {
        let (r, mut w) = stream.into_split();
        let mut reader = BufReader::new(r).lines();
        let mut traces = Vec::new();

        while let Ok(Some(line)) = reader.next_line().await {
            let trace: serde_json::Value = serde_json::from_str(&line)
                .map_err(|e| PluginError::PluginError(format!("Failed to parse trace: {}", e)))?;
            traces.push(trace);

            let request: Request =
                serde_json::from_str(&line).map_err(|e| PluginError::PluginError(e.to_string()))?;

            let response = relayer_api.handle_request(request, &state).await;

            let response_str = serde_json::to_string(&response)
                .map_err(|e| PluginError::PluginError(e.to_string()))?
                + "\n";

            let _ = w.write_all(response_str.as_bytes()).await;
        }

        Ok(traces)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::{
        jobs::MockJobProducerTrait,
        services::plugins::{MockRelayerApiTrait, PluginMethod, Response},
        utils::mocks::mockutils::{create_mock_app_state, create_mock_evm_transaction_request},
    };

    use super::*;

    use tempfile::tempdir;
    use tokio::{
        io::{AsyncBufReadExt, BufReader},
        time::timeout,
    };

    #[tokio::test]
    async fn test_socket_service_listen_and_shutdown() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("test.sock");

        let mock_relayer = MockRelayerApiTrait::default();

        let service = SocketService::new(socket_path.to_str().unwrap()).unwrap();

        let state = create_mock_app_state(None, None, None, None).await;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let listen_handle = tokio::spawn(async move {
            service
                .listen(
                    shutdown_rx,
                    Arc::new(web::ThinData(state)),
                    Arc::new(mock_relayer),
                )
                .await
        });

        shutdown_tx.send(()).unwrap();

        let result = timeout(Duration::from_millis(100), listen_handle).await;
        assert!(result.is_ok(), "Listen handle timed out");
        assert!(result.unwrap().is_ok(), "Listen handle returned error");
    }

    #[tokio::test]
    async fn test_socket_service_handle_connection() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("test.sock");

        let mut mock_relayer = MockRelayerApiTrait::default();

        mock_relayer
            .expect_handle_request::<MockJobProducerTrait>()
            .returning(|_, _| Response {
                request_id: "test".to_string(),
                result: Some(serde_json::json!("test")),
                error: None,
            });

        let service = SocketService::new(socket_path.to_str().unwrap()).unwrap();

        let state = create_mock_app_state(None, None, None, None).await;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let listen_handle = tokio::spawn(async move {
            service
                .listen(
                    shutdown_rx,
                    Arc::new(web::ThinData(state)),
                    Arc::new(mock_relayer),
                )
                .await
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = UnixStream::connect(socket_path.to_str().unwrap())
            .await
            .unwrap();

        let request = Request {
            request_id: "test".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::SendTransaction,
            payload: serde_json::json!(create_mock_evm_transaction_request()),
        };

        let request_json = serde_json::to_string(&request).unwrap() + "\n";

        client.write_all(request_json.as_bytes()).await.unwrap();

        let mut reader = BufReader::new(&mut client);
        let mut response_str = String::new();
        let read_result = timeout(
            Duration::from_millis(1000),
            reader.read_line(&mut response_str),
        )
        .await;

        assert!(
            read_result.is_ok(),
            "Reading response timed out: {:?}",
            read_result
        );
        let bytes_read = read_result.unwrap().unwrap();
        assert!(bytes_read > 0, "No data received");
        shutdown_tx.send(()).unwrap();

        let response: Response = serde_json::from_str(&response_str).unwrap();

        assert!(response.error.is_none(), "Error should be none");
        assert!(response.result.is_some(), "Result should be some");
        assert_eq!(
            response.request_id, request.request_id,
            "Request id mismatch"
        );

        client.shutdown().await.unwrap();

        let traces = listen_handle.await.unwrap().unwrap();

        assert_eq!(traces.len(), 1);
        let expected: serde_json::Value = serde_json::from_str(&request_json).unwrap();
        let actual: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&traces[0]).unwrap()).unwrap();
        assert_eq!(expected, actual, "Request json mismatch with trace");
    }
}
