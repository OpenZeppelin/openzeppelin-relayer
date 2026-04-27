mod proof_server;
mod subxt_client;
mod tx_builder;

pub use proof_server::RemoteProofServer;
pub use subxt_client::MidnightSubxtClient;
pub use tx_builder::MidnightTxBuilder;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::models::{MidnightNetwork, RpcConfig};
use crate::services::provider::rpc_selector::RpcSelector;
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
    /// Lazily-connected Subxt client. Held in a `RwLock<Option<...>>`
    /// rather than a `OnceCell` so a transport-level failure can clear
    /// the cache and force a reconnect via the selector. `Clone` is
    /// cheap on `MidnightSubxtClient` (the inner `OnlineClient` shares
    /// state via Arc), so callers take an owned copy and release the
    /// lock before doing network IO.
    subxt_state: Arc<RwLock<Option<MidnightSubxtClient>>>,
    selector: RpcSelector,
}

impl MidnightProvider {
    pub fn new(network: MidnightNetwork) -> Result<Self, ProviderError> {
        let rpc_client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::NetworkConfiguration(e.to_string()))?;
        let indexer_client = MidnightIndexerClient::new(network.indexer_urls.clone());
        let selector = RpcSelector::new_with_defaults(network.rpc_urls.clone()).map_err(|e| {
            ProviderError::NetworkConfiguration(format!(
                "failed to build Midnight RPC selector: {e}"
            ))
        })?;

