use crate::{
    config::network::IndexerUrls,
    models::{NetworkConfigData, NetworkRepoModel, RepositoryError},
};
use core::time::Duration;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Debug)]
pub struct MidnightNetwork {
    /// Unique network identifier (e.g., "mainnet", "sepolia", "custom-devnet").
    pub network: String,
    /// List of RPC endpoint URLs for connecting to the network.
    pub rpc_urls: Vec<String>,
    /// List of Explorer endpoint URLs for connecting to the network.
    pub explorer_urls: Option<Vec<String>>,
    /// Estimated average time between blocks in milliseconds.
    pub average_blocktime_ms: u64,
    /// Flag indicating if the network is a testnet.
    pub is_testnet: bool,
    /// List of arbitrary tags for categorizing or filtering networks.
    pub tags: Vec<String>,
    /// List of Indexer endpoint URLs for connecting to the network.
    pub indexer_urls: IndexerUrls,
    /// URL of the prover server for generating proofs.
    pub prover_url: String,
}

impl TryFrom<NetworkRepoModel> for MidnightNetwork {
    type Error = RepositoryError;

    /// Converts a NetworkRepoModel to a MidnightNetwork.
    ///
    /// # Arguments
    /// * `network_repo` - The repository model to convert
    ///
    /// # Returns
    /// Result containing the MidnightNetwork if successful, or a RepositoryError
    fn try_from(network_repo: NetworkRepoModel) -> Result<Self, Self::Error> {
        match &network_repo.config {
            NetworkConfigData::Midnight(midnight_config) => {
                let common = &midnight_config.common;

                let rpc_urls = common.rpc_urls.clone().ok_or_else(|| {
                    RepositoryError::InvalidData(format!(
                        "Midnight network '{}' has no rpc_urls",
                        network_repo.name
                    ))
                })?;

                let average_blocktime_ms = common.average_blocktime_ms.ok_or_else(|| {
                    RepositoryError::InvalidData(format!(
                        "Midnight network '{}' has no average_blocktime_ms",
                        network_repo.name
                    ))
                })?;

                Ok(MidnightNetwork {
                    network: common.network.clone(),
                    rpc_urls,
                    explorer_urls: common.explorer_urls.clone(),
                    average_blocktime_ms,
                    is_testnet: common.is_testnet.unwrap_or(false),
                    tags: common.tags.clone().unwrap_or_default(),
                    indexer_urls: midnight_config.indexer_urls.clone(),
                    prover_url: midnight_config.prover_url.clone(),
                })
            }
            _ => Err(RepositoryError::InvalidData(format!(
                "Network '{}' is not a Midnight network",
                network_repo.name
            ))),
        }
    }
}

impl MidnightNetwork {
    pub fn average_blocktime(&self) -> Option<Duration> {
        Some(Duration::from_millis(self.average_blocktime_ms))
    }

    pub fn public_rpc_urls(&self) -> Option<&[String]> {
        if self.rpc_urls.is_empty() {
            None
        } else {
            Some(&self.rpc_urls)
        }
    }

    pub fn explorer_urls(&self) -> Option<&[String]> {
        self.explorer_urls.as_deref()
    }

    pub fn is_testnet(&self) -> bool {
        self.is_testnet
    }
}
