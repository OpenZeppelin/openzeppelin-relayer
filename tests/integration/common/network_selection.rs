//! Network selection logic for integration tests
//!
//! This module provides utilities for selecting which networks to test based on
//! environment variables, test modes, and network tags.

use super::registry::TestRegistry;
use eyre::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::Path;

/// Main config.json structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub networks: String,
}

/// Network configuration from config/networks/*.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfigFile {
    pub networks: Vec<NetworkEntry>,
}

/// Individual network entry from config files
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkEntry {
    pub network: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_testnet: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub average_blocktime_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_confirmations: Option<u32>,
}

/// Get the list of networks to test based on environment variables
///
/// Priority:
/// 1. TEST_NETWORKS (comma-separated list) - explicit override
/// 2. TEST_TAGS (comma-separated tags) - select by tags
/// 3. TEST_MODE (quick/ci/full) - select by mode
/// 4. Default to "quick" mode (networks tagged with "quick")
///
/// # Examples
///
/// ```
/// // Select networks tagged with "quick"
/// std::env::set_var("TEST_MODE", "quick");
/// let networks = get_test_networks().unwrap();
///
/// // Select networks tagged with both "evm" and "fast"
/// std::env::set_var("TEST_TAGS", "evm,fast");
/// let networks = get_test_networks().unwrap();
///
/// // Explicit network list
/// std::env::set_var("TEST_NETWORKS", "sepolia,optimism-sepolia");
/// let networks = get_test_networks().unwrap();
/// ```
pub fn get_test_networks() -> Result<Vec<String>> {
    let registry = TestRegistry::load()?;

    // Priority 1: Explicit TEST_NETWORKS override
    if let Ok(networks_str) = env::var("TEST_NETWORKS") {
        let networks_str = networks_str.trim();
        if !networks_str.is_empty() {
            let networks: Vec<String> = networks_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            if !networks.is_empty() {
                // Validate that networks exist in registry
                for network in &networks {
                    if registry.get_network(network).is_err() {
                        eprintln!("Warning: Network '{}' not found in registry", network);
                    }
                }
                return Ok(networks);
            }
        }
    }

    // Priority 2: TEST_TAGS for tag-based selection
    if let Ok(tags_str) = env::var("TEST_TAGS") {
        let tags_str = tags_str.trim();
        if !tags_str.is_empty() {
            return select_by_tags(&tags_str, &registry);
        }
    }

    // Priority 3: TEST_MODE
    let test_mode = env::var("TEST_MODE").unwrap_or_else(|_| "quick".to_string());

    select_by_mode(&test_mode, &registry)
}

/// Select networks by tags
///
/// Returns networks that have ALL of the specified tags
fn select_by_tags(tags_str: &str, registry: &TestRegistry) -> Result<Vec<String>> {
    let tags: Vec<String> = tags_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if tags.is_empty() {
        return Err(eyre::eyre!("No tags specified"));
    }

    let networks = registry.networks_by_tags(&tags);

    if networks.is_empty() {
        return Err(eyre::eyre!(
            "No networks found with tags: {}",
            tags.join(", ")
        ));
    }

    Ok(networks)
}

/// Select networks by test mode
///
/// Modes:
/// - "quick": Networks tagged with "quick"
/// - "ci": Networks tagged with "ci"
/// - "full": All non-deprecated testnets from config/networks/*.json
fn select_by_mode(mode: &str, registry: &TestRegistry) -> Result<Vec<String>> {
    match mode.to_lowercase().as_str() {
        "quick" => select_by_tags("quick", registry),
        "ci" => select_by_tags("ci", registry),
        "full" => load_full_testnets(),
        _ => {
            eprintln!(
                "Warning: Unknown TEST_MODE '{}', defaulting to 'quick'",
                mode
            );
            select_by_tags("quick", registry)
        }
    }
}

