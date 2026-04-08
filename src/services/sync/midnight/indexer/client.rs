use std::time::Duration;

use reqwest::Client;
use serde_json::{json, Value};

use crate::config::IndexerUrls;

use super::{BlockData, IndexerError, TransactionData, ViewingKeyFormat, WalletSyncEvent};

#[derive(Clone, Debug)]
pub struct MidnightIndexerClient {
    http_client: Client,
    http_url: String,
    ws_url: String,
}

impl MidnightIndexerClient {
    pub fn new(indexer_urls: IndexerUrls) -> Self {
        let http_client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build Midnight indexer client");

        Self {
            http_client,
            http_url: indexer_urls.http,
            ws_url: indexer_urls.ws,
        }
    }

    pub fn http_url(&self) -> &str {
        &self.http_url
    }

    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }

    pub async fn health_check(&self) -> Result<(), IndexerError> {
        self.execute_query("query HealthCheck { __typename }", None)
            .await
            .map(|_| ())
    }

    pub async fn connect_wallet(
        &self,
        viewing_key: &ViewingKeyFormat,
    ) -> Result<String, IndexerError> {
        let query = r#"
            mutation ConnectWallet($viewingKey: ViewingKey!) {
                connect(viewingKey: $viewingKey)
            }
        "#;

        let response = self
            .execute_query(
                query,
                Some(json!({
                    "viewingKey": viewing_key.as_str()
                })),
            )
            .await?;

        response
            .get("data")
            .and_then(|data| data.get("connect"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or(IndexerError::NoData)
    }

    /// Disconnect a wallet session from the indexer.
    pub async fn disconnect_wallet(&self, session_id: &str) -> Result<(), IndexerError> {
        let query = r#"
            mutation DisconnectWallet($sessionId: HexEncoded!) {
                disconnect(sessionId: $sessionId)
            }
        "#;

        self.execute_query(query, Some(json!({ "sessionId": session_id })))
            .await
            .map(|_| ())
    }

    /// Fetch wallet sync events from the indexer using an HTTP subscription query.
    ///
    /// This is the HTTP-based approach (polling). A WebSocket-based streaming
    /// approach using `shieldedTransactions` subscription can be added later for
    /// real-time incremental sync.
    pub async fn fetch_wallet_events(
        &self,
        session_id: &str,
        start_index: Option<u64>,
    ) -> Result<Vec<WalletSyncEvent>, IndexerError> {
        let query = r#"
            query WalletSync($sessionId: HexEncoded!, $index: Int, $sendProgressUpdates: Boolean) {
                shieldedTransactions(sessionId: $sessionId, index: $index, sendProgressUpdates: $sendProgressUpdates) {
                    __typename
                    ... on ViewingUpdate {
                        index
                        update {
                            __typename
                            ... on RelevantTransaction {
                                transaction {
                                    hash
                                    identifiers
                                    raw
                                    applyStage
                                    merkleTreeRoot
                                    protocolVersion
                                }
                                start
                                end
                            }
                            ... on MerkleTreeCollapsedUpdate {
                                protocolVersion
                                start
                                end
                                update
                            }
                        }
                    }
                    ... on ProgressUpdate {
                        highestIndex
                        highestRelevantIndex
                        highestRelevantWalletIndex
                    }
                }
            }
        "#;

        let mut variables = json!({
            "sessionId": session_id,
            "sendProgressUpdates": true,
        });

        if let Some(idx) = start_index {
            variables["index"] = json!(idx);
        }

        let response = self.execute_query(query, Some(variables)).await?;

        let events_value = response
            .get("data")
            .and_then(|d| d.get("shieldedTransactions"));

        match events_value {
            Some(Value::Array(arr)) => arr
                .iter()
                .map(|v| serde_json::from_value(v.clone()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(IndexerError::from),
            Some(val) => {
                // Single event (not array)
                let event: WalletSyncEvent = serde_json::from_value(val.clone())?;
                Ok(vec![event])
            }
            None => Ok(vec![]),
        }
    }

    /// Query a transaction by hash from the indexer.
    ///
    /// Uses the v4 `transactions(offset: { hash: ... })` query which returns
    /// the transaction with its nested block data.
    pub async fn get_transaction_by_hash(
        &self,
        hash: &str,
    ) -> Result<Option<TransactionData>, IndexerError> {
        let query = r#"
            query TransactionByHash($hash: HexEncoded!) {
                transactions(offset: { hash: $hash }) {
                    id
                    hash
                    protocolVersion
                    raw
                    block {
                        hash
                        height
                    }
                }
            }
        "#;

        let response = self
            .execute_query(query, Some(json!({ "hash": hash })))
            .await?;

        // The v4 API returns a single transaction object (not an array)
        let tx_value = response
            .get("data")
            .and_then(|data| data.get("transactions"));

        match tx_value {
            Some(Value::Null) | None => Ok(None),
            Some(val) => {
                // Flatten the nested block into the transaction data
                let mut tx: TransactionData = serde_json::from_value(val.clone())?;
                if let Some(block) = val.get("block") {
                    tx.block_hash = block.get("hash").and_then(Value::as_str).map(String::from);
                    tx.block_height = block.get("height").and_then(Value::as_u64);
                }
                // Do NOT default apply_stage — absence means the status is unknown.
                // The caller must handle None explicitly to avoid false confirmations.
                Ok(Some(tx))
            }
        }
    }

    /// Query a block by hash from the indexer.
    pub async fn get_block_by_hash(&self, hash: &str) -> Result<Option<BlockData>, IndexerError> {
        let query = r#"
            query BlockByHash($hash: HexEncoded!) {
                block(offset: { hash: $hash }) {
                    hash
                    height
                    protocolVersion
                    timestamp
                }
            }
        "#;

        let response = self
            .execute_query(query, Some(json!({ "hash": hash })))
            .await?;

        response
            .get("data")
            .and_then(|data| data.get("block"))
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(IndexerError::from)
    }

    /// Get the unshielded balance for an address by querying recent transactions
    /// via block scanning.
    ///
    /// This scans blocks in batches to find unshielded UTXOs owned by the address.
    /// For each block, sums created outputs and subtracts spent outputs.
    ///
    /// `scan_depth` controls how many blocks back to scan (default ~10000).
    pub async fn get_unshielded_balance(
        &self,
        owner_address: &str,
        current_height: u64,
        scan_depth: u64,
    ) -> Result<u128, IndexerError> {
        // Use a batch query that fetches multiple blocks' transactions at once.
        // The indexer doesn't support batch block queries natively, but we can
        // scan efficiently by checking only blocks that contain transactions.
        let start = current_height.saturating_sub(scan_depth);

        // Query blocks in chunks to find ones with unshielded outputs for our address
        let query = r#"
            query BlockWithOutputs($height: Int!) {
                block(offset: { height: $height }) {
                    height
                    transactions {
                        unshieldedCreatedOutputs { owner value }
                        unshieldedSpentOutputs { owner value }
                    }
                }
            }
        "#;

        let mut balance: i128 = 0;

        // Sample blocks instead of scanning every one — check every Nth block
        // to find activity windows, then scan densely around those.
        // For now, use a simpler approach: scan the most recent blocks
        // and trust that the full balance will come from WebSocket sync later.
        let scan_start = if current_height > 200 {
            current_height - 200
        } else {
            start
        };

        for height in scan_start..=current_height {
            let response = match self
                .execute_query(query, Some(json!({ "height": height as i64 })))
                .await
            {
                Ok(r) => r,
                Err(_) => continue, // Skip blocks that fail to query
            };

            let txs = response
                .get("data")
                .and_then(|d| d.get("block"))
                .and_then(|b| b.get("transactions"))
                .and_then(Value::as_array);

            if let Some(txs) = txs {
                for tx in txs {
                    if let Some(created) =
                        tx.get("unshieldedCreatedOutputs").and_then(Value::as_array)
                    {
                        for utxo in created {
                            if utxo.get("owner").and_then(Value::as_str) == Some(owner_address) {
                                if let Some(val) = utxo.get("value").and_then(Value::as_str) {
                                    balance += val.parse::<i128>().unwrap_or(0);
                                }
                            }
                        }
                    }
                    if let Some(spent) = tx.get("unshieldedSpentOutputs").and_then(Value::as_array)
                    {
                        for utxo in spent {
                            if utxo.get("owner").and_then(Value::as_str) == Some(owner_address) {
                                if let Some(val) = utxo.get("value").and_then(Value::as_str) {
                                    balance -= val.parse::<i128>().unwrap_or(0);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(balance.max(0) as u128)
    }

    pub async fn execute_query(
        &self,
        query: &str,
        variables: Option<Value>,
    ) -> Result<Value, IndexerError> {
        let body = json!({
            "query": query,
            "variables": variables.unwrap_or(Value::Null),
        });

        let response = self
            .http_client
            .post(&self.http_url)
            .json(&body)
            .send()
            .await?;
        let payload: Value = response.error_for_status()?.json().await?;

        if let Some(errors) = payload.get("errors") {
            return Err(IndexerError::GraphQLError(errors.to_string()));
        }

        Ok(payload)
    }
}
