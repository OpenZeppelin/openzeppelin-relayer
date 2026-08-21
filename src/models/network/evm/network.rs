use crate::config::GasPriceCacheConfig;
use crate::constants::{
    ARBITRUM_BASED_TAG, DEFAULT_EVM_STATUS_CHECK_INITIAL_DELAY_SECONDS, LACKS_MEMPOOL_TAGS,
    MAX_EVM_STATUS_CHECK_DELAY_SECONDS, MIN_EVM_STATUS_CHECK_INITIAL_DELAY_SECONDS,
    MIN_EVM_STATUS_CHECK_RETRY_DELAY_SECONDS, OPTIMISM_BASED_TAG, OPTIMISM_TAG, POLYGON_ZKEVM_TAG,
    ROLLUP_TAG,
};
use crate::models::{NetworkConfigData, NetworkRepoModel, RepositoryError, RpcConfig};
use std::time::Duration;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct EvmNetwork {
    // Common network fields (flattened from NetworkConfigCommon)
    /// Unique network identifier (e.g., "mainnet", "sepolia", "custom-devnet").
    pub network: String,
    /// List of RPC endpoint configurations for connecting to the network.
    pub rpc_urls: Vec<RpcConfig>,
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
    /// Delay before the first transaction status check, in seconds.
    pub status_check_initial_delay_seconds: i64,
    /// Optional delay between successful checks while the transaction is not final, in seconds.
    pub status_check_retry_delay_seconds: Option<u64>,
    /// List of specific features supported by the network (e.g., "eip1559").
    pub features: Vec<String>,
    /// The symbol of the network's native currency (e.g., "ETH", "MATIC").
    pub symbol: String,
    /// Gas price cache configuration
    pub gas_price_cache: Option<GasPriceCacheConfig>,
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

                let configured_delay = evm_config
                    .status_check
                    .as_ref()
                    .and_then(|config| config.initial_delay_seconds)
                    .unwrap_or(DEFAULT_EVM_STATUS_CHECK_INITIAL_DELAY_SECONDS);
                if !(MIN_EVM_STATUS_CHECK_INITIAL_DELAY_SECONDS
                    ..=MAX_EVM_STATUS_CHECK_DELAY_SECONDS)
                    .contains(&configured_delay)
                {
                    return Err(RepositoryError::InvalidData(format!(
                        "EVM network '{}' has an invalid status_check.initial_delay_seconds",
                        network_repo.name
                    )));
                }
                let status_check_initial_delay_seconds =
                    i64::try_from(configured_delay).map_err(|_| {
                        RepositoryError::InvalidData(format!(
                            "EVM network '{}' has an invalid status_check.initial_delay_seconds",
                            network_repo.name
                        ))
                    })?;

                let status_check_retry_delay_seconds = evm_config
                    .status_check
                    .as_ref()
                    .and_then(|config| config.retry_delay_seconds);
                if status_check_retry_delay_seconds.is_some_and(|delay| {
                    !(MIN_EVM_STATUS_CHECK_RETRY_DELAY_SECONDS..=MAX_EVM_STATUS_CHECK_DELAY_SECONDS)
                        .contains(&delay)
                }) {
                    return Err(RepositoryError::InvalidData(format!(
                        "EVM network '{}' has an invalid status_check.retry_delay_seconds",
                        network_repo.name
                    )));
                }

                Ok(EvmNetwork {
                    network: common.network.clone(),
                    rpc_urls: common.rpc_urls.clone().unwrap_or_default(),
                    explorer_urls: common.explorer_urls.clone(),
                    average_blocktime_ms,
                    is_testnet: common.is_testnet.unwrap_or(false),
                    tags: common.tags.clone().unwrap_or_default(),
                    chain_id,
                    required_confirmations,
                    status_check_initial_delay_seconds,
                    status_check_retry_delay_seconds,
                    features: evm_config.features.clone().unwrap_or_default(),
                    symbol,
                    gas_price_cache: evm_config.gas_price_cache.clone(),
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
        self.tags
            .iter()
            .any(|t| t == OPTIMISM_BASED_TAG || t == OPTIMISM_TAG)
    }

    pub fn is_rollup(&self) -> bool {
        self.tags.iter().any(|t| t == ROLLUP_TAG)
    }

    ///  Returns whether this network lacks mempool-like behavior (no public/pending pool).
    ///
    /// Returns true if any tag in `constants::LACKS_MEMPOOL_TAGS` is present.
    /// Currently includes:
    /// - "no-mempool"
    /// - "arbitrum-based"
    /// - "optimism-based"
    /// - "optimism" (deprecated; kept for compatibility)
    pub fn lacks_mempool(&self) -> bool {
        self.tags
            .iter()
            .any(|t| LACKS_MEMPOOL_TAGS.contains(&t.as_str()))
    }

    pub fn is_arbitrum(&self) -> bool {
        self.tags.iter().any(|t| t == ARBITRUM_BASED_TAG)
    }

    pub fn is_polygon_zkevm(&self) -> bool {
        self.tags.iter().any(|t| t == POLYGON_ZKEVM_TAG)
    }

    pub fn is_testnet(&self) -> bool {
        self.is_testnet
    }

    /// Returns the recommended number of confirmations needed for this network.
    pub fn required_confirmations(&self) -> u64 {
        self.required_confirmations
    }

    /// Returns the delay before the first transaction status check, in seconds.
    pub fn status_check_initial_delay_seconds(&self) -> i64 {
        self.status_check_initial_delay_seconds
    }

    /// Returns the configured delay between successful non-final status checks.
    pub fn status_check_retry_delay_seconds(&self) -> Option<u64> {
        self.status_check_retry_delay_seconds
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

    pub fn public_rpc_urls(&self) -> Option<&[RpcConfig]> {
        if self.rpc_urls.is_empty() {
            None
        } else {
            Some(&self.rpc_urls)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EvmNetworkConfig, NetworkConfigCommon, StatusCheckConfig};
    use crate::constants::{NO_MEMPOOL_TAG, OPTIMISM_TAG};
    use crate::models::{NetworkConfigData, NetworkRepoModel, NetworkType, RpcConfig};

    fn create_test_evm_network_with_tags(tags: Vec<&str>) -> EvmNetwork {
        EvmNetwork {
            network: "test-network".to_string(),
            rpc_urls: vec![RpcConfig::new("https://rpc.example.com".to_string())],
            explorer_urls: None,
            average_blocktime_ms: 12000,
            is_testnet: false,
            tags: tags.into_iter().map(|s| s.to_string()).collect(),
            chain_id: 1,
            required_confirmations: 1,
            status_check_initial_delay_seconds: 8,
            status_check_retry_delay_seconds: None,
            features: vec!["eip1559".to_string()],
            symbol: "ETH".to_string(),
            gas_price_cache: None,
        }
    }

    fn create_test_evm_config() -> EvmNetworkConfig {
        EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "test-network".to_string(),
                from: None,
                rpc_urls: Some(vec![RpcConfig::new("https://rpc.example.com".to_string())]),
                explorer_urls: None,
                average_blocktime_ms: Some(12000),
                is_testnet: Some(false),
                tags: Some(vec![ROLLUP_TAG.to_string(), OPTIMISM_BASED_TAG.to_string()]),
            },
            chain_id: Some(10),
            required_confirmations: Some(1),
            status_check: None,
            features: Some(vec!["eip1559".to_string()]),
            symbol: Some("ETH".to_string()),
            gas_price_cache: None,
        }
    }

    fn repo_model(config: EvmNetworkConfig) -> NetworkRepoModel {
        NetworkRepoModel {
            id: "evm:test-network".to_string(),
            name: "test-network".to_string(),
            network_type: NetworkType::Evm,
            config: NetworkConfigData::Evm(config),
        }
    }

    #[test]
    fn test_is_optimism_with_optimism_tag() {
        let network = create_test_evm_network_with_tags(vec![OPTIMISM_BASED_TAG, ROLLUP_TAG]);
        assert!(network.is_optimism());
    }

    #[test]
    fn test_is_optimism_without_optimism_tag() {
        let network = create_test_evm_network_with_tags(vec![ROLLUP_TAG, "mainnet"]);
        assert!(!network.is_optimism());
    }

    #[test]
    fn test_is_optimism_with_deprecated_optimism_tag() {
        let network = create_test_evm_network_with_tags(vec![OPTIMISM_TAG, ROLLUP_TAG]);
        assert!(network.is_optimism());
    }

    #[test]
    fn test_lacks_mempool_with_deprecated_optimism_tag() {
        let network = create_test_evm_network_with_tags(vec![OPTIMISM_TAG, ROLLUP_TAG]);
        assert!(network.lacks_mempool());
    }

    #[test]
    fn test_is_rollup_with_rollup_tag() {
        let network = create_test_evm_network_with_tags(vec![ROLLUP_TAG, NO_MEMPOOL_TAG]);
        assert!(network.is_rollup());
    }

    #[test]
    fn test_is_rollup_without_rollup_tag() {
        let network = create_test_evm_network_with_tags(vec!["mainnet", "ethereum"]);
        assert!(!network.is_rollup());
    }

    #[test]
    fn test_lacks_mempool_with_no_mempool_tag() {
        let network = create_test_evm_network_with_tags(vec![ROLLUP_TAG, NO_MEMPOOL_TAG]);
        assert!(network.lacks_mempool());
    }

    #[test]
    fn test_lacks_mempool_without_no_mempool_tag() {
        let network = create_test_evm_network_with_tags(vec![ROLLUP_TAG]);
        assert!(!network.lacks_mempool());
    }

    #[test]
    fn test_arbitrum_like_network() {
        let network = create_test_evm_network_with_tags(vec![ROLLUP_TAG, ARBITRUM_BASED_TAG]);
        assert!(network.is_rollup());
        assert!(network.is_arbitrum());
        assert!(network.lacks_mempool());
        assert!(!network.is_optimism());
    }

    #[test]
    fn test_optimism_like_network() {
        let network = create_test_evm_network_with_tags(vec![ROLLUP_TAG, OPTIMISM_BASED_TAG]);
        assert!(network.is_rollup());
        assert!(network.is_optimism());
        assert!(network.lacks_mempool());
    }

    #[test]
    fn test_polygon_zkevm_network() {
        let network = create_test_evm_network_with_tags(vec![ROLLUP_TAG, POLYGON_ZKEVM_TAG]);
        assert!(network.is_rollup());
        assert!(network.is_polygon_zkevm());
        assert!(!network.lacks_mempool());
        assert!(!network.is_optimism());
        assert!(!network.is_arbitrum());
    }

    #[test]
    fn test_ethereum_mainnet_like_network() {
        let network = create_test_evm_network_with_tags(vec!["mainnet", "ethereum"]);
        assert!(!network.is_rollup());
        assert!(!network.is_optimism());
        assert!(!network.lacks_mempool());
    }

    #[test]
    fn test_empty_tags() {
        let network = create_test_evm_network_with_tags(vec![]);
        assert!(!network.is_rollup());
        assert!(!network.is_optimism());
        assert!(!network.lacks_mempool());
    }

    #[test]
    fn test_try_from_with_tags() {
        let network = EvmNetwork::try_from(repo_model(create_test_evm_config())).unwrap();
        assert!(network.is_optimism());
        assert!(network.is_rollup());
        assert!(network.lacks_mempool());
    }

    #[test]
    fn test_try_from_resolves_status_check_initial_delay() {
        let mut config = create_test_evm_config();
        let default_network = EvmNetwork::try_from(repo_model(config.clone())).unwrap();
        assert_eq!(
            default_network.status_check_initial_delay_seconds(),
            DEFAULT_EVM_STATUS_CHECK_INITIAL_DELAY_SECONDS as i64
        );
        assert_eq!(default_network.status_check_retry_delay_seconds(), None);

        config.status_check = Some(StatusCheckConfig {
            initial_delay_seconds: Some(3),
            retry_delay_seconds: Some(5),
        });
        let network = EvmNetwork::try_from(repo_model(config)).unwrap();
        assert_eq!(network.status_check_initial_delay_seconds(), 3);
        assert_eq!(network.status_check_retry_delay_seconds(), Some(5));
    }

    #[test]
    fn test_try_from_rejects_invalid_status_check_initial_delay() {
        for delay in [0, 101] {
            let mut config = create_test_evm_config();
            config.status_check = Some(StatusCheckConfig {
                initial_delay_seconds: Some(delay),
                retry_delay_seconds: None,
            });

            let error = EvmNetwork::try_from(repo_model(config)).unwrap_err();
            assert!(matches!(
                error,
                RepositoryError::InvalidData(message)
                    if message.contains("status_check.initial_delay_seconds")
            ));
        }
    }

    #[test]
    fn test_try_from_rejects_invalid_status_check_retry_delay() {
        for delay in [0, 1, 4, 101] {
            let mut config = create_test_evm_config();
            config.status_check = Some(StatusCheckConfig {
                initial_delay_seconds: None,
                retry_delay_seconds: Some(delay),
            });

            let error = EvmNetwork::try_from(repo_model(config)).unwrap_err();
            assert!(matches!(
                error,
                RepositoryError::InvalidData(message)
                    if message.contains("status_check.retry_delay_seconds")
            ));
        }
    }
}
