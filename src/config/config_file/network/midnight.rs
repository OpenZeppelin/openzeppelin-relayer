//! Midnight Network Configuration
//!
//! This module is gated behind the `midnight` Cargo feature so default builds do
//! not pull Midnight-only configuration surface or dependencies.

use super::common::NetworkConfigCommon;
use crate::config::ConfigFileError;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Hash, ToSchema)]
pub struct IndexerUrls {
    pub http: String,
    pub ws: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MidnightNetworkConfig {
    #[serde(flatten)]
    pub common: NetworkConfigCommon,
    pub indexer_urls: IndexerUrls,
    pub prover_url: String,
    pub commitment_tree_ttl: Option<u64>,
}

impl MidnightNetworkConfig {
    pub fn validate(&self) -> Result<(), ConfigFileError> {
        self.common.validate()?;

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

        reqwest::Url::parse(&self.prover_url).map_err(|_| {
            ConfigFileError::InvalidFormat(format!("Invalid prover URL: {}", self.prover_url))
        })?;

        match self.common.network.as_str() {
            "preview" | "preprod" | "testnet" | "mainnet" | "devnet" => {}
            _ => {
                return Err(ConfigFileError::InvalidFormat(format!(
                    "Invalid network_id: {}. Must be one of: preview, preprod, testnet, mainnet, devnet",
                    self.common.network
                )))
            }
        }

        if self.commitment_tree_ttl == Some(0) {
            return Err(ConfigFileError::InvalidFormat(
                "commitment_tree_ttl must be greater than 0".to_string(),
            ));
        }

        Ok(())
    }

    pub fn merge_with_parent(&self, parent: &Self) -> Self {
        Self {
            common: self.common.merge_with_parent(&parent.common),
            indexer_urls: self.indexer_urls.clone(),
            prover_url: self.prover_url.clone(),
            commitment_tree_ttl: self.commitment_tree_ttl.or(parent.commitment_tree_ttl),
        }
    }
}
