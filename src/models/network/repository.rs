use crate::{
    config::{
        EvmNetworkConfig, NetworkConfigCommon, NetworkFileConfig, SolanaNetworkConfig,
        StellarNetworkConfig,
    },
    models::NetworkType,
};
use eyre;
use serde::{Deserialize, Serialize};

/// Network configuration data enum that can hold different network types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkConfigData {
    /// EVM network configuration
    Evm(EvmNetworkConfig),
    /// Solana network configuration
    Solana(SolanaNetworkConfig),
    /// Stellar network configuration
    Stellar(StellarNetworkConfig),
}

impl NetworkConfigData {
    /// Returns the common network configuration shared by all network types.
    pub fn common(&self) -> &NetworkConfigCommon {
        match self {
            NetworkConfigData::Evm(config) => &config.common,
            NetworkConfigData::Solana(config) => &config.common,
            NetworkConfigData::Stellar(config) => &config.common,
        }
    }

    /// Returns the network type based on the configuration variant.
    pub fn network_type(&self) -> NetworkType {
        match self {
            NetworkConfigData::Evm(_) => NetworkType::Evm,
            NetworkConfigData::Solana(_) => NetworkType::Solana,
            NetworkConfigData::Stellar(_) => NetworkType::Stellar,
        }
    }

    /// Returns the network name from the common configuration.
    pub fn network_name(&self) -> &str {
        &self.common().network
    }
}

/// Network repository model representing a network configuration stored in the repository.
///
/// This model is used to store network configurations that have been processed from
/// the configuration file and are ready to be used by the application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkRepoModel {
    /// Unique identifier composed of network_type and name, e.g., "evm:mainnet"
    pub id: String,
    /// Name of the network (e.g., "mainnet", "sepolia")
    pub name: String,
    /// Type of the network (EVM, Solana, Stellar)
    pub network_type: NetworkType,
    /// Network configuration data specific to the network type
    pub config: NetworkConfigData,
}

impl NetworkRepoModel {
    /// Creates a new NetworkRepoModel with EVM configuration.
    ///
    /// # Arguments
    /// * `config` - The EVM network configuration
    ///
    /// # Returns
    /// A new NetworkRepoModel instance
    pub fn new_evm(config: EvmNetworkConfig) -> Self {
        let name = config.common.network.clone();
        let id = format!("evm:{}", name).to_lowercase();
        Self {
            id,
            name,
            network_type: NetworkType::Evm,
            config: NetworkConfigData::Evm(config),
        }
    }

    /// Creates a new NetworkRepoModel with Solana configuration.
    ///
    /// # Arguments
    /// * `config` - The Solana network configuration
    ///
    /// # Returns
    /// A new NetworkRepoModel instance
    pub fn new_solana(config: SolanaNetworkConfig) -> Self {
        let name = config.common.network.clone();
        let id = format!("solana:{}", name).to_lowercase();
        Self {
            id,
            name,
            network_type: NetworkType::Solana,
            config: NetworkConfigData::Solana(config),
        }
    }

    /// Creates a new NetworkRepoModel with Stellar configuration.
    ///
    /// # Arguments
    /// * `config` - The Stellar network configuration
    ///
    /// # Returns
    /// A new NetworkRepoModel instance
    pub fn new_stellar(config: StellarNetworkConfig) -> Self {
        let name = config.common.network.clone();
        let id = format!("stellar:{}", name).to_lowercase();
        Self {
            id,
            name,
            network_type: NetworkType::Stellar,
            config: NetworkConfigData::Stellar(config),
        }
    }

    /// Creates an ID string from network type and name.
    ///
    /// # Arguments
    /// * `network_type` - The type of network
    /// * `name` - The name of the network
    ///
    /// # Returns
    /// A lowercase string ID in format "network_type:name"
    pub fn create_id(network_type: NetworkType, name: &str) -> String {
        format!("{:?}:{}", network_type, name).to_lowercase()
    }

