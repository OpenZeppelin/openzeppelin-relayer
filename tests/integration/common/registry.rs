//! Test registry for managing test signers and deployed contracts
//!
//! The registry provides a centralized configuration for integration tests,
//! including signer addresses, keystore paths, and deployed contract addresses
//! for each test network.

use eyre::{Context, Result};
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
    pub signer: SignerConfig,
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

/// Signer configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignerConfig {
    pub id: String,
    pub address: String,
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
    /// # Errors
    ///
    /// Returns an error if the registry file cannot be read or parsed
    pub fn load() -> Result<Self> {
        Self::load_from_path("tests/integration/registry.json")
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

    /// Get signer configuration for a network
    ///
    /// # Errors
    ///
    /// Returns an error if the network is not found in the registry
    pub fn get_signer(&self, network: &str) -> Result<&SignerConfig> {
        let network_config = self.get_network(network)?;
        Ok(&network_config.signer)
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

    /// Get all network names in the registry
    pub fn network_names(&self) -> Vec<String> {
        self.networks.keys().cloned().collect()
    }

    /// Get networks by type (evm, solana, stellar)
    pub fn networks_by_type(&self, network_type: &str) -> Vec<String> {
        self.networks
            .iter()
            .filter(|(_, config)| config.network_type == network_type)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Check if a signer has been configured (non-placeholder address)
    pub fn has_real_signer(&self, network: &str) -> Result<bool> {
        let signer = self.get_signer(network)?;

        // Check for placeholder or not-yet-derived addresses
        let is_placeholder = signer.address.starts_with("0x0000000000000000")
            || signer.address.starts_with("1111111111111111")
            || signer.address == "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
            || signer.address == "TO_BE_DERIVED_FROM_KEYSTORE"
            || signer.address == "TO_BE_EXTRACTED"
            || signer.address == "TO_BE_IMPLEMENTED"
            || signer.address.starts_with("PLACEHOLDER_");

        Ok(!is_placeholder)
    }

    /// Check if a contract has been deployed (non-placeholder address)
    pub fn has_real_contract(&self, network: &str, contract_name: &str) -> Result<bool> {
        let address = self.get_contract(network, contract_name)?;

        // Check for placeholder addresses
        let is_placeholder = address.starts_with("0x0000000000000000");

        Ok(!is_placeholder)
    }

    /// Get networks filtered by tags
    ///
    /// Returns networks that have ALL of the specified tags and are enabled
    pub fn networks_by_tags(&self, tags: &[String]) -> Vec<String> {
        self.networks
            .iter()
            .filter(|(_, config)| {
                config.enabled && tags.iter().all(|tag| config.tags.contains(tag))
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Get all enabled networks
    pub fn enabled_networks(&self) -> Vec<String> {
        self.networks
            .iter()
            .filter(|(_, config)| config.enabled)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Validate if a network is ready for testing
    ///
    /// A network is ready if:
    /// - It's enabled
    /// - It has a real (non-placeholder) signer address
    /// - It has at least one deployed contract (optional for non-EVM networks)
    pub fn validate_readiness(&self, network: &str) -> Result<ReadinessStatus> {
        let config = self.get_network(network)?;

        let has_real_signer = self.has_real_signer(network).unwrap_or(false);

        // Check which contracts are deployed
        let mut missing_contracts = Vec::new();
        let mut has_any_contract = false;

        for (name, address) in &config.contracts {
            if address.starts_with("0x0000000000000000") {
                missing_contracts.push(name.clone());
            } else {
                has_any_contract = true;
            }
        }

        // For non-EVM networks without contracts (Solana, Stellar), just check signer
        let requires_contracts = !config.contracts.is_empty();
        let has_contracts = if requires_contracts {
            has_any_contract
        } else {
            true // Non-contract networks are OK
        };

        let ready = config.enabled && has_real_signer && has_contracts;

        Ok(ReadinessStatus {
            network: network.to_string(),
            ready,
            enabled: config.enabled,
            has_signer: has_real_signer,
            has_contracts,
            missing_contracts,
        })
    }
}

/// Status of a network's readiness for testing
#[derive(Debug, Clone)]
pub struct ReadinessStatus {
    pub network: String,
    pub ready: bool,
    pub enabled: bool,
    pub has_signer: bool,
    pub has_contracts: bool,
    pub missing_contracts: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_registry() {
        let result = TestRegistry::load();
        assert!(
            result.is_ok(),
            "Failed to load registry: {:?}",
            result.err()
        );

        let registry = result.unwrap();
        assert!(!registry.networks.is_empty(), "Registry has no networks");
    }

    #[test]
    fn test_get_network() {
        let registry = TestRegistry::load().unwrap();

        // Test valid network
        let sepolia = registry.get_network("sepolia");
        assert!(sepolia.is_ok());
        assert_eq!(sepolia.unwrap().network_type, "evm");

        // Test invalid network
        let invalid = registry.get_network("non-existent");
        assert!(invalid.is_err());
    }

    #[test]
    fn test_get_signer() {
        let registry = TestRegistry::load().unwrap();

        let signer = registry.get_signer("sepolia");
        assert!(signer.is_ok());

        let signer = signer.unwrap();
        assert!(!signer.address.is_empty());
        assert!(!signer.id.is_empty());
    }

    #[test]
    fn test_get_contract() {
        let registry = TestRegistry::load().unwrap();

        // Test valid contract
        let contract = registry.get_contract("sepolia", "simple_storage");
        assert!(contract.is_ok());

        // Test invalid contract name
        let invalid = registry.get_contract("sepolia", "non_existent");
        assert!(invalid.is_err());
    }

    #[test]
    fn test_networks_by_type() {
        let registry = TestRegistry::load().unwrap();

        let evm_networks = registry.networks_by_type("evm");
        assert!(!evm_networks.is_empty());
        assert!(evm_networks.contains(&"sepolia".to_string()));

        // Verify the function works for other network types (may be empty)
        let _solana_networks = registry.networks_by_type("solana");
        let _stellar_networks = registry.networks_by_type("stellar");
    }

    #[test]
    fn test_has_real_signer() {
        let registry = TestRegistry::load().unwrap();

        // Sepolia has a real address, should return true
        let has_real_sepolia = registry.has_real_signer("sepolia").unwrap();
        assert!(has_real_sepolia, "Sepolia should have a real signer");
    }

    #[test]
    fn test_has_real_contract() {
        let registry = TestRegistry::load().unwrap();

        // simple_storage has a real deployed contract
        let has_real = registry
            .has_real_contract("sepolia", "simple_storage")
            .unwrap();
        assert!(has_real, "simple_storage should be detected as real");

        // test_erc20 is still a placeholder
        let has_placeholder = registry.has_real_contract("sepolia", "test_erc20").unwrap();
        assert!(
            !has_placeholder,
            "test_erc20 should not be detected as real"
        );
    }
}
