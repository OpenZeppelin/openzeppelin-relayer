//! Test registry for managing test signers and deployed contracts
//!
//! The registry provides a centralized configuration for integration tests,
//! including signer addresses, keystore paths, and deployed contract addresses
//! for each test network.

use crate::integration::common::client::RelayerClient;
use eyre::{Context, Result};
use openzeppelin_relayer::models::relayer::RelayerResponse;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Test registry containing network configurations, signers, and contracts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestRegistry {
    pub networks: HashMap<String, NetworkConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _metadata: Option<RegistryMetadata>,
}

/// Network configuration for integration tests
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub network_name: String,
    pub network_type: String,
    pub contracts: HashMap<String, String>,
    pub min_balance: String,

    /// Tags for selection (e.g., ["quick", "ci", "evm", "rollup"])
    #[serde(default)]
    pub tags: Vec<String>,

    /// Whether this network is enabled for testing
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// Relayer discovery via API
pub struct RelayerDiscovery;

impl RelayerDiscovery {
    /// Find all enabled relayers for a network by querying the API
    /// Filters by: network name match + !paused
    pub async fn find_relayers_for_network(network: &str) -> Result<Vec<RelayerResponse>> {
        let client = RelayerClient::from_env()?;
        let all_relayers = client.list_relayers().await?;

        // Filter relayers: network match AND not paused
        let relayers: Vec<RelayerResponse> = all_relayers
            .into_iter()
            .filter(|r| r.network == network && !r.paused)
            .collect();

        Ok(relayers)
    }
}

/// Registry metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryMetadata {
    pub description: String,
    pub version: String,
    pub last_updated: String,
}

impl TestRegistry {
    /// Load the test registry from the default location
    ///
    /// Defaults to `tests/integration/config/local/registry.json` (local mode).
    /// Set TEST_REGISTRY_PATH env var to override (e.g., for testnet mode).
    ///
    /// # Errors
    ///
    /// Returns an error if the registry file cannot be read or parsed
    pub fn load() -> Result<Self> {
        let path = std::env::var("TEST_REGISTRY_PATH")
            .unwrap_or_else(|_| "tests/integration/config/local/registry.json".to_string());
        Self::load_from_path(path)
    }

    /// Load the test registry from a specific path
    ///
    /// # Errors
    ///
    /// Returns an error if the registry file cannot be read or parsed
    pub fn load_from_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let contents = fs::read_to_string(path)
            .wrap_err_with(|| format!("Failed to read registry file: {}", path.display()))?;

        let registry: TestRegistry = serde_json::from_str(&contents)
            .wrap_err_with(|| format!("Failed to parse registry JSON from: {}", path.display()))?;

        Ok(registry)
    }

    /// Get network configuration by name
    ///
    /// # Errors
    ///
    /// Returns an error if the network is not found in the registry
    pub fn get_network(&self, network: &str) -> Result<&NetworkConfig> {
        self.networks
            .get(network)
            .ok_or_else(|| eyre::eyre!("Network '{}' not found in registry", network))
    }

    /// Get contract address for a network
    ///
    /// # Errors
    ///
    /// Returns an error if the network or contract is not found
    pub fn get_contract(&self, network: &str, contract_name: &str) -> Result<&String> {
        let network_config = self.get_network(network)?;
        network_config.contracts.get(contract_name).ok_or_else(|| {
            eyre::eyre!(
                "Contract '{}' not found for network '{}'",
                contract_name,
                network
            )
        })
    }

    /// Get all enabled network names
    pub fn enabled_networks(&self) -> Vec<String> {
        self.networks
            .iter()
            .filter(|(_, config)| config.enabled)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Check if a contract has been deployed (non-placeholder address)
    pub fn has_real_contract(&self, network: &str, contract_name: &str) -> Result<bool> {
        let address = self.get_contract(network, contract_name)?;

        // Check for placeholder addresses
        let is_placeholder = address.starts_with("0x0000000000000000");

        Ok(!is_placeholder)
    }

    /// Validate if a network is ready for testing
    ///
    /// A network is ready if:
    /// - It's enabled
    /// - It has at least one deployed contract (optional for non-EVM networks)
    ///
    /// Note: Signer validation is now done via RelayerDiscovery from config.json
    pub fn validate_readiness(&self, network: &str) -> Result<ReadinessStatus> {
        let config = self.get_network(network)?;

        // Check which contracts are deployed
        let mut has_any_contract = false;

        for (_name, address) in &config.contracts {
            if !address.starts_with("0x0000000000000000") {
                has_any_contract = true;
            }
        }

        // For non-EVM networks without contracts (Solana, Stellar), contracts are optional
        let requires_contracts = !config.contracts.is_empty();
        let has_contracts = if requires_contracts {
            has_any_contract
        } else {
            true // Non-contract networks are OK
        };

        let ready = config.enabled && has_contracts;

        Ok(ReadinessStatus {
            ready,
            enabled: config.enabled,
            has_contracts,
        })
    }
}

/// Status of a network's readiness for testing
#[derive(Debug, Clone)]
pub struct ReadinessStatus {
    pub ready: bool,
    pub enabled: bool,
    pub has_contracts: bool,
}
