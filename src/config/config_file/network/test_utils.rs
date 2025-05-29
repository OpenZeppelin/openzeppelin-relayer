//! Test utilities for network configuration tests.
//!
//! This module provides simple helper functions used across
//! the network configuration test modules to reduce code duplication.

use super::*;
use serde_json::json;
use std::fs::File;
use std::io::Write;
use tempfile::TempDir;

// =============================================================================
// Simple Network Creation Functions
// =============================================================================

/// Creates a default valid NetworkConfigCommon for testing.
pub fn create_network_common(network: &str) -> NetworkConfigCommon {
    NetworkConfigCommon {
        network: network.to_string(),
        from: None,
        rpc_urls: Some(vec!["https://rpc.example.com".to_string()]),
        explorer_urls: Some(vec!["https://explorer.example.com".to_string()]),
        average_blocktime_ms: Some(12000),
        is_testnet: Some(true),
        tags: Some(vec!["test".to_string()]),
    }
}

/// Creates a NetworkConfigCommon with inheritance.
pub fn create_network_common_with_parent(network: &str, parent: &str) -> NetworkConfigCommon {
    NetworkConfigCommon {
        network: network.to_string(),
        from: Some(parent.to_string()),
        rpc_urls: None,
        explorer_urls: None,
        average_blocktime_ms: None,
        is_testnet: None,
        tags: None,
    }
}

/// Creates a default valid EVM network configuration.
pub fn create_evm_network(network: &str) -> EvmNetworkConfig {
    EvmNetworkConfig {
        common: create_network_common(network),
        chain_id: Some(31337),
        required_confirmations: Some(1),
        features: Some(vec!["eip1559".to_string()]),
        symbol: Some("ETH".to_string()),
    }
}

/// Creates an EVM network configuration with inheritance.
pub fn create_evm_network_with_parent(network: &str, parent: &str) -> EvmNetworkConfig {
    EvmNetworkConfig {
        common: create_network_common_with_parent(network, parent),
        chain_id: Some(31338), // Override parent's chain_id to show inheritance working
        required_confirmations: Some(1), // Required field, but could be different from parent
        features: None,        // Will inherit from parent
        symbol: Some("ETH".to_string()), // Required field
    }
}

/// Creates an EVM network configuration for inheritance testing (with None values).
pub fn create_evm_network_for_inheritance_test(network: &str, parent: &str) -> EvmNetworkConfig {
    EvmNetworkConfig {
        common: create_network_common_with_parent(network, parent),
        chain_id: None,               // Will inherit from parent
        required_confirmations: None, // Will inherit from parent
        features: None,               // Will inherit from parent
        symbol: None,                 // Will inherit from parent
    }
}

/// Creates an invalid EVM network (missing required fields).
pub fn create_invalid_evm_network(network: &str) -> EvmNetworkConfig {
    EvmNetworkConfig {
        common: NetworkConfigCommon {
            network: network.to_string(),
            from: None,
            rpc_urls: None, // Missing required field
            explorer_urls: None,
            average_blocktime_ms: None,
            is_testnet: None,
            tags: None,
        },
        chain_id: None, // Missing required field
        required_confirmations: None,
        features: None,
        symbol: None,
    }
}

/// Creates a default valid Solana network configuration.
pub fn create_solana_network(network: &str) -> SolanaNetworkConfig {
    SolanaNetworkConfig {
        common: NetworkConfigCommon {
            network: network.to_string(),
            from: None,
            rpc_urls: Some(vec![format!("https://api.{}.solana.com", network)]),
            explorer_urls: Some(vec!["https://explorer.example.com".to_string()]),
            average_blocktime_ms: Some(400),
            is_testnet: Some(true),
            tags: Some(vec!["solana".to_string()]),
        },
    }
}

/// Creates a Solana network configuration with inheritance.
pub fn create_solana_network_with_parent(network: &str, parent: &str) -> SolanaNetworkConfig {
    SolanaNetworkConfig {
        common: NetworkConfigCommon {
            network: network.to_string(),
            from: Some(parent.to_string()),
            rpc_urls: Some(vec![format!("https://api.{}.solana.com", network)]), // Override parent's RPC URLs
            explorer_urls: Some(vec!["https://explorer.example.com".to_string()]),
            average_blocktime_ms: Some(500), // Override parent's blocktime
            is_testnet: None,                // Will inherit from parent
            tags: None,                      // Will inherit from parent
        },
    }
}