/// Load all non-deprecated testnets from config/networks/*.json
///
/// This reads config.json to get the networks path, then scans that directory
/// for all networks where:
/// - is_testnet = true
/// - tags does not contain "deprecated"
///
/// The config path can be specified via CONFIG_PATH env var, defaults to config/config.json
pub fn load_full_testnets() -> Result<Vec<String>> {
    // Read config.json to get networks path
    let config_path_str =
        env::var("CONFIG_PATH").unwrap_or_else(|_| "config/config.json".to_string());
    let config_path = Path::new(&config_path_str);

    if !config_path.exists() {
        return Err(eyre::eyre!(
            "Config file not found: {}",
            config_path.display()
        ));
    }

    let config_contents = fs::read_to_string(config_path)
        .wrap_err_with(|| format!("Failed to read config: {}", config_path.display()))?;

    let config: Config =
        serde_json::from_str(&config_contents).wrap_err("Failed to parse config.json")?;

    // Get networks directory from config
    let config_dir = Path::new(&config.networks);

    if !config_dir.exists() {
        return Err(eyre::eyre!(
            "Networks directory not found: {}",
            config_dir.display()
        ));
    }

    let mut testnets = Vec::new();

    // Read all .json files in the networks directory
    for entry in fs::read_dir(config_dir)
        .wrap_err_with(|| format!("Failed to read directory: {}", config_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();

        // Skip non-JSON files
        if !path.extension().map_or(false, |ext| ext == "json") {
            continue;
        }

        // Read and parse the file
        let contents = fs::read_to_string(&path)
            .wrap_err_with(|| format!("Failed to read file: {}", path.display()))?;

        let config: NetworkConfigFile = serde_json::from_str(&contents)
            .wrap_err_with(|| format!("Failed to parse JSON from: {}", path.display()))?;

        // Filter for non-deprecated testnets
        for network in config.networks {
            if network.is_testnet.unwrap_or(false) {
                let is_deprecated = network
                    .tags
                    .as_ref()
                    .map(|tags| tags.contains(&"deprecated".to_string()))
                    .unwrap_or(false);

                if !is_deprecated {
                    testnets.push(network.network);
                }
            }
        }
    }

    // Sort for consistent ordering
    testnets.sort();
    testnets.dedup();

    Ok(testnets)
}

/// Filter networks to only include ready ones
///
/// If TEST_READY_ONLY is set to "true", filters out networks that aren't ready
pub fn get_ready_networks(networks: Vec<String>) -> Result<Vec<String>> {
    let ready_only = env::var("TEST_READY_ONLY")
        .unwrap_or_else(|_| "false".to_string())
        .to_lowercase()
        == "true";

    if !ready_only {
        return Ok(networks);
    }

    let registry = TestRegistry::load()?;

    let ready: Vec<String> = networks
        .into_iter()
        .filter(|network| {
            registry
                .validate_readiness(network)
                .map(|status| status.ready)
                .unwrap_or(false)
        })
        .collect();

    if ready.is_empty() {
        return Err(eyre::eyre!(
            "No ready networks found. Ensure signers are funded and contracts are deployed."
        ));
    }

    Ok(ready)
}

/// Report network readiness status
///
/// Provides a detailed report of which networks are ready for testing
pub fn report_network_readiness() -> Result<()> {
    let registry = TestRegistry::load()?;
    let selected = get_test_networks()?;

    println!("\n{}", "=".repeat(70));
    println!("Integration Test Network Readiness Report");
    println!("{}\n", "=".repeat(70));

    let mut ready_count = 0;

    for network in &selected {
        match registry.validate_readiness(network) {
            Ok(status) => {
                if status.ready {
                    ready_count += 1;
                    println!("✅ {}: READY", network);

                    if let Ok(config) = registry.get_network(network) {
                        println!("   Type: {}", config.network_type);
                        if !config.tags.is_empty() {
                            println!("   Tags: {}", config.tags.join(", "));
                        }
                    }
                } else {
                    println!("⚠️  {}: NOT READY", network);
                    if !status.enabled {
                        println!("   - Network is disabled");
                    }
                    if !status.has_signer {
                        println!("   - Missing funded signer");
                    }
                    if !status.has_contracts && !status.missing_contracts.is_empty() {
                        println!(
                            "   - Missing contracts: {}",
                            status.missing_contracts.join(", ")
                        );
                    }
                }
            }
            Err(e) => {
                println!("❌ {}: ERROR - {}", network, e);
            }
        }
        println!();
    }

    println!("{}", "=".repeat(70));
    println!("Summary: {}/{} networks ready", ready_count, selected.len());
    println!("{}\n", "=".repeat(70));

    if ready_count == 0 {
        return Err(eyre::eyre!("No networks are ready for testing"));
    }

    Ok(())
}

/// Check if the current test run should test a specific network
///
/// This is useful for conditional test execution based on the selected networks
pub fn should_test_network(network: &str) -> Result<bool> {
    let selected_networks = get_test_networks()?;
    Ok(selected_networks.contains(&network.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn test_get_test_networks_default() {
        // Clear environment variables
        env::remove_var("TEST_MODE");
        env::remove_var("TEST_NETWORKS");
        env::remove_var("TEST_TAGS");

        let networks = get_test_networks().unwrap();
        // Default mode is "quick", should return networks tagged with "quick"
        assert!(!networks.is_empty());
        assert!(networks.contains(&"sepolia".to_string()));
    }

    #[test]
    #[serial]
    fn test_get_test_networks_quick() {
        env::set_var("TEST_MODE", "quick");
        env::remove_var("TEST_NETWORKS");
        env::remove_var("TEST_TAGS");

        let networks = get_test_networks().unwrap();
        // Should return networks tagged with "quick"
        assert!(!networks.is_empty());
        assert!(networks.contains(&"sepolia".to_string()));

        env::remove_var("TEST_MODE");
    }

    #[test]
    #[serial]
    fn test_get_test_networks_ci() {
        env::set_var("TEST_MODE", "ci");
        env::remove_var("TEST_NETWORKS");
        env::remove_var("TEST_TAGS");

        let networks = get_test_networks().unwrap();
        // Should return networks tagged with "ci"
        assert!(!networks.is_empty());
        assert!(networks.contains(&"sepolia".to_string()));

        env::remove_var("TEST_MODE");
    }

    #[test]
    #[serial]
    fn test_get_test_networks_by_tags() {
        env::set_var("TEST_TAGS", "evm");
        env::remove_var("TEST_MODE");
        env::remove_var("TEST_NETWORKS");

        let networks = get_test_networks().unwrap();
        // Should return networks tagged with "evm"
        assert!(!networks.is_empty());
        assert!(networks.contains(&"sepolia".to_string()));

        env::remove_var("TEST_TAGS");
    }

    #[test]
    #[serial]
    fn test_get_test_networks_by_multiple_tags() {
        env::set_var("TEST_TAGS", "evm,quick");
        env::remove_var("TEST_MODE");
        env::remove_var("TEST_NETWORKS");

        let networks = get_test_networks().unwrap();
        // Should return networks with both "evm" AND "quick" tags
        assert!(!networks.is_empty());
        assert!(networks.contains(&"sepolia".to_string()));

        env::remove_var("TEST_TAGS");
    }

    #[test]
    #[serial]
    fn test_get_test_networks_custom() {
        env::set_var("TEST_NETWORKS", "sepolia,base-sepolia");
        env::remove_var("TEST_MODE");
        env::remove_var("TEST_TAGS");

        let networks = get_test_networks().unwrap();
        assert_eq!(networks.len(), 2);
        assert_eq!(networks, vec!["sepolia", "base-sepolia"]);

        env::remove_var("TEST_NETWORKS");
    }

    #[test]
    #[serial]
    fn test_get_test_networks_custom_with_spaces() {
        env::set_var("TEST_NETWORKS", " sepolia , base-sepolia , ");
        env::remove_var("TEST_MODE");
        env::remove_var("TEST_TAGS");

        let networks = get_test_networks().unwrap();
        assert_eq!(networks.len(), 2);
        assert_eq!(networks, vec!["sepolia", "base-sepolia"]);

        env::remove_var("TEST_NETWORKS");
    }

    #[test]
    #[serial]
    fn test_get_test_networks_empty_string() {
        env::set_var("TEST_NETWORKS", "");
        env::set_var("TEST_MODE", "ci");
        env::remove_var("TEST_TAGS");

        let networks = get_test_networks().unwrap();
        // When TEST_NETWORKS is empty, falls back to TEST_MODE
        assert!(!networks.is_empty());
        assert!(networks.contains(&"sepolia".to_string()));

        env::remove_var("TEST_NETWORKS");
        env::remove_var("TEST_MODE");
    }

    #[test]
    fn test_load_full_testnets() {
        let result = load_full_testnets();

        assert!(result.is_ok());

        if result.is_ok() {
            let networks = result.unwrap();
            assert!(!networks.is_empty(), "Should find at least some testnets");

            // Should include at least sepolia (most configs have this)
            // Note: network names in config files might differ from our constants
            // e.g., "devnet" vs "solana-devnet", "testnet" vs "stellar-testnet"
            assert!(
                networks.contains(&"sepolia".to_string())
                    || networks.contains(&"devnet".to_string())
                    || networks.contains(&"testnet".to_string()),
                "Should contain at least one testnet: {:?}",
                networks
            );
        }
    }

    #[test]
    #[serial]
    fn test_should_test_network() {
        env::set_var("TEST_MODE", "quick");
        env::remove_var("TEST_NETWORKS");
        env::remove_var("TEST_TAGS");

        // Sepolia is tagged with "quick"
        assert!(should_test_network("sepolia").unwrap());

        // Networks not in registry or not tagged with "quick"
        assert!(!should_test_network("base-sepolia").unwrap());
        assert!(!should_test_network("non-existent-network").unwrap());

        env::remove_var("TEST_MODE");
    }
}