        Ok(Self {
            network,
            rpc_client,
            indexer_client,
            subxt_state: Arc::new(RwLock::new(None)),
            selector,
        })
    }

    /// Get a connected Subxt client, connecting (with selector failover)
    /// if one isn't cached. Returned by value (cheap clone) so the lock
    /// is released before any network IO on the client.
    async fn subxt_client(&self) -> Result<MidnightSubxtClient, ProviderError> {
        if let Some(c) = self.subxt_state.read().await.clone() {
            return Ok(c);
        }
        let mut guard = self.subxt_state.write().await;
        if let Some(c) = guard.clone() {
            return Ok(c);
        }
        let fresh = self.connect_subxt().await?;
        *guard = Some(fresh.clone());
        Ok(fresh)
    }

    /// Drop the cached Subxt client so the next `subxt_client()` call
    /// reconnects. Used after a transport-level failure to recover from
    /// a node that went away mid-session.
    async fn invalidate_subxt(&self) {
        *self.subxt_state.write().await = None;
    }

    /// Walk the selector until a URL connects via WebSocket. Subxt needs
    /// a WS endpoint while the HTTP RPC in config uses https — converted
    /// per-URL.
    async fn connect_subxt(&self) -> Result<MidnightSubxtClient, ProviderError> {
        let mut tried: HashSet<String> = HashSet::new();
        let mut last_err: Option<ProviderError> = None;
        for _ in 0..self.selector.provider_count() {
            let url = match self.selector.get_next_url(&tried) {
                Ok(u) => u,
                Err(e) => {
                    last_err = Some(ProviderError::NetworkConfiguration(e.to_string()));
                    break;
                }
            };
            let ws_url = url
                .replace("https://", "wss://")
                .replace("http://", "ws://");
            match MidnightSubxtClient::connect(&ws_url).await {
                Ok(c) => return Ok(c),
                Err(e) => {
                    self.selector.mark_current_as_failed();
                    tried.insert(url);
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| {
            ProviderError::NetworkConfiguration(
                "Midnight network has no RPC URLs configured".to_string(),
            )
        }))
    }

    pub fn network(&self) -> &MidnightNetwork {
        &self.network
    }

    pub fn rpc_urls(&self) -> &[RpcConfig] {
        &self.network.rpc_urls
    }

    /// Per-call failover: walk the selector until a URL succeeds or all are
    /// exhausted. Network-level failures (timeout, connection error, 5xx)
    /// mark the provider as failed and trigger failover; RPC-level errors
    /// (`RpcErrorCode`) propagate without retry — those are application
    /// errors from a healthy node, not a node-down signal.
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

        let mut tried: HashSet<String> = HashSet::new();
        let mut last_err: Option<ProviderError> = None;
        for _ in 0..self.selector.provider_count() {
            let url = match self.selector.get_next_url(&tried) {
                Ok(u) => u,
                Err(e) => {
                    last_err = Some(ProviderError::NetworkConfiguration(e.to_string()));
                    break;
                }
            };

            match self.rpc_call_once::<T>(&url, &payload).await {
                Ok(v) => return Ok(v),
                Err(err) if is_provider_failure(&err) => {
                    tracing::warn!(
                        url = %url,
                        method = %method,
                        error = %err,
                        "Midnight RPC provider failure, marking failed and failing over"
                    );
                    self.selector.mark_current_as_failed();
                    tried.insert(url);
                    last_err = Some(err);
                    continue;
                }
                Err(err) => return Err(err),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            ProviderError::NetworkConfiguration(
                "Midnight network has no RPC URLs configured".to_string(),
            )
        }))
    }

    async fn rpc_call_once<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        payload: &Value,
    ) -> Result<T, ProviderError> {
        let response = self
            .rpc_client
            .post(url)
            .json(payload)
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

/// Heuristic: does this `ProviderError` from `MidnightSubxtClient::submit_transaction`
/// look like a WS/transport failure (worth invalidating + reconnecting) rather
/// than a chain-level rejection (InvalidTransaction, decode error, etc.)?
///
/// `subxt::Error::Rpc(_)`, `Io`, `WebSocket`, "connection closed", and
/// "subscription dropped" all appear in subxt's debug output via the `{e:?}`
/// format used in `MidnightSubxtClient::submit_transaction`. Chain rejections
/// look like `InvalidTransaction(...)`, `Custom(...)`, or carry a Substrate
/// error code — those should propagate without retry to avoid duplicate
/// submission attempts.
fn is_subxt_transport_error(err: &ProviderError) -> bool {
    let msg = match err {
        ProviderError::Other(s) => s.as_str(),
        ProviderError::NetworkConfiguration(s) => s.as_str(),
        ProviderError::Timeout => return true,
        ProviderError::TransportError(_) => return true,
        _ => return false,
    };
    let m = msg.to_ascii_lowercase();
    // Match only on substrings that clearly indicate a connection/IO layer
    // failure. We deliberately do NOT match `rpc(` because that prefix also
    // wraps chain-rejection envelopes like
    // `Rpc(JsonRpcError { code: 1010, message: "Invalid Transaction" })`,
    // which we must NOT retry — duplicate-submission risk.
    [
        "websocket",
        "connection",
        "transport",
        "io(",
        "disconnected",
        "subscription",
        "eof",
        "reset by peer",
        "broken pipe",
    ]
    .iter()
    .any(|needle| m.contains(needle))
}

/// Classify whether a `ProviderError` indicates the *provider* (RPC node)
/// is unhealthy and should be skipped in favor of the next URL.
///
/// Application-level errors (`RpcErrorCode`, `InvalidAddress`) are
/// excluded: a healthy node can return them, and they should surface to
/// the caller rather than triggering a node-failover loop. Everything
/// else (timeout, transport error, rate-limited, bad gateway, generic
/// "other") is treated as a node-down signal.
fn is_provider_failure(err: &ProviderError) -> bool {
    !matches!(
        err,
        ProviderError::RpcErrorCode { .. } | ProviderError::InvalidAddress(_)
    )
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

        let client = self.subxt_client().await?;
        match client.submit_transaction(bytes.clone()).await {
            Ok(r) => Ok(r),
            Err(e) if is_subxt_transport_error(&e) => {
                tracing::warn!(
                    error = %e,
                    "Subxt transport failure on submit; invalidating client and reconnecting"
                );
                self.invalidate_subxt().await;
                let fresh = self.subxt_client().await?;
                fresh.submit_transaction(bytes).await
            }
            Err(e) => Err(e),
        }
    }

    fn get_indexer_client(&self) -> &MidnightIndexerClient {
        &self.indexer_client
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_error_code_does_not_trigger_failover() {
        let err = ProviderError::RpcErrorCode {
            code: -32601,
            message: "Method not found".into(),
        };
        assert!(!is_provider_failure(&err));
    }

    #[test]
    fn invalid_address_does_not_trigger_failover() {
        let err = ProviderError::InvalidAddress("bad bech32".into());
        assert!(!is_provider_failure(&err));
    }

    #[test]
    fn timeout_triggers_failover() {
        assert!(is_provider_failure(&ProviderError::Timeout));
    }

    #[test]
    fn transport_and_request_errors_trigger_failover() {
        assert!(is_provider_failure(&ProviderError::TransportError(
            "connection reset".into()
        )));
        assert!(is_provider_failure(&ProviderError::RequestError {
            error: "5xx".into(),
            status_code: 503,
        }));
        assert!(is_provider_failure(&ProviderError::BadGateway));
        assert!(is_provider_failure(&ProviderError::RateLimited));
    }

    #[test]
    fn subxt_transport_substrings_trigger_reconnect() {
        // Sample error strings that should look transport-y (modeled on
        // subxt's actual debug output).
        let cases = [
            "Extrinsic submission failed: Rpc(ClientError(WebSocket(IoError(Os { code: 32, kind: BrokenPipe }))))",
            "Extrinsic submission failed: Io(Custom { kind: ConnectionReset })",
            "Extrinsic submission failed: Subscription dropped before response",
            "Extrinsic submission failed: connection closed by remote peer",
            "Extrinsic submission failed: Transport(Custom { ... })",
            "Extrinsic submission failed: unexpected EOF",
        ];
        for c in cases {
            let err = ProviderError::Other(c.into());
            assert!(
                is_subxt_transport_error(&err),
                "expected transport hit for: {c}"
            );
        }
    }

    #[test]
    fn subxt_chain_rejection_does_not_trigger_reconnect() {
        // These look like node-rejected the transaction — must not loop on retry.
        let cases = [
            "Extrinsic submission failed: Rpc(JsonRpcError { code: 1010, message: \"Invalid Transaction\", data: \"BadProof\" })",
            "Extrinsic submission failed: InvalidTransaction(Custom(170))",
            "Extrinsic submission failed: Decode(Could not decode OpaqueExtrinsic.0)",
        ];
        for c in cases {
            let err = ProviderError::Other(c.into());
            assert!(
                !is_subxt_transport_error(&err),
                "did not expect transport hit for: {c}"
            );
        }
    }
}
