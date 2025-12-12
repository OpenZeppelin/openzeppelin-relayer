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

    /// ID of the pre-configured relayer for this network (defined in config.json)
    pub relayer_id: String,

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

    /// Get all enabled network names
    pub fn enabled_networks(&self) -> Vec<String> {
        self.networks
            .iter()
            .filter(|(_, config)| config.enabled)
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
            ready,
            enabled: config.enabled,
            has_signer: has_real_signer,
            has_contracts,
        })
    }
}

/// Status of a network's readiness for testing
#[derive(Debug, Clone)]
pub struct ReadinessStatus {
    pub ready: bool,
    pub enabled: bool,
    pub has_signer: bool,
    pub has_contracts: bool,
}