    /// Returns the common network configuration.
    pub fn common(&self) -> &NetworkConfigCommon {
        self.config.common()
    }

    /// Returns the network configuration data.
    pub fn config(&self) -> &NetworkConfigData {
        &self.config
    }
}

impl TryFrom<NetworkFileConfig> for NetworkRepoModel {
    type Error = eyre::Report;

    /// Converts a NetworkFileConfig into a NetworkRepoModel.
    ///
    /// # Arguments
    /// * `network_config` - The network file configuration to convert
    ///
    /// # Returns
    /// Result containing the NetworkRepoModel or an error
    fn try_from(network_config: NetworkFileConfig) -> Result<Self, Self::Error> {
        match network_config {
            NetworkFileConfig::Evm(evm_config) => Ok(Self::new_evm(evm_config)),
            NetworkFileConfig::Solana(solana_config) => Ok(Self::new_solana(solana_config)),
            NetworkFileConfig::Stellar(stellar_config) => Ok(Self::new_stellar(stellar_config)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_evm_config(name: &str, chain_id: u64, symbol: &str) -> EvmNetworkConfig {
        EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: name.to_string(),
                from: None,
                rpc_urls: Some(vec!["https://rpc.example.com".to_string()]),
                explorer_urls: Some(vec!["https://explorer.example.com".to_string()]),
                average_blocktime_ms: Some(12000),
                is_testnet: Some(false),
                tags: Some(vec!["mainnet".to_string()]),
            },
            chain_id: Some(chain_id),
            required_confirmations: Some(12),
            features: Some(vec!["eip1559".to_string()]),
            symbol: Some(symbol.to_string()),
        }
    }

    fn create_solana_config(name: &str, is_testnet: bool) -> SolanaNetworkConfig {
        SolanaNetworkConfig {
            common: NetworkConfigCommon {
                network: name.to_string(),
                from: None,
                rpc_urls: Some(vec!["https://api.mainnet-beta.solana.com".to_string()]),
                explorer_urls: Some(vec!["https://explorer.solana.com".to_string()]),
                average_blocktime_ms: Some(400),
                is_testnet: Some(is_testnet),
                tags: Some(vec!["solana".to_string()]),
            },
        }
    }

    fn create_stellar_config(name: &str, passphrase: Option<&str>) -> StellarNetworkConfig {
        StellarNetworkConfig {
            common: NetworkConfigCommon {
                network: name.to_string(),
                from: None,
                rpc_urls: Some(vec!["https://horizon.stellar.org".to_string()]),
                explorer_urls: Some(vec!["https://stellarchain.io".to_string()]),
                average_blocktime_ms: Some(5000),
                is_testnet: Some(passphrase.is_none()),
                tags: Some(vec!["stellar".to_string()]),
            },
            passphrase: passphrase.map(|s| s.to_string()),
        }
    }

    #[test]
    fn test_network_config_data_evm() {
        let config = create_evm_config("mainnet", 1, "ETH");
        let config_data = NetworkConfigData::Evm(config);

        assert_eq!(config_data.network_name(), "mainnet");
        assert_eq!(config_data.network_type(), NetworkType::Evm);
        assert_eq!(config_data.common().network, "mainnet");
        assert_eq!(config_data.common().is_testnet, Some(false));
    }

    #[test]
    fn test_network_config_data_solana() {
        let config = create_solana_config("devnet", true);
        let config_data = NetworkConfigData::Solana(config);

        assert_eq!(config_data.network_name(), "devnet");
        assert_eq!(config_data.network_type(), NetworkType::Solana);
        assert_eq!(config_data.common().is_testnet, Some(true));
    }

    #[test]
    fn test_network_config_data_stellar() {
        let config = create_stellar_config("testnet", None);
        let config_data = NetworkConfigData::Stellar(config);

        assert_eq!(config_data.network_name(), "testnet");
        assert_eq!(config_data.network_type(), NetworkType::Stellar);
        assert_eq!(config_data.common().is_testnet, Some(true));
    }

    #[test]
    fn test_new_evm() {
        let config = create_evm_config("mainnet", 1, "ETH");
        let network_repo = NetworkRepoModel::new_evm(config);

        assert_eq!(network_repo.name, "mainnet");
        assert_eq!(network_repo.network_type, NetworkType::Evm);
        assert_eq!(network_repo.id, "evm:mainnet");

        match network_repo.config() {
            NetworkConfigData::Evm(evm_config) => {
                assert_eq!(evm_config.chain_id, Some(1));
                assert_eq!(evm_config.symbol, Some("ETH".to_string()));
            }
            _ => panic!("Expected EVM config"),
        }
    }

    #[test]
    fn test_new_solana() {
        let config = create_solana_config("devnet", true);
        let network_repo = NetworkRepoModel::new_solana(config);

        assert_eq!(network_repo.name, "devnet");
        assert_eq!(network_repo.network_type, NetworkType::Solana);
        assert_eq!(network_repo.id, "solana:devnet");

        match network_repo.config() {
            NetworkConfigData::Solana(solana_config) => {
                assert_eq!(solana_config.common.is_testnet, Some(true));
            }
            _ => panic!("Expected Solana config"),
        }
    }

    #[test]
    fn test_new_stellar() {
        let config = create_stellar_config(
            "mainnet",
            Some("Public Global Stellar Network ; September 2015"),
        );
        let network_repo = NetworkRepoModel::new_stellar(config);

        assert_eq!(network_repo.name, "mainnet");
        assert_eq!(network_repo.network_type, NetworkType::Stellar);
        assert_eq!(network_repo.id, "stellar:mainnet");

        match network_repo.config() {
            NetworkConfigData::Stellar(stellar_config) => {
                assert_eq!(
                    stellar_config.passphrase,
                    Some("Public Global Stellar Network ; September 2015".to_string())
                );
            }
            _ => panic!("Expected Stellar config"),
        }
    }

    #[test]
    fn test_create_id() {
        assert_eq!(
            NetworkRepoModel::create_id(NetworkType::Evm, "Mainnet"),
            "evm:mainnet"
        );
        assert_eq!(
            NetworkRepoModel::create_id(NetworkType::Solana, "DEVNET"),
            "solana:devnet"
        );
        assert_eq!(
            NetworkRepoModel::create_id(NetworkType::Stellar, "TestNet"),
            "stellar:testnet"
        );
    }

    #[test]
    fn test_create_id_with_special_characters() {
        assert_eq!(
            NetworkRepoModel::create_id(NetworkType::Evm, "My-Network_123"),
            "evm:my-network_123"
        );
        assert_eq!(
            NetworkRepoModel::create_id(NetworkType::Solana, "Test Network"),
            "solana:test network"
        );
    }

    #[test]
    fn test_common_method() {
        let config = create_evm_config("mainnet", 1, "ETH");
        let network_repo = NetworkRepoModel::new_evm(config);

        let common = network_repo.common();
        assert_eq!(common.network, "mainnet");
        assert_eq!(common.is_testnet, Some(false));
        assert_eq!(common.average_blocktime_ms, Some(12000));
        assert_eq!(
            common.rpc_urls,
            Some(vec!["https://rpc.example.com".to_string()])
        );
    }

    #[test]
    fn test_config_method() {
        let config = create_evm_config("mainnet", 1, "ETH");
        let network_repo = NetworkRepoModel::new_evm(config);

        let config_data = network_repo.config();
        assert!(matches!(config_data, NetworkConfigData::Evm(_)));
        assert_eq!(config_data.network_type(), NetworkType::Evm);
        assert_eq!(config_data.network_name(), "mainnet");
    }

    #[test]
    fn test_try_from_evm() {
        let evm_config = create_evm_config("mainnet", 1, "ETH");
        let network_file_config = NetworkFileConfig::Evm(evm_config);

        let result = NetworkRepoModel::try_from(network_file_config);
        assert!(result.is_ok());

        let network_repo = result.unwrap();
        assert_eq!(network_repo.name, "mainnet");
        assert_eq!(network_repo.network_type, NetworkType::Evm);
        assert_eq!(network_repo.id, "evm:mainnet");
    }

    #[test]
    fn test_try_from_solana() {
        let solana_config = create_solana_config("devnet", true);
        let network_file_config = NetworkFileConfig::Solana(solana_config);

        let result = NetworkRepoModel::try_from(network_file_config);
        assert!(result.is_ok());

        let network_repo = result.unwrap();
        assert_eq!(network_repo.name, "devnet");
        assert_eq!(network_repo.network_type, NetworkType::Solana);
        assert_eq!(network_repo.id, "solana:devnet");
    }

    #[test]
    fn test_try_from_stellar() {
        let stellar_config = create_stellar_config("testnet", None);
        let network_file_config = NetworkFileConfig::Stellar(stellar_config);

        let result = NetworkRepoModel::try_from(network_file_config);
        assert!(result.is_ok());

        let network_repo = result.unwrap();
        assert_eq!(network_repo.name, "testnet");
        assert_eq!(network_repo.network_type, NetworkType::Stellar);
        assert_eq!(network_repo.id, "stellar:testnet");
    }

    #[test]
    fn test_serialization_roundtrip() {
        let config = create_evm_config("mainnet", 1, "ETH");
        let network_repo = NetworkRepoModel::new_evm(config);

        let serialized = serde_json::to_string(&network_repo).unwrap();
        let deserialized: NetworkRepoModel = serde_json::from_str(&serialized).unwrap();

        assert_eq!(network_repo.id, deserialized.id);
        assert_eq!(network_repo.name, deserialized.name);
        assert_eq!(network_repo.network_type, deserialized.network_type);
    }

    #[test]
    fn test_clone() {
        let config = create_evm_config("mainnet", 1, "ETH");
        let network_repo = NetworkRepoModel::new_evm(config);
        let cloned = network_repo.clone();

        assert_eq!(network_repo.id, cloned.id);
        assert_eq!(network_repo.name, cloned.name);
        assert_eq!(network_repo.network_type, cloned.network_type);
    }

    #[test]
    fn test_debug() {
        let config = create_evm_config("mainnet", 1, "ETH");
        let network_repo = NetworkRepoModel::new_evm(config);

        let debug_str = format!("{:?}", network_repo);
        assert!(debug_str.contains("NetworkRepoModel"));
        assert!(debug_str.contains("mainnet"));
        assert!(debug_str.contains("Evm"));
    }

    #[test]
    fn test_network_types_consistency() {
        let evm_config = create_evm_config("mainnet", 1, "ETH");
        let solana_config = create_solana_config("devnet", true);
        let stellar_config = create_stellar_config("testnet", None);

        let evm_repo = NetworkRepoModel::new_evm(evm_config);
        let solana_repo = NetworkRepoModel::new_solana(solana_config);
        let stellar_repo = NetworkRepoModel::new_stellar(stellar_config);

        assert_eq!(evm_repo.network_type, evm_repo.config().network_type());
        assert_eq!(
            solana_repo.network_type,
            solana_repo.config().network_type()
        );
        assert_eq!(
            stellar_repo.network_type,
            stellar_repo.config().network_type()
        );
    }

    #[test]
    fn test_empty_optional_fields() {
        let minimal_config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "minimal".to_string(),
                from: None,
                rpc_urls: Some(vec!["https://rpc.example.com".to_string()]),
                explorer_urls: None,
                average_blocktime_ms: None,
                is_testnet: None,
                tags: None,
            },
            chain_id: Some(1),
            required_confirmations: Some(1),
            features: None,
            symbol: Some("ETH".to_string()),
        };

        let network_repo = NetworkRepoModel::new_evm(minimal_config);
        assert_eq!(network_repo.name, "minimal");
        assert_eq!(network_repo.common().explorer_urls, None);
        assert_eq!(network_repo.common().tags, None);
    }
}
