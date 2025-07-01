//! Midnight Network Configuration
//!
//! This module provides configuration support for Midnight blockchain networks.

use super::common::NetworkConfigCommon;
use crate::config::ConfigFileError;
use serde::{Deserialize, Serialize};

/// Configuration specific to Midnight networks.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MidnightNetworkConfig {
    /// Common network fields.
    #[serde(flatten)]
    pub common: NetworkConfigCommon,
    // Midnight-specific fields
    pub indexer_url: String,              // URL for the indexer server
    pub prover_url: Option<String>,       // URL for the prover server
    pub commitment_tree_ttl: Option<u64>, // How long to cache Merkle roots
    pub network_id: Option<String>,       // mainnet, testnet, devnet
}

impl MidnightNetworkConfig {
    /// Validates the specific configuration fields for a Midnight network.
    ///
    /// # Returns
    /// - `Ok(())` if the Midnight configuration is valid.
    /// - `Err(ConfigFileError)` if validation fails (e.g., missing fields, invalid URLs).
    pub fn validate(&self) -> Result<(), ConfigFileError> {
        self.common.validate()?;
        Ok(())
    }

    /// Merges this Midnight configuration with a parent Midnight configuration.
    /// Parent values are used as defaults, child values take precedence.
    pub fn merge_with_parent(&self, parent: &Self) -> Self {
        Self {
            common: self.common.merge_with_parent(&parent.common),
            indexer_url: self.indexer_url.clone(),
            prover_url: self.prover_url.clone().or(parent.prover_url.clone()),
            commitment_tree_ttl: self.commitment_tree_ttl.or(parent.commitment_tree_ttl),
            network_id: self.network_id.clone().or(parent.network_id.clone()),
        }
    }
}
