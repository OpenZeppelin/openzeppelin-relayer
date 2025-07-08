//! Types for GraphQL indexer integration with session management

use serde::{Deserialize, Serialize};

/// Transaction application stage from the indexer
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum ApplyStage {
    /// Transaction is still pending
    Pending,
    /// Transaction succeeded entirely
    SucceedEntirely,
    /// Transaction succeeded partially
    SucceedPartially,
    /// Transaction failed entirely
    FailEntirely,
}

impl ApplyStage {
    /// Check if the transaction should be applied to the wallet state
    pub fn should_apply(&self) -> bool {
        matches!(
            self,
            ApplyStage::SucceedEntirely | ApplyStage::SucceedPartially
        )
    }
}

/// Transaction data from the indexer containing transaction details and application status.
///
/// This struct represents a transaction as returned by the indexer, including its hash, optional identifiers,
/// raw data, application stage, Merkle tree root, and protocol version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionData {
    /// The transaction hash.
    pub hash: String,
    /// Optional list of identifiers associated with the transaction.
    pub identifiers: Option<Vec<String>>,
    /// Optional raw transaction data as a hex string.
    pub raw: Option<String>,
    /// The application stage of the transaction (pending, succeeded, failed, etc.).
    #[serde(rename = "applyStage")]
    pub apply_stage: Option<ApplyStage>,
    /// Optional Merkle tree root associated with the transaction.
    #[serde(rename = "merkleTreeRoot")]
    pub merkle_tree_root: Option<String>,
    /// Optional protocol version for the transaction.
    #[serde(rename = "protocolVersion")]
    pub protocol_version: Option<u32>,
}

/// Information about a collapsed Merkle tree update from the indexer.
///
/// This struct contains details about a Merkle tree update, including the blockchain index, protocol version,
/// start and end indices, and the update data as a string.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollapsedUpdateInfo {
    /// The blockchain index at which the update occurred.
    pub blockchain_index: u64,
    /// The protocol version for the update.
    pub protocol_version: u32,
    /// The start index of the update range.
    pub start: u64,
    /// The end index of the update range.
    pub end: u64,
    /// The update data as a string (typically hex or base64 encoded).
    pub update_data: String,
}

/// Events emitted during wallet synchronization via GraphQL subscription.
///
/// This enum represents the different event types that can be received from the indexer during wallet sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WalletSyncEvent {
    /// A viewing update event, containing Merkle updates and/or relevant transactions.
    ViewingUpdate {
        #[serde(rename = "__typename")]
        type_name: String,
        /// The blockchain index for this update.
        index: u64,
        /// The list of Zswap chain state updates (transactions or Merkle updates).
        update: Vec<ZswapChainStateUpdate>,
    },
    /// A progress update event, reporting sync progress indices.
    ProgressUpdate {
        #[serde(rename = "__typename")]
        type_name: String,
        /// The highest blockchain index seen.
        #[serde(rename = "highestIndex")]
        highest_index: u64,
        /// The highest relevant index for the wallet.
        #[serde(rename = "highestRelevantIndex")]
        highest_relevant_index: u64,
        /// The highest relevant wallet index.
        #[serde(rename = "highestRelevantWalletIndex")]
        highest_relevant_wallet_index: u64,
    },
}

/// Updates to the Zswap chain state, including transactions and Merkle tree updates.
///
/// This enum represents either a relevant transaction or a Merkle tree collapsed update.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "__typename")]
pub enum ZswapChainStateUpdate {
    /// A relevant transaction update.
    RelevantTransaction {
        /// The transaction data.
        transaction: TransactionData,
        /// The start index for the transaction (optional, defaults to 0).
        #[serde(default)]
        start: u64,
        /// The end index for the transaction (optional, defaults to 0).
        #[serde(default)]
        end: u64,
    },
    /// A collapsed Merkle tree update.
    MerkleTreeCollapsedUpdate {
        /// The protocol version for the update (optional, defaults to 0).
        #[serde(rename = "protocolVersion", default)]
        protocol_version: u32,
        /// The start index of the update range (optional, defaults to 0).
        #[serde(default)]
        start: u64,
        /// The end index of the update range (optional, defaults to 0).
        #[serde(default)]
        end: u64,
        /// The update data as a string (optional, defaults to empty string).
        #[serde(default)]
        update: String,
    },
}

/// Formats for wallet viewing keys used to query the indexer.
///
/// This enum represents the supported viewing key formats for wallet queries.
#[derive(Debug, Clone)]
pub enum ViewingKeyFormat {
    /// Bech32m format (preferred): mn_shield-esk_dev1...
    Bech32m(String),
}

impl ViewingKeyFormat {
    /// Get the viewing key as a string for API calls
    pub fn as_str(&self) -> &str {
        match self {
            ViewingKeyFormat::Bech32m(key) => key,
        }
    }
}

/// Error types for indexer operations and session management
#[derive(Debug, thiserror::Error)]
pub enum IndexerError {
    #[error("GraphQL error: {0}")]
    GraphQLError(String),

    #[error("No data returned")]
    NoData,

    #[error("WebSocket error: {0}")]
    WebSocketError(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("JSON parse error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Session error: {0}")]
    SessionError(String),
}
