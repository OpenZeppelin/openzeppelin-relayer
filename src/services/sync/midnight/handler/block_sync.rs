//! Block-by-block LedgerState synchronization.
//!
//! Processes blocks from genesis (or cached checkpoint) to build the
//! complete LedgerState including DUST merkle trees.

use std::sync::Arc;

use midnight_node_ledger_helpers::{
    make_block_context, DefaultDB, HashOutput, LedgerContext, ProofMarker, PureGeneratorPedersen,
    Signature, Timestamp, Transaction,
};
use midnight_node_metadata::midnight_metadata_latest as mn_meta;
use subxt::{OnlineClient, PolkadotConfig};
use tracing::{debug, info, warn};

/// The finalized transaction type from the chain (implements Tagged).
type FinalizedTx = Transaction<Signature, ProofMarker, PureGeneratorPedersen, DefaultDB>;

/// SerdeTransaction wrapper type.
type SerdeTx = midnight_node_ledger_helpers::SerdeTransaction<Signature, ProofMarker, DefaultDB>;

/// Sync the LedgerContext by processing blocks from the chain.
///
/// Returns the number of blocks processed and transactions applied.
pub async fn sync_blocks(
    context: &Arc<LedgerContext<DefaultDB>>,
    api: &OnlineClient<PolkadotConfig>,
    rpc_url: &str,
    start_height: u64,
    end_height: u64,
) -> Result<BlockSyncResult, BlockSyncError> {
    let mut result = BlockSyncResult::default();

    // Process genesis block specially if starting from 0
    if start_height == 0 {
        info!("Processing genesis block");
        if let Err(e) = process_genesis(context, rpc_url).await {
            warn!(error = %e, "Genesis block processing failed");
        }
        result.blocks_processed += 1;
    }

    let actual_start = if start_height == 0 { 1 } else { start_height };
    let batch_size = 500;
    let mut current = actual_start;

    while current <= end_height {
        let batch_end = (current + batch_size - 1).min(end_height);

        for height in current..=batch_end {
            match process_block(context, api, rpc_url, height).await {
                Ok(tx_count) => {
                    result.blocks_processed += 1;
                    result.transactions_applied += tx_count;
                }
                Err(e) => {
                    // Don't log every skip — most blocks have no Midnight txs
                    result.blocks_skipped += 1;
                }
            }
        }

        info!(
            height = batch_end,
            processed = result.blocks_processed,
            txs = result.transactions_applied,
            skipped = result.blocks_skipped,
            "Block sync progress"
        );

        current = batch_end + 1;
    }

    Ok(result)
}

async fn get_block_hash(rpc_url: &str, height: u64) -> Result<subxt::utils::H256, BlockSyncError> {
    let client = reqwest::Client::new();
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "chain_getBlockHash",
        "params": [height],
    });
    let resp: serde_json::Value = client
        .post(rpc_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| BlockSyncError::RpcError(format!("hash request: {e}")))?
        .json()
        .await
        .map_err(|e| BlockSyncError::RpcError(format!("hash json: {e}")))?;

    let hash_hex = resp
        .get("result")
        .and_then(|r| r.as_str())
        .ok_or_else(|| BlockSyncError::RpcError("no hash result".into()))?;

    let bytes = hex::decode(hash_hex.trim_start_matches("0x"))
        .map_err(|e| BlockSyncError::RpcError(format!("hash hex: {e}")))?;
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes[..32]);
    Ok(subxt::utils::H256(arr))
}

async fn process_genesis(
    context: &Arc<LedgerContext<DefaultDB>>,
    rpc_url: &str,
) -> Result<(), BlockSyncError> {
    // Get genesis state from system properties via JSON-RPC
    let client = reqwest::Client::new();
    let resp: serde_json::Value = client
        .post(rpc_url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "system_properties", "params": [],
        }))
        .send()
        .await
        .map_err(|e| BlockSyncError::RpcError(format!("system_properties: {e}")))?
        .json()
        .await
        .map_err(|e| BlockSyncError::RpcError(format!("json: {e}")))?;

    let props = resp
        .get("result")
        .cloned()
        .unwrap_or(serde_json::Value::Object(Default::default()));

    if let Some(genesis_state_hex) = props.get("genesis_state").and_then(|v| v.as_str()) {
        let genesis_bytes = hex::decode(genesis_state_hex)
            .map_err(|e| BlockSyncError::RpcError(format!("genesis hex: {e}")))?;
        context.update_ledger_state_from_bytes(&genesis_bytes);
        info!(bytes = genesis_bytes.len(), "Genesis state applied");
    }

    Ok(())
}

async fn process_block(
    context: &Arc<LedgerContext<DefaultDB>>,
    api: &OnlineClient<PolkadotConfig>,
    rpc_url: &str,
    height: u64,
) -> Result<u64, BlockSyncError> {
    let hash = get_block_hash(rpc_url, height).await?;

    let block = api
        .blocks()
        .at(hash)
        .await
        .map_err(|e| BlockSyncError::RpcError(format!("block at {height}: {e}")))?;

    // Extract Midnight transactions from extrinsics
    let extrinsics = block
        .extrinsics()
        .await
        .map_err(|e| BlockSyncError::RpcError(format!("extrinsics: {e}")))?;

    let mut midnight_txs: Vec<SerdeTx> = Vec::new();

    for ext in extrinsics.iter() {
        let call = match ext.as_root_extrinsic::<mn_meta::Call>() {
            Ok(c) => c,
            Err(_) => continue,
        };

        if let mn_meta::Call::Midnight(mn_meta::midnight::Call::send_mn_transaction {
            midnight_tx,
        }) = &call
        {
            let tx_result: Result<FinalizedTx, _> =
                midnight_node_ledger_helpers::deserialize(&mut &midnight_tx[..]);
            match tx_result {
                Ok(tx) => midnight_txs.push(SerdeTx::Midnight(tx)),
                Err(_) => {} // Skip unparsable txs
            }
        }
    }

    if midnight_txs.is_empty() {
        return Ok(0);
    }

    // Build block context
    let header = block.header();
    let parent_hash_bytes: [u8; 32] = header.parent_hash.0;

    // Extract timestamp from Timestamp::set extrinsic
    let mut timestamp_ms: Option<u64> = None;
    for ext in extrinsics.iter() {
        if let Ok(mn_meta::Call::Timestamp(mn_meta::timestamp::Call::set { now })) =
            ext.as_root_extrinsic::<mn_meta::Call>()
        {
            timestamp_ms = Some(now);
            break;
        }
    }

    let tblock = Timestamp::from_secs(timestamp_ms.unwrap_or(0) / 1000);
    let block_context = make_block_context(
        tblock,
        HashOutput(parent_hash_bytes),
        tblock, // last_block_time approximation
    );

    context.update_from_block(&midnight_txs, &block_context, None, None);

    Ok(midnight_txs.len() as u64)
}

#[derive(Debug, Clone, Default)]
pub struct BlockSyncResult {
    pub blocks_processed: u64,
    pub blocks_skipped: u64,
    pub transactions_applied: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum BlockSyncError {
    #[error("RPC error: {0}")]
    RpcError(String),
}
