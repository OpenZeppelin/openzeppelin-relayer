mod proof_server;
mod subxt_client;
mod tx_builder;

pub use proof_server::RemoteProofServer;
pub use subxt_client::MidnightSubxtClient;
pub use tx_builder::MidnightTxBuilder;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::OnceCell;

use crate::models::{MidnightNetwork, RpcConfig};
use crate::services::provider::ProviderError;
use crate::services::sync::midnight::MidnightIndexerClient;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransactionSubmissionResult {
    pub extrinsic_tx_hash: String,
    pub pallet_tx_hash: Option<String>,
}

#[async_trait]
pub trait MidnightProviderTrait: Send + Sync {
    async fn health_check(&self) -> Result<bool, ProviderError>;
    async fn get_block_number(&self) -> Result<u64, ProviderError>;
    async fn get_block_by_hash(&self, hash: &str) -> Result<Option<Value>, ProviderError>;
    async fn get_transaction_by_hash(&self, hash: &str) -> Result<Option<Value>, ProviderError>;
    async fn send_raw_extrinsic(
        &self,
        encoded_extrinsic: &str,
    ) -> Result<TransactionSubmissionResult, ProviderError>;
    fn get_indexer_client(&self) -> &MidnightIndexerClient;
}

#[derive(Debug, Clone)]
pub struct MidnightProvider {
    network: MidnightNetwork,
    rpc_client: Client,
    indexer_client: MidnightIndexerClient,
    subxt_cell: Arc<OnceCell<MidnightSubxtClient>>,
}

impl MidnightProvider {
    pub fn new(network: MidnightNetwork) -> Result<Self, ProviderError> {
        let rpc_client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::NetworkConfiguration(e.to_string()))?;
        let indexer_client = MidnightIndexerClient::new(network.indexer_urls.clone());

        Ok(Self {
            network,
            rpc_client,
            indexer_client,
            subxt_cell: Arc::new(OnceCell::new()),
        })
    }

    /// Lazily connect a Subxt client to the node. Subxt needs a WebSocket
    /// endpoint while the HTTP RPC in config uses https — convert on first use.
    async fn subxt_client(&self) -> Result<&MidnightSubxtClient, ProviderError> {
        self.subxt_cell
            .get_or_try_init(|| async {
                let url = self.first_rpc_url()?;
                let ws_url = url
                    .replace("https://", "wss://")
                    .replace("http://", "ws://");
                MidnightSubxtClient::connect(&ws_url).await
            })
            .await
    }

    pub fn network(&self) -> &MidnightNetwork {
        &self.network
    }

    pub fn rpc_urls(&self) -> &[RpcConfig] {
        &self.network.rpc_urls
    }

    fn first_rpc_url(&self) -> Result<&str, ProviderError> {
        self.rpc_urls()
            .first()
            .map(|cfg| cfg.url.as_str())
            .ok_or_else(|| {
                ProviderError::NetworkConfiguration(
                    "Midnight network has no RPC URLs configured".to_string(),
                )
            })
    }

    async fn rpc_call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<T, ProviderError> {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });

        let response = self
            .rpc_client
            .post(self.first_rpc_url()?)
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    ProviderError::Timeout
                } else {
                    ProviderError::Other(e.to_string())
                }
            })?;

        let body: Value = response
            .error_for_status()
            .map_err(|e| ProviderError::RequestError {
                error: e.to_string(),
                status_code: e.status().map(|s| s.as_u16()).unwrap_or(500),
            })?
            .json()
            .await
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        if let Some(error) = body.get("error") {
            let code = error
                .get("code")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Unknown Midnight RPC error")
                .to_string();
            return Err(ProviderError::RpcErrorCode { code, message });
        }

        serde_json::from_value(body.get("result").cloned().unwrap_or(Value::Null))
            .map_err(|e| ProviderError::Other(e.to_string()))
    }
}

#[async_trait]
impl MidnightProviderTrait for MidnightProvider {
    async fn health_check(&self) -> Result<bool, ProviderError> {
        let _: Value = self.rpc_call("system_health", json!([])).await?;
        Ok(true)
    }

    async fn get_block_number(&self) -> Result<u64, ProviderError> {
        #[derive(Deserialize)]
        struct Header {
            number: String,
        }

        let header: Header = self.rpc_call("chain_getHeader", json!([])).await?;
        u64::from_str_radix(header.number.trim_start_matches("0x"), 16)
            .map_err(|e| ProviderError::Other(format!("Invalid block number hex: {e}")))
    }

    async fn get_block_by_hash(&self, hash: &str) -> Result<Option<Value>, ProviderError> {
        self.indexer_client
            .get_block_by_hash(hash)
            .await
            .map(|result| result.map(|block| serde_json::to_value(block).unwrap_or(Value::Null)))
            .map_err(|e| ProviderError::Other(e.to_string()))
    }

    async fn get_transaction_by_hash(&self, hash: &str) -> Result<Option<Value>, ProviderError> {
        self.indexer_client
            .get_transaction_by_hash(hash)
            .await
            .map(|result| result.map(|tx| serde_json::to_value(tx).unwrap_or(Value::Null)))
            .map_err(|e| ProviderError::Other(e.to_string()))
    }

    async fn send_raw_extrinsic(
        &self,
        encoded_extrinsic: &str,
    ) -> Result<TransactionSubmissionResult, ProviderError> {
        // Midnight transaction bytes are NOT a valid Substrate OpaqueExtrinsic
        // on their own — author_submitExtrinsic rejects them with JSON-RPC 1040
        // "Could not decode OpaqueExtrinsic.0". Instead, wrap the bytes in the
        // Midnight pallet's send_mn_transaction call and submit as an unsigned
        // extrinsic via Subxt.
        let bytes = hex::decode(encoded_extrinsic.trim_start_matches("0x"))
            .map_err(|e| ProviderError::Other(format!("Invalid extrinsic hex: {e}")))?;

        self.subxt_client().await?.submit_transaction(bytes).await
    }

    fn get_indexer_client(&self) -> &MidnightIndexerClient {
        &self.indexer_client
    }
}
