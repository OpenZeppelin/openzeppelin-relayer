//! Subxt-based client for Midnight transaction submission.
//!
//! Uses the `midnight-node-metadata` crate to create properly formatted
//! unsigned extrinsics for the Midnight pallet's `send_mn_transaction` call.

use midnight_node_metadata::midnight_metadata_latest as mn_meta;
use subxt::{OnlineClient, PolkadotConfig};
use tracing::{debug, info};

use super::TransactionSubmissionResult;
use crate::services::provider::ProviderError;

/// Wraps a Subxt `OnlineClient` for Midnight-specific operations.
pub struct MidnightSubxtClient {
    api: OnlineClient<PolkadotConfig>,
}

impl MidnightSubxtClient {
    /// Connect to a Midnight node via WebSocket.
    pub async fn connect(rpc_url: &str) -> Result<Self, ProviderError> {
        let api = OnlineClient::<PolkadotConfig>::from_url(rpc_url)
            .await
            .map_err(|e| {
                ProviderError::NetworkConfiguration(format!(
                    "Failed to connect Subxt to {rpc_url}: {e}"
                ))
            })?;

        info!(rpc_url, "Subxt client connected to Midnight node");
        Ok(Self { api })
    }

    /// Submit a serialized Midnight transaction as an unsigned extrinsic.
    ///
    /// `serialized_tx` is the raw transaction bytes (output of
    /// `midnight_node_ledger_helpers::serialize()`), NOT hex-encoded.
    pub async fn submit_transaction(
        &self,
        serialized_tx: Vec<u8>,
    ) -> Result<TransactionSubmissionResult, ProviderError> {
        // Create the pallet call: midnight.send_mn_transaction(tx_bytes)
        let mn_tx = mn_meta::tx().midnight().send_mn_transaction(serialized_tx);

        // Create an unsigned extrinsic
        let unsigned_extrinsic = self.api.tx().create_unsigned(&mn_tx).map_err(|e| {
            ProviderError::Other(format!("Failed to create unsigned extrinsic: {e}"))
        })?;

        debug!("Submitting unsigned Midnight extrinsic");

        // Submit and get the extrinsic hash
        let tx_hash = unsigned_extrinsic
            .submit()
            .await
            .map_err(|e| ProviderError::Other(format!("Extrinsic submission failed: {e}")))?;

        let extrinsic_hash = format!("0x{}", hex::encode(tx_hash.0));

        info!(extrinsic_hash = %extrinsic_hash, "Midnight extrinsic submitted");

        Ok(TransactionSubmissionResult {
            extrinsic_tx_hash: extrinsic_hash,
            pallet_tx_hash: None, // Computed separately from the transaction hash
        })
    }

    /// Get the Subxt API client (for reading state, etc.).
    pub fn api(&self) -> &OnlineClient<PolkadotConfig> {
        &self.api
    }
}
