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
        network_config
            .contracts
            .get(contract_name)
            .ok_or_else(|| {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_registry() {
        let result = TestRegistry::load();
        assert!(result.is_ok(), "Failed to load registry: {:?}", result.err());

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

        // With placeholder addresses, this should return false
        let has_real = registry.has_real_contract("sepolia", "simple_storage").unwrap();
        assert!(!has_real, "Placeholder contract should not be detected as real");
    }
}
