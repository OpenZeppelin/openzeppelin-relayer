use crate::{jobs::JobProducerTrait, models::AppState};
use actix_web::web;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;

use super::{
    relayer_api::{RelayerApi, Request},
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

    pub async fn listen<J: JobProducerTrait + 'static>(
        self,
        shutdown_rx: oneshot::Receiver<()>,
        state: Arc<web::ThinData<AppState<J>>>,
    ) -> Result<(), PluginError> {
        let mut shutdown = shutdown_rx;

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
    ) -> Result<(), PluginError> {
        let (r, mut w) = stream.into_split();
        let mut reader = BufReader::new(r).lines();

        while let Ok(Some(line)) = reader.next_line().await {
            let request: Request =
                serde_json::from_str(&line).map_err(|e| PluginError::PluginError(e.to_string()))?;

            let response = RelayerApi::handle_request(request, &state).await;

            let response_str = serde_json::to_string(&response)
                .map_err(|e| PluginError::PluginError(e.to_string()))?
                + "\n";

            let _ = w.write_all(response_str.as_bytes()).await;
        }

        Ok(())
    }
}
