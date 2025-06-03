//! Stellar Network Configuration
//!
//! This module provides configuration support for Stellar blockchain networks including
//! Stellar mainnet (Pubnet), testnet, and custom Stellar-compatible networks.
//!
//! ## Key Features
//!
//! - **Full inheritance support**: Stellar networks can inherit from other Stellar networks
//! - **Network passphrase**: Critical field for transaction signing and network identification
//! - **Standard validation**: Inherits all common field validation requirements
//! - **Type safety**: Inheritance only allowed between Stellar networks

use super::common::NetworkConfigCommon;
use crate::config::ConfigFileError;
use serde::{Deserialize, Serialize};

/// Configuration specific to Stellar networks.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct StellarNetworkConfig {
    /// Common network fields.
    #[serde(flatten)]
    pub common: NetworkConfigCommon,
    /// The passphrase for the Stellar network.
    pub passphrase: Option<String>,
    // Additional Stellar-specific fields can be added here.
}

impl StellarNetworkConfig {
    /// Validates the specific configuration fields for a Stellar network.
    ///
    /// # Returns
    /// - `Ok(())` if the Stellar configuration is valid.
    /// - `Err(ConfigFileError)` if validation fails (e.g., missing fields, invalid URLs).
    pub fn validate(&self) -> Result<(), ConfigFileError> {
        self.common.validate()?;
        Ok(())
    }

    /// Merges this Stellar configuration with a parent Stellar configuration.
    /// Parent values are used as defaults, child values take precedence.
    pub fn merge_with_parent(&self, parent: &Self) -> Self {
        Self {
            common: self.common.merge_with_parent(&parent.common),
            passphrase: self
                .passphrase
                .clone()
                .or_else(|| parent.passphrase.clone()),
            // Add Stellar-specific field merging here as they are added to the struct
        }
    }
}
