use crate::models::{NetworkConfigData, NetworkRepoModel, RepositoryError};
use std::time::Duration;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct EvmNetwork {
    // Common network fields (flattened from NetworkConfigCommon)
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
    /// The unique chain identifier (Chain ID) for the EVM network.
    pub chain_id: u64,
    /// Number of block confirmations required before a transaction is considered final.
    pub required_confirmations: u64,
    /// List of specific features supported by the network (e.g., "eip1559").
    pub features: Vec<String>,
    /// The symbol of the network's native currency (e.g., "ETH", "MATIC").
    pub symbol: String,
}

impl TryFrom<NetworkRepoModel> for EvmNetwork {
    type Error = RepositoryError;

    /// Converts a NetworkRepoModel to an EvmNetwork.
    ///
    /// # Arguments
    /// * `network_repo` - The repository model to convert
    ///
    /// # Returns
    /// Result containing the EvmNetwork if successful, or a RepositoryError
    fn try_from(network_repo: NetworkRepoModel) -> Result<Self, Self::Error> {
        match &network_repo.config {
            NetworkConfigData::Evm(evm_config) => {
                let common = &evm_config.common;

                let chain_id = evm_config.chain_id.ok_or_else(|| {
                    RepositoryError::InvalidData(format!(
                        "EVM network '{}' has no chain_id",
                        network_repo.name
                    ))
                })?;

                let required_confirmations =
                    evm_config.required_confirmations.ok_or_else(|| {
                        RepositoryError::InvalidData(format!(
                            "EVM network '{}' has no required_confirmations",
                            network_repo.name
                        ))
                    })?;

                let symbol = evm_config.symbol.clone().ok_or_else(|| {
                    RepositoryError::InvalidData(format!(
                        "EVM network '{}' has no symbol",
                        network_repo.name
                    ))
                })?;

                let average_blocktime_ms = common.average_blocktime_ms.ok_or_else(|| {
                    RepositoryError::InvalidData(format!(
                        "EVM network '{}' has no average_blocktime_ms",
                        network_repo.name
                    ))
                })?;

                Ok(EvmNetwork {
                    network: common.network.clone(),
                    rpc_urls: common.rpc_urls.clone().unwrap_or_default(),
                    explorer_urls: common.explorer_urls.clone(),
                    average_blocktime_ms,
                    is_testnet: common.is_testnet.unwrap_or(false),
                    tags: common.tags.clone().unwrap_or_default(),
                    chain_id,
                    required_confirmations,
                    features: evm_config.features.clone().unwrap_or_default(),
                    symbol,
                })
            }
            _ => Err(RepositoryError::InvalidData(format!(
                "Network '{}' is not an EVM network",
                network_repo.name
            ))),
        }
    }
}

impl EvmNetwork {
    pub fn is_optimism(&self) -> bool {
        self.tags.contains(&"optimism".to_string())
    }

    pub fn is_rollup(&self) -> bool {
        self.tags.contains(&"rollup".to_string())
    }

    pub fn is_testnet(&self) -> bool {
        self.is_testnet
    }

    /// Returns the recommended number of confirmations needed for this network.
    pub fn required_confirmations(&self) -> u64 {
        self.required_confirmations
    }

    pub fn id(&self) -> u64 {
        self.chain_id
    }

    pub fn average_blocktime(&self) -> Option<Duration> {
        Some(Duration::from_millis(self.average_blocktime_ms))
    }

    pub fn is_legacy(&self) -> bool {
        !self.features.contains(&"eip1559".to_string())
    }

    pub fn explorer_urls(&self) -> Option<&[String]> {
        self.explorer_urls.as_deref()
    }

    pub fn public_rpc_urls(&self) -> Option<&[String]> {
        if self.rpc_urls.is_empty() {
            None
        } else {
            Some(&self.rpc_urls)
        }
    }
}
