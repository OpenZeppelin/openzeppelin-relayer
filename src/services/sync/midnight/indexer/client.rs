//!
//! GraphQL client for Midnight blockchain indexer with session management.
//!
//! This module provides an async client for interacting with the Midnight GraphQL indexer.
//! It supports wallet session management, real-time subscriptions to wallet and block events,
//! and GraphQL query execution. All methods are async and designed for use with Tokio.

use crate::services::midnight::indexer::types::*;

use futures_util::{SinkExt, StreamExt};
use log::{debug, error, info};
use reqwest::Client;
use serde_json::json;
use std::time::Duration;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};

/// Midnight GraphQL indexer client
#[derive(Clone)]
pub struct MidnightIndexerClient {
    /// The underlying HTTP client for GraphQL queries.
    http_client: Client,
    /// The HTTP URL for GraphQL queries.
    http_url: String,
    /// The WebSocket URL for real-time subscriptions.
    ws_url: String,
}

impl MidnightIndexerClient {
    /// Create a new indexer client.
    ///
    /// # Arguments
    /// * `http_url` - The HTTP endpoint for GraphQL queries.
    /// * `ws_url` - The WebSocket endpoint for subscriptions.
    ///
    /// # Returns
    /// A new `MidnightIndexerClient` instance.
    pub fn new(http_url: String, ws_url: String) -> Self {
        let http_client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            http_client,
            http_url,
            ws_url,
        }
    }

    /// Establish wallet session with viewing key.
    ///
    /// # Arguments
    /// * `viewing_key` - The viewing key to use for the wallet session.
    ///
    /// # Returns
    /// The session ID as a string, or an `IndexerError` if the connection fails.
    pub async fn connect_wallet(
        &self,
        viewing_key: &ViewingKeyFormat,
    ) -> Result<String, IndexerError> {
        info!(
            "Connecting wallet with viewing key: {}",
            viewing_key.as_str()
        );

        let query = r#"
            mutation ConnectWallet($viewingKey: ViewingKey!) {
                connect(viewingKey: $viewingKey)
            }
        "#;

        let variables = json!({
            "viewingKey": viewing_key.as_str()
        });

        let response = self.execute_query(query, Some(variables)).await?;

        let session_id = response
            .get("data")
            .and_then(|data| data.get("connect"))
            .and_then(|connect| connect.as_str())
            .ok_or_else(|| IndexerError::NoData)?
            .to_string();

        info!("Connected wallet with session ID: {}", session_id);
        Ok(session_id)
    }

    /// Subscribe to wallet updates using session ID.
    ///
    /// # Arguments
    /// * `session_id` - The wallet session ID.
    /// * `start_index` - Optional starting blockchain index for the subscription.
    /// * `send_progress_updates` - Whether to include progress updates in the stream.
    ///
    /// # Returns
    /// A pinned async stream of `WalletSyncEvent` results. Each item is either a wallet event or an error.
    ///
    /// # Errors
    /// Returns `IndexerError` if the WebSocket connection or subscription fails.
    pub async fn subscribe_wallet(
        &self,
        session_id: &str,
        start_index: Option<u64>,
        send_progress_updates: Option<bool>,
    ) -> Result<
        std::pin::Pin<
            Box<dyn futures_util::Stream<Item = Result<WalletSyncEvent, IndexerError>> + Send>,
        >,
        IndexerError,
    > {
        debug!("Attempting WebSocket connection to: {}", self.ws_url);

        // Create WebSocket request with required subprotocol
        let mut request = self.ws_url.clone().into_client_request()?;
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            "graphql-transport-ws".parse().map_err(|_| {
                IndexerError::GraphQLError("Invalid WebSocket subprotocol header value".to_string())
            })?,
        );

        let (ws_stream, response) = connect_async(request).await?;
        debug!(
            "WebSocket connection established, response status: {}",
            response.status()
        );
        let (mut ws_sender, mut ws_receiver) = ws_stream.split();

        // Send connection init
        let init_message = json!({
            "type": "connection_init"
        });
        ws_sender
            .send(Message::Text(init_message.to_string()))
            .await?;

        // Wait for connection ack
        if let Some(msg) = ws_receiver.next().await {
            match msg? {
                Message::Text(text) => {
                    let parsed: serde_json::Value = serde_json::from_str(&text)?;
                    if parsed.get("type")
                        != Some(&serde_json::Value::String("connection_ack".to_string()))
                    {
                        return Err(IndexerError::SessionError(
                            "Connection not acknowledged".to_string(),
                        ));
                    }
                }
                _ => {
                    return Err(IndexerError::SessionError(
                        "Unexpected message type during handshake".to_string(),
                    ));
                }
            }
        }

        // Start wallet subscription
        let subscription_query = format!(
            r#"
            subscription WalletSync {{
                wallet(sessionId: "{}", index: {}, sendProgressUpdates: {}) {{
                    __typename
                    ... on ViewingUpdate {{
                        index
                        update {{
                            __typename
                            ... on RelevantTransaction {{
                                transaction {{
                                    hash
                                    applyStage
                                    raw
                                    identifiers
                                    merkleTreeRoot
                                    protocolVersion
                                }}
                                start
                                end
                            }}
                            ... on MerkleTreeCollapsedUpdate {{
                                protocolVersion
                                start
                                end
                                update
                            }}
                        }}
                    }}
                    ... on ProgressUpdate {{
                        highestIndex
                        highestRelevantIndex
                        highestRelevantWalletIndex
                    }}
                }}
            }}
            "#,
            session_id,
            start_index.unwrap_or(0),
            send_progress_updates.unwrap_or(true)
        );

        let start_message = json!({
            "id": "wallet-sync",
            "type": "subscribe",
            "payload": {
                "query": subscription_query
            }
        });

        ws_sender
            .send(Message::Text(start_message.to_string()))
            .await?;

        // Return stream of wallet events
        let stream = ws_receiver.filter_map(|msg| async move {
            match msg {
                Ok(Message::Text(text)) => {
                    match serde_json::from_str::<serde_json::Value>(&text) {
                        Ok(parsed) => {
                            // Handle different message types
                            if let Some(msg_type) = parsed.get("type").and_then(|t| t.as_str()) {
                                match msg_type {
                                    "next" => {
                                        if let Some(wallet_data) = parsed
                                            .get("payload")
                                            .and_then(|p| p.get("data"))
                                            .and_then(|d| d.get("wallet"))
                                        {
                                            debug!(
                                                "Raw wallet data: {}",
                                                serde_json::to_string_pretty(&wallet_data)
                                                    .unwrap_or_else(|_| "Invalid JSON".to_string())
                                            );
                                            match serde_json::from_value::<WalletSyncEvent>(
                                                wallet_data.clone(),
                                            ) {
                                                Ok(event) => Some(Ok(event)),
                                                Err(e) => {
                                                    error!(
                                                        "Failed to deserialize wallet event: {}",
                                                        e
                                                    );
                                                    error!(
                                                        "Raw data was: {}",
                                                        serde_json::to_string_pretty(&wallet_data)
                                                            .unwrap_or_else(
                                                                |_| "Invalid JSON".to_string()
                                                            )
                                                    );
                                                    Some(Err(IndexerError::JsonError(e)))
                                                }
                                            }
                                        } else {
                                            Some(Err(IndexerError::NoData))
                                        }
                                    }
                                    "error" => {
                                        let error_msg = parsed
                                            .get("payload")
                                            .and_then(|p| p.get("message"))
                                            .and_then(|m| m.as_str())
                                            .unwrap_or("Unknown subscription error");
                                        Some(Err(IndexerError::GraphQLError(error_msg.to_string())))
                                    }
                                    "complete" => {
                                        debug!("Wallet subscription completed");
                                        None // End the stream
                                    }
                                    _ => {
                                        debug!("Ignoring message type: {}", msg_type);
                                        None // Skip other message types
                                    }
                                }
                            } else {
                                Some(Err(IndexerError::GraphQLError(
                                    "Message missing type field".to_string(),
                                )))
                            }
                        }
                        Err(e) => Some(Err(IndexerError::JsonError(e))),
                    }
                }
                Ok(_) => Some(Err(IndexerError::GraphQLError(
                    "Unexpected message type".to_string(),
                ))),
                Err(e) => Some(Err(IndexerError::WebSocketError(e))),
            }
        });

        Ok(Box::pin(stream))
    }

    /// Subscribe to all blocks from a given height.
    ///
    /// # Arguments
    /// * `start_height` - The starting block height for the subscription.
    ///
    /// # Returns
    /// A pinned async stream of block data as JSON values, or errors.
    ///
    /// # Errors
    /// Returns `IndexerError` if the WebSocket connection or subscription fails.
    #[allow(dead_code)]
    pub async fn subscribe_blocks(
        &self,
        start_height: u64,
    ) -> Result<
        std::pin::Pin<
            Box<dyn futures_util::Stream<Item = Result<serde_json::Value, IndexerError>> + Send>,
        >,
        IndexerError,
    > {
        debug!(
            "Attempting WebSocket connection for blocks subscription to: {}",
            self.ws_url
        );

        // Create WebSocket request with required subprotocol
        let mut request = self.ws_url.clone().into_client_request()?;
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            "graphql-transport-ws".parse().map_err(|_| {
                IndexerError::GraphQLError("Invalid WebSocket subprotocol header value".to_string())
            })?,
        );

        let (ws_stream, response) = connect_async(request).await?;
        debug!(
            "WebSocket connection established for blocks, response status: {}",
            response.status()
        );
        let (mut ws_sender, mut ws_receiver) = ws_stream.split();

        // Send connection init
        let init_message = json!({
            "type": "connection_init"
        });
        ws_sender
            .send(Message::Text(init_message.to_string()))
            .await?;

        // Wait for connection ack
        if let Some(msg) = ws_receiver.next().await {
            match msg? {
                Message::Text(text) => {
                    let parsed: serde_json::Value = serde_json::from_str(&text)?;
                    if parsed.get("type")
                        != Some(&serde_json::Value::String("connection_ack".to_string()))
                    {
                        return Err(IndexerError::SessionError(
                            "Connection not acknowledged".to_string(),
                        ));
                    }
                }
                _ => {
                    return Err(IndexerError::SessionError(
                        "Unexpected message type during handshake".to_string(),
                    ));
                }
            }
        }

        // Start blocks subscription
        let subscription_query = format!(
            r#"
            subscription Blocks {{
                blocks(offset: {{ height: {} }}) {{
                    hash
                    height
                    protocolVersion
                    timestamp
                    author
                    transactions {{
                        hash
                        protocolVersion
                        applyStage
                        identifiers
                        raw
                        merkleTreeRoot
                    }}
                }}
            }}
            "#,
            start_height
        );

        let start_message = json!({
            "id": "blocks-sync",
            "type": "subscribe",
            "payload": {
                "query": subscription_query
            }
        });

        ws_sender
            .send(Message::Text(start_message.to_string()))
            .await?;

        // Return stream of block events
        let stream = ws_receiver.filter_map(|msg| async move {
            match msg {
                Ok(Message::Text(text)) => {
                    match serde_json::from_str::<serde_json::Value>(&text) {
                        Ok(parsed) => {
                            // Handle different message types
                            if let Some(msg_type) = parsed.get("type").and_then(|t| t.as_str()) {
                                match msg_type {
                                    "next" => {
                                        if let Some(blocks_data) = parsed
                                            .get("payload")
                                            .and_then(|p| p.get("data"))
                                            .and_then(|d| d.get("blocks"))
                                        {
                                            debug!(
                                                "Raw blocks data: {}",
                                                serde_json::to_string_pretty(&blocks_data)
                                                    .unwrap_or_else(|_| "Invalid JSON".to_string())
                                            );
                                            Some(Ok(blocks_data.clone()))
                                        } else {
                                            Some(Err(IndexerError::NoData))
                                        }
                                    }
                                    "error" => {
                                        let error_msg = parsed
                                            .get("payload")
                                            .and_then(|p| p.get("message"))
                                            .and_then(|m| m.as_str())
                                            .unwrap_or("Unknown subscription error");
                                        Some(Err(IndexerError::GraphQLError(error_msg.to_string())))
                                    }
                                    "complete" => {
                                        info!("Blocks subscription completed");
                                        None // End the stream
                                    }
                                    _ => {
                                        debug!("Ignoring message type: {}", msg_type);
                                        None // Skip other message types
                                    }
                                }
                            } else {
                                Some(Err(IndexerError::GraphQLError(
                                    "Message missing type field".to_string(),
                                )))
                            }
                        }
                        Err(e) => Some(Err(IndexerError::JsonError(e))),
                    }
                }
                Ok(_) => Some(Err(IndexerError::GraphQLError(
                    "Unexpected message type".to_string(),
                ))),
                Err(e) => Some(Err(IndexerError::WebSocketError(e))),
            }
        });

        Ok(Box::pin(stream))
    }

    /// Execute a GraphQL query.
    ///
    /// # Arguments
    /// * `query` - The GraphQL query string.
    /// * `variables` - Optional variables for the query.
    ///
    /// # Returns
    /// The JSON response from the indexer, or an `IndexerError` if the request fails.
    pub async fn execute_query(
        &self,
        query: &str,
        variables: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, IndexerError> {
        let request_body = json!({
            "query": query,
            "variables": variables
        });

        let response = self
            .http_client
            .post(&self.http_url)
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(IndexerError::GraphQLError(format!(
                "HTTP error: {}",
                response.status()
            )));
        }

        let response_json: serde_json::Value = response.json().await?;

        if let Some(errors) = response_json.get("errors") {
            return Err(IndexerError::GraphQLError(format!(
                "GraphQL errors: {}",
                errors
            )));
        }

        Ok(response_json)
    }
}
