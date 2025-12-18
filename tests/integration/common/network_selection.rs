//! Network selection logic for integration tests
//!
//! This module provides utilities for selecting which networks to test
//! based on the registry configuration.

use super::registry::TestRegistry;
use eyre::Result;

/// Get the list of networks to test based on the registry
///
/// Returns all networks that have `enabled: true` in the registry.
pub fn get_test_networks() -> Result<Vec<String>> {
    let registry = TestRegistry::load()?;
    let networks = registry.enabled_networks();

    if networks.is_empty() {
        return Err(eyre::eyre!("No enabled networks found in registry"));
    }

    Ok(networks)
}
