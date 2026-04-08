use serde::{Deserialize, Serialize};

/// Transaction status as determined by the indexer.
///
/// NOTE: The v4 indexer no longer exposes `applyStage` directly on the
/// `Transaction` type. Status is inferred from the presence/absence of
/// `zswapLedgerEvents` and `dustLedgerEvents`. This enum is used
/// internally for the status check flow and may be populated from
/// different sources depending on the indexer version.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum ApplyStage {
    Pending,
    SucceedEntirely,
    SucceedPartially,
    FailEntirely,
}

impl ApplyStage {
    pub fn should_apply(&self) -> bool {
        matches!(self, Self::SucceedEntirely | Self::SucceedPartially)
    }
}

/// Transaction data returned by the indexer.
///
/// Matches the v4 GraphQL `Transaction` type schema:
/// - `id`: Int! (auto-incrementing indexer ID)
/// - `hash`: HexEncoded!
/// - `protocolVersion`: Int!
/// - `raw`: HexEncoded!
/// - `block`: Block! (nested, we flatten to block_hash/block_height)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransactionData {
    pub hash: String,
    /// Indexer-assigned sequential ID
    pub id: Option<i64>,
    #[serde(rename = "protocolVersion")]
    pub protocol_version: Option<u32>,
    pub raw: Option<String>,
    /// Inferred status — not a direct indexer field in v4
    #[serde(rename = "applyStage")]
    pub apply_stage: Option<ApplyStage>,
    /// Block hash (populated from nested `block { hash }` query)
    #[serde(rename = "blockHash")]
    pub block_hash: Option<String>,
    /// Block height (populated from nested `block { height }` query)
    #[serde(rename = "blockHeight")]
    pub block_height: Option<u64>,
}

/// Block data returned by the indexer.
///
/// Matches the v4 GraphQL `Block` type: `hash: HexEncoded!`, `height: Int!`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlockData {
    pub hash: String,
    pub height: Option<u64>,
    #[serde(rename = "protocolVersion")]
    pub protocol_version: Option<u32>,
    pub timestamp: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CollapsedUpdateInfo {
    pub blockchain_index: u64,
    pub protocol_version: u32,
    pub start: u64,
    pub end: u64,
    pub update_data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum WalletSyncEvent {
    ViewingUpdate {
        #[serde(rename = "__typename")]
        type_name: String,
        index: u64,
        update: Vec<ZswapChainStateUpdate>,
    },
    ProgressUpdate {
        #[serde(rename = "__typename")]
        type_name: String,
        #[serde(rename = "highestIndex")]
        highest_index: u64,
        #[serde(rename = "highestRelevantIndex")]
        highest_relevant_index: u64,
        #[serde(rename = "highestRelevantWalletIndex")]
        highest_relevant_wallet_index: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "__typename")]
pub enum ZswapChainStateUpdate {
    RelevantTransaction {
        transaction: TransactionData,
        #[serde(default)]
        start: u64,
        #[serde(default)]
        end: u64,
    },
    MerkleTreeCollapsedUpdate {
        #[serde(rename = "protocolVersion", default)]
        protocol_version: u32,
        #[serde(default)]
        start: u64,
        #[serde(default)]
        end: u64,
        #[serde(default)]
        update: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewingKeyFormat {
    Bech32m(String),
}

impl ViewingKeyFormat {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Bech32m(key) => key,
        }
    }
}

/// An unshielded UTXO as returned by the indexer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnshieldedUtxo {
    pub owner: String,
    #[serde(rename = "tokenType")]
    pub token_type: String,
    pub value: String,
    #[serde(rename = "outputIndex")]
    pub output_index: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum IndexerError {
    #[error("GraphQL error: {0}")]
    GraphQLError(String),
    #[error("No data returned from indexer")]
    NoData,
    #[error("HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),
    #[error("JSON parse error: {0}")]
    JsonError(#[from] serde_json::Error),
}
