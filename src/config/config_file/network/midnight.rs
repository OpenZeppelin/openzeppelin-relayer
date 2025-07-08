//! Midnight Network Configuration
//!
//! This module provides configuration support for Midnight blockchain networks.

use super::common::NetworkConfigCommon;
use crate::config::ConfigFileError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Hash)]
pub struct IndexerUrls {
    pub http: String,
    pub ws: String,
}

/// Configuration specific to Midnight networks.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MidnightNetworkConfig {
    /// Common network fields.
    #[serde(flatten)]
    pub common: NetworkConfigCommon,
    // Midnight-specific fields
    pub indexer_urls: IndexerUrls, // URL for the indexer server (ws, http)
    pub prover_url: String,        // URL for the prover server
    pub commitment_tree_ttl: Option<u64>, // How long to cache Merkle roots
}

impl MidnightNetworkConfig {
    /// Validates the specific configuration fields for a Midnight network.
    ///
    /// # Returns
    /// - `Ok(())` if the Midnight configuration is valid.
    /// - `Err(ConfigFileError)` if validation fails (e.g., missing fields, invalid URLs).
    pub fn validate(&self) -> Result<(), ConfigFileError> {
        self.common.validate()?;

        // Validate indexer URLs
        reqwest::Url::parse(&self.indexer_urls.http).map_err(|_| {
            ConfigFileError::InvalidFormat(format!(
                "Invalid indexer HTTP URL: {}",
                self.indexer_urls.http
            ))
        })?;

        reqwest::Url::parse(&self.indexer_urls.ws).map_err(|_| {
            ConfigFileError::InvalidFormat(format!(
                "Invalid indexer WebSocket URL: {}",
                self.indexer_urls.ws
            ))
        })?;

        // Validate prover URL if provided
        reqwest::Url::parse(&self.prover_url).map_err(|_| {
            ConfigFileError::InvalidFormat(format!("Invalid prover URL: {}", self.prover_url))
        })?;

        // Validate network_id if provided
        match self.common.network.as_str() {
            "mainnet" | "testnet" | "devnet" => {}
            _ => {
                return Err(ConfigFileError::InvalidFormat(format!(
                    "Invalid network_id: {}. Must be one of: mainnet, testnet, devnet",
                    self.common.network
                )))
            }
        }

        // Validate commitment_tree_ttl is reasonable if provided
        if let Some(ttl) = self.commitment_tree_ttl {
            if ttl == 0 {
                return Err(ConfigFileError::InvalidFormat(
                    "commitment_tree_ttl must be greater than 0".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Merges this Midnight configuration with a parent Midnight configuration.
    /// Parent values are used as defaults, child values take precedence.
    pub fn merge_with_parent(&self, parent: &Self) -> Self {
        Self {
            common: self.common.merge_with_parent(&parent.common),
            indexer_urls: self.indexer_urls.clone(),
            prover_url: self.prover_url.clone(),
            commitment_tree_ttl: self.commitment_tree_ttl.or(parent.commitment_tree_ttl),
        }
    }
}
