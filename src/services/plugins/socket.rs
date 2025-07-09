//! This module is responsible for creating a socket connection to the relayer server.
//! It is used to send requests to the relayer server and processing the responses.
//! It also intercepts the logs, errors and return values.
//!
//! The socket connection is created using the `UnixListener`.
//!
//! 1. Creates a socket connection using the `UnixListener`.
//! 2. Each request payload is stringified by the client and added as a new line to the socket.
//! 3. The server reads the requests from the socket and processes them.
//! 4. The server sends the responses back to the client in the same format. By writing a new line in the socket
//! 5. When the client sends the socket shutdown signal, the server closes the socket connection.
//!
//! Example:
//! 1. Create a new socket connection using `/tmp/socket.sock`
//! 2. Client sends request (writes in `/tmp/socket.sock`):
//! ```json
//! {
//!   "request_id": "123",
//!   "relayer_id": "relayer1",
//!   "method": "sendTransaction",
//!   "payload": {
//!     "to": "0x1234567890123456789012345678901234567890",
//!     "value": "1000000000000000000"
//!   }
//! }
//! ```
//! 3. Server process the requests, calls the relayer API and sends back the response (writes in `/tmp/socket.sock`):
//! ```json
//! {
//!   "request_id": "123",
//!   "result": {
//!     "id": "123",
//!     "status": "success"
//!   }
//! }
//! ```
//! 4. Client reads the response (reads from `/tmp/socket.sock`):
//! ```json
//! {
//!   "request_id": "123",
//!   "result": {
//!     "id": "123",
//!     "status": "success"
//!   }
//! }
//! ```
//! 5. Once the client finishes the execution, it sends a shutdown signal to the server.
//! 6. The server closes the socket connection.
//!

use crate::jobs::JobProducerTrait;
use crate::models::{
    NetworkRepoModel, NotificationRepoModel, RelayerRepoModel, SignerRepoModel, ThinDataAppState,
    TransactionRepoModel,
};
use crate::repositories::{
    NetworkRepository, PluginRepositoryTrait, RelayerRepository, Repository,
    TransactionCounterTrait, TransactionRepository,
};
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
    /// Creates a new socket service.
    ///
    /// # Arguments
    ///
    /// * `socket_path` - The path to the socket file.
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

    /// Listens for incoming connections and processes the requests.
    ///
    /// # Arguments
    ///
    /// * `shutdown_rx` - A receiver for the shutdown signal.
    /// * `state` - The application state.
    /// * `relayer_api` - The relayer API.
    ///
    /// # Returns
    ///
    /// A vector of traces.
    #[allow(clippy::type_complexity)]
    pub async fn listen<RA, J, RR, TR, NR, NFR, SR, TCR, PR>(
        self,
        shutdown_rx: oneshot::Receiver<()>,
        state: Arc<ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>>,
        relayer_api: Arc<RA>,
    ) -> Result<Vec<serde_json::Value>, PluginError>
    where
        RA: RelayerApiTrait<J, RR, TR, NR, NFR, SR, TCR, PR> + 'static + Send + Sync,
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
        let mut shutdown = shutdown_rx;

        let mut traces = Vec::new();

        loop {
            let state = Arc::clone(&state);
            let relayer_api = Arc::clone(&relayer_api);
            tokio::select! {
                Ok((stream, _)) = self.listener.accept() => {
                    let result = tokio::spawn(Self::handle_connection::<RA, J, RR, TR, NR, NFR, SR, TCR, PR>(stream, state, relayer_api))
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

    /// Handles a new connection.
    ///
    /// # Arguments
    ///
    /// * `stream` - The stream to the client.
    /// * `state` - The application state.
    /// * `relayer_api` - The relayer API.
    ///
    /// # Returns
    ///
    /// A vector of traces.
    #[allow(clippy::type_complexity)]
    async fn handle_connection<RA, J, RR, TR, NR, NFR, SR, TCR, PR>(
        stream: UnixStream,
        state: Arc<ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>>,
        relayer_api: Arc<RA>,
    ) -> Result<Vec<serde_json::Value>, PluginError>
    where
        RA: RelayerApiTrait<J, RR, TR, NR, NFR, SR, TCR, PR> + 'static + Send + Sync,
        J: JobProducerTrait + 'static,
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
    use crate::{
        services::plugins::{MockRelayerApiTrait, PluginMethod, Response},
        utils::mocks::mockutils::{create_mock_app_state, create_mock_evm_transaction_request},
    };
    use actix_web::web;
    use std::time::Duration;

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

        let state = create_mock_app_state(None, None, None, None, None).await;
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

        mock_relayer.expect_handle_request().returning(|_, _| {
            Box::pin(async move {
                Response {
                    request_id: "test".to_string(),
                    result: Some(serde_json::json!("test")),
                    error: None,
                }
            })
        });

        let service = SocketService::new(socket_path.to_str().unwrap()).unwrap();

        let state = create_mock_app_state(None, None, None, None, None).await;
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
