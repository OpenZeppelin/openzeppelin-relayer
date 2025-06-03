//! Solana Network Configuration
//!
//! This module provides configuration support for Solana blockchain networks including
//! mainnet-beta, testnet, devnet, and custom Solana-compatible networks.
//!
//! ## Key Features
//!
//! - **Full inheritance support**: Solana networks can inherit from other Solana networks
//! - **Standard validation**: Inherits all common field validation requirements
//! - **Type safety**: Inheritance only allowed between Solana networks

use super::common::NetworkConfigCommon;
use crate::config::ConfigFileError;
use serde::{Deserialize, Serialize};

/// Configuration specific to Solana networks.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SolanaNetworkConfig {
    /// Common network fields.
    #[serde(flatten)]
    pub common: NetworkConfigCommon,
    // Additional Solana-specific fields can be added here.
}

impl SolanaNetworkConfig {
    /// Validates the specific configuration fields for a Solana network.
    ///
    /// # Returns
    /// - `Ok(())` if the Solana configuration is valid.
    /// - `Err(ConfigFileError)` if validation fails (e.g., missing fields, invalid URLs).
    pub fn validate(&self) -> Result<(), ConfigFileError> {
        self.common.validate()?;
        Ok(())
    }

    /// Merges this Solana configuration with a parent Solana configuration.
    /// Parent values are used as defaults, child values take precedence.
    pub fn merge_with_parent(&self, parent: &Self) -> Self {
        Self {
            common: self.common.merge_with_parent(&parent.common),
            // Add Solana-specific field merging here as they are added to the struct
        }
    }
}