/// Creates a default valid Stellar network configuration.
pub fn create_stellar_network(network: &str) -> StellarNetworkConfig {
    StellarNetworkConfig {
        common: NetworkConfigCommon {
            network: network.to_string(),
            from: None,
            rpc_urls: Some(vec![format!("https://horizon.{}.stellar.org", network)]),
            explorer_urls: Some(vec!["https://explorer.example.com".to_string()]),
            average_blocktime_ms: Some(5000),
            is_testnet: Some(true),
            tags: Some(vec!["stellar".to_string()]),
        },
        passphrase: Some("Test Network ; September 2015".to_string()),
    }
}

/// Creates a Stellar network configuration with inheritance.
pub fn create_stellar_network_with_parent(network: &str, parent: &str) -> StellarNetworkConfig {
    StellarNetworkConfig {
        common: NetworkConfigCommon {
            network: network.to_string(),
            from: Some(parent.to_string()),
            rpc_urls: Some(vec![format!("https://horizon.{}.stellar.org", network)]), // Override parent's RPC URLs
            explorer_urls: Some(vec!["https://explorer.example.com".to_string()]),
            average_blocktime_ms: Some(6000), // Override parent's blocktime
            is_testnet: None,                 // Will inherit from parent
            tags: None,                       // Will inherit from parent
        },
        passphrase: None, // Will inherit from parent
    }
}

// =============================================================================
// Wrapped Network Creation Functions (for NetworkFileConfig)
// =============================================================================

/// Creates a wrapped EVM network configuration.
pub fn create_evm_network_wrapped(network: &str) -> NetworkFileConfig {
    NetworkFileConfig::Evm(create_evm_network(network))
}

/// Creates a wrapped EVM network configuration with inheritance.
pub fn create_evm_network_wrapped_with_parent(network: &str, parent: &str) -> NetworkFileConfig {
    NetworkFileConfig::Evm(create_evm_network_with_parent(network, parent))
}

/// Creates a wrapped invalid EVM network configuration.
pub fn create_invalid_evm_network_wrapped(network: &str) -> NetworkFileConfig {
    NetworkFileConfig::Evm(create_invalid_evm_network(network))
}

/// Creates a wrapped Solana network configuration.
pub fn create_solana_network_wrapped(network: &str) -> NetworkFileConfig {
    NetworkFileConfig::Solana(create_solana_network(network))
}

/// Creates a wrapped Solana network configuration with inheritance.
pub fn create_solana_network_wrapped_with_parent(network: &str, parent: &str) -> NetworkFileConfig {
    NetworkFileConfig::Solana(create_solana_network_with_parent(network, parent))
}

/// Creates a wrapped Stellar network configuration.
pub fn create_stellar_network_wrapped(network: &str) -> NetworkFileConfig {
    NetworkFileConfig::Stellar(create_stellar_network(network))
}

// =============================================================================
// Temporary File Utilities
// =============================================================================

/// Creates a temporary file with the given content.
pub fn create_temp_file(dir: &TempDir, file_name: &str, content: &str) {
    let file_path = dir.path().join(file_name);
    let mut file = File::create(file_path).expect("Failed to create temp file");
    write!(file, "{}", content).expect("Failed to write to temp file");
}

/// Creates a valid EVM network JSON for testing file loading.
pub fn create_valid_evm_network_json() -> serde_json::Value {
    json!({
        "networks": [
            {
                "type": "evm",
                "network": "test-evm",
                "chain_id": 31337,
                "required_confirmations": 1,
                "symbol": "ETH",
                "rpc_urls": ["https://rpc.test.example.com"]
            }
        ]
    })
}

/// Creates a valid Solana network JSON for testing file loading.
pub fn create_valid_solana_network_json() -> serde_json::Value {
    json!({
        "networks": [
            {
                "type": "solana",
                "network": "test-solana",
                "rpc_urls": ["https://api.devnet.solana.com"]
            }
        ]
    })
}

/// Creates an invalid network JSON for testing error handling.
pub fn create_invalid_network_json() -> serde_json::Value {
    json!({
        "networks": [
            {
                "type": "invalid_type", // Invalid network type that doesn't exist
                "network": "invalid-network",
                "some_field": "some_value"
            }
        ]
    })
}
