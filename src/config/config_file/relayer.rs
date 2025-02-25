//! Configuration file definitions for relayer services.
//!
//! Provides configuration structures and validation for relayer settings:
//! - Network configuration (EVM, Solana, Stellar)
//! - Gas/fee policies
//! - Transaction validation rules
//! - Network endpoints
use super::{ConfigFileError, ConfigFileNetworkType};
use crate::models::{EvmNetwork, SolanaNetwork, StellarNetwork};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum ConfigFileRelayerNetworkPolicy {
    Evm(ConfigFileRelayerEvmPolicy),
    Solana(ConfigFileRelayerSolanaPolicy),
    Stellar(ConfigFileRelayerStellarPolicy),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ConfigFileRelayerEvmPolicy {
    pub gas_price_cap: Option<u64>,
    pub whitelist_receivers: Option<Vec<String>>,
    pub eip1559_pricing: Option<bool>,
    pub private_transactions: Option<bool>,
    pub min_balance: Option<u128>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AllowedToken {
    pub mint: String,
    /// Maximum supported token fee (in lamports) for a transaction. Optional.
    pub max_allowed_fee: Option<u64>,
    // Conversion slippage percentage for token. Optional.
    pub conversion_slippage_percentage: Option<f32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ConfigFileRelayerSolanaPolicy {
    /// Minimum balance required for the relayer (in lamports). Optional.
    pub min_balance: Option<u64>,

    /// List of allowed tokens by their identifiers. Only these tokens are supported if provided.
    pub allowed_tokens: Option<Vec<AllowedToken>>,

    /// List of allowed programs by their identifiers. Only these programs are supported if
    /// provided.
    pub allowed_programs: Option<Vec<String>>,

    /// List of allowed accounts by their public keys. The relayer will only operate with these
    /// accounts if provided.
    pub allowed_accounts: Option<Vec<String>>,

    /// List of disallowed accounts by their public keys. These accounts will be explicitly
    /// blocked.
    pub disallowed_accounts: Option<Vec<String>>,

    /// Maximum transaction size. Optional.
    pub max_tx_data_size: Option<u16>,

    /// Maximum supported signatures. Optional.
    pub max_signatures: Option<u8>,

    /// Maximum allowed transfer amount (in lamports) for a transaction. Optional.
    pub max_allowed_transfer_amount_lamports: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ConfigFileRelayerStellarPolicy {
    pub max_fee: Option<u32>,
    pub timeout_seconds: Option<u64>,
    pub min_balance: Option<u64>,
}

#[derive(Debug, Serialize, Clone)]
pub struct RelayerFileConfig {
    pub id: String,
    pub name: String,
    pub network: String,
    pub paused: bool,
    #[serde(flatten)]
    pub network_type: ConfigFileNetworkType,
    #[serde(default)]
    pub policies: Option<ConfigFileRelayerNetworkPolicy>,
    pub signer_id: String,
    #[serde(default)]
    pub notification_id: Option<String>,
}
use serde::{de, Deserializer};
use serde_json::Value;

impl<'de> Deserialize<'de> for RelayerFileConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Deserialize as a generic JSON object
        let mut value: Value = Value::deserialize(deserializer)?;

        // Extract and validate required fields
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| de::Error::missing_field("id"))?
            .to_string();

        let name = value
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| de::Error::missing_field("name"))?
            .to_string();

        let network = value
            .get("network")
            .and_then(Value::as_str)
            .ok_or_else(|| de::Error::missing_field("network"))?
            .to_string();

        let paused = value
            .get("paused")
            .and_then(Value::as_bool)
            .ok_or_else(|| de::Error::missing_field("paused"))?;

        // Deserialize `network_type` using `ConfigFileNetworkType`
        let network_type: ConfigFileNetworkType = serde_json::from_value(
            value
                .get("network_type")
                .cloned()
                .ok_or_else(|| de::Error::missing_field("network_type"))?,
        )
        .map_err(de::Error::custom)?;

        let signer_id = value
            .get("signer_id")
            .and_then(Value::as_str)
            .ok_or_else(|| de::Error::missing_field("signer_id"))?
            .to_string();

        let notification_id = value
            .get("notification_id")
            .and_then(Value::as_str)
            .map(|s| s.to_string());

        // Handle `policies`, using `network_type` to determine how to deserialize
        let policies = if let Some(policy_value) = value.get_mut("policies") {
            match network_type {
                ConfigFileNetworkType::Evm => {
                    serde_json::from_value::<ConfigFileRelayerEvmPolicy>(policy_value.clone())
                        .map(ConfigFileRelayerNetworkPolicy::Evm)
                        .map(Some)
                        .map_err(de::Error::custom)
                }
                ConfigFileNetworkType::Solana => {
                    serde_json::from_value::<ConfigFileRelayerSolanaPolicy>(policy_value.clone())
                        .map(ConfigFileRelayerNetworkPolicy::Solana)
                        .map(Some)
                        .map_err(de::Error::custom)
                }
                ConfigFileNetworkType::Stellar => {
                    serde_json::from_value::<ConfigFileRelayerStellarPolicy>(policy_value.clone())
                        .map(ConfigFileRelayerNetworkPolicy::Stellar)
                        .map(Some)
                        .map_err(de::Error::custom)
                }
            }
        } else {
            Ok(None) // `policies` is optional
        }?;

        Ok(RelayerFileConfig {
            id,
            name,
            network,
            paused,
            network_type,
            policies,
            signer_id,
            notification_id,
        })
    }
}

impl RelayerFileConfig {
    const MAX_ID_LENGTH: usize = 36;

    fn validate_network(&self) -> Result<(), ConfigFileError> {
        match self.network_type {
            ConfigFileNetworkType::Evm => {
                if EvmNetwork::from_network_str(&self.network).is_err() {
                    return Err(ConfigFileError::InvalidNetwork {
                        network_type: "EVM".to_string(),
                        name: self.network.clone(),
                    });
                }
            }
            ConfigFileNetworkType::Stellar => {
                if StellarNetwork::from_network_str(&self.network).is_err() {
                    return Err(ConfigFileError::InvalidNetwork {
                        network_type: "Stellar".to_string(),
                        name: self.network.clone(),
                    });
                }
            }
            ConfigFileNetworkType::Solana => {
                if SolanaNetwork::from_network_str(&self.network).is_err() {
                    return Err(ConfigFileError::InvalidNetwork {
                        network_type: "Solana".to_string(),
                        name: self.network.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    fn validate_solana_pub_keys(&self, keys: &Option<Vec<String>>) -> Result<(), ConfigFileError> {
        if let Some(keys) = keys {
            let solana_pub_key_regex =
                Regex::new(r"^[1-9A-HJ-NP-Za-km-z]{32,44}$").map_err(|e| {
                    ConfigFileError::InternalError(format!("Regex compilation error: {}", e))
                })?;
            for key in keys {
                if !solana_pub_key_regex.is_match(key) {
                    return Err(ConfigFileError::InvalidPolicy(
                        "Value must contain only letters, numbers, dashes and underscores".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn validate_policies(&self) -> Result<(), ConfigFileError> {
        match self.network_type {
            ConfigFileNetworkType::Solana => {
                if let Some(ConfigFileRelayerNetworkPolicy::Solana(policy)) = &self.policies {
                    self.validate_solana_pub_keys(&policy.allowed_accounts)?;
                    self.validate_solana_pub_keys(&policy.disallowed_accounts)?;
                    let allowed_token_keys = policy.allowed_tokens.as_ref().map(|tokens| {
                        tokens
                            .iter()
                            .map(|token| token.mint.clone())
                            .collect::<Vec<String>>()
                    });
                    self.validate_solana_pub_keys(&allowed_token_keys)?;
                    self.validate_solana_pub_keys(&policy.allowed_programs)?;
                    // check if both allowed_accounts and disallowed_accounts are present
                    if policy.allowed_accounts.is_some() && policy.disallowed_accounts.is_some() {
                        return Err(ConfigFileError::InvalidPolicy(
                            "allowed_accounts and disallowed_accounts cannot be both present"
                                .into(),
                        ));
                    }
                }
            }
            ConfigFileNetworkType::Evm => {}
            ConfigFileNetworkType::Stellar => {}
        }
        Ok(())
    }

    // TODO add validation that multiple relayers on same network cannot use same signer
    pub fn validate(&self) -> Result<(), ConfigFileError> {
        if self.id.is_empty() {
            return Err(ConfigFileError::MissingField("relayer id".into()));
        }
        let id_regex = Regex::new(r"^[a-zA-Z0-9-_]+$").map_err(|e| {
            ConfigFileError::InternalError(format!("Regex compilation error: {}", e))
        })?;
        if !id_regex.is_match(&self.id) {
            return Err(ConfigFileError::InvalidIdFormat(
                "ID must contain only letters, numbers, dashes and underscores".into(),
            ));
        }

        if self.id.len() > Self::MAX_ID_LENGTH {
            return Err(ConfigFileError::InvalidIdLength(format!(
                "ID length must not exceed {} characters",
                Self::MAX_ID_LENGTH
            )));
        }
        if self.name.is_empty() {
            return Err(ConfigFileError::MissingField("relayer name".into()));
        }
        if self.network.is_empty() {
            return Err(ConfigFileError::MissingField("network".into()));
        }

        self.validate_network()?;

        self.validate_policies()?;
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RelayersFileConfig {
    pub relayers: Vec<RelayerFileConfig>,
}

impl RelayersFileConfig {
    pub fn new(relayers: Vec<RelayerFileConfig>) -> Self {
        Self { relayers }
    }

    pub fn validate(&self) -> Result<(), ConfigFileError> {
        if self.relayers.is_empty() {
            return Err(ConfigFileError::MissingField("relayers".into()));
        }

        let mut ids = HashSet::new();
        for relayer in &self.relayers {
            relayer.validate()?;
            if !ids.insert(relayer.id.clone()) {
                return Err(ConfigFileError::DuplicateId(relayer.id.clone()));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_solana_policy_duplicate_entries() {
        let config = json!({
            "id": "solana-relayer",
            "name": "Solana Mainnet Relayer",
            "network": "mainnet",
            "network_type": "solana",
            "signer_id": "solana-signer",
            "paused": false,
            "policies": {
                "allowed_accounts": ["EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"],
                "disallowed_accounts": ["EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"],
            }
        });

        let relayer: RelayerFileConfig = serde_json::from_value(config).unwrap();

        let err = relayer.validate_policies().unwrap_err();

        assert_eq!(
            err.to_string(),
            "Invalid policy: allowed_accounts and disallowed_accounts cannot be both present"
        );
    }

    #[test]
    fn test_solana_policy_format() {
        let config = json!({
            "id": "solana-relayer",
            "name": "Solana Mainnet Relayer",
            "network": "mainnet",
            "network_type": "solana",
            "signer_id": "solana-signer",
            "paused": false,
            "policies": {
                "min_balance": 100,
                "allowed_tokens": [ {"mint": "token1"}, {"mint": "token2"}],
            }
        });

        let relayer: RelayerFileConfig = serde_json::from_value(config).unwrap();

        let err = relayer.validate_policies().unwrap_err();

        assert_eq!(
            err.to_string(),
            "Invalid policy: Value must contain only letters, numbers, dashes and underscores"
        );
    }

    #[test]
    fn test_valid_evm_relayer() {
        let config = json!({
            "id": "test-relayer",
            "name": "Test Relayer",
            "network": "mainnet",
            "network_type": "evm",
            "signer_id": "test-signer",
            "paused": false,
            "policies": {
                "gas_price_cap": 100,
                "whitelist_receivers": ["0x1234"],
                "eip1559_pricing": true
            }
        });

        let relayer: RelayerFileConfig = serde_json::from_value(config).unwrap();
        assert!(relayer.validate().is_ok());
        assert_eq!(relayer.id, "test-relayer");
        assert_eq!(relayer.network_type, ConfigFileNetworkType::Evm);
    }

    #[test]
    fn test_valid_solana_relayer() {
        let config = json!({
            "id": "solana-relayer",
            "name": "Solana Mainnet Relayer",
            "network": "mainnet-beta",
            "network_type": "solana",
            "signer_id": "solana-signer",
            "paused": false,
            "policies": {
                "min_balance": 100,
                "disallowed_accounts": ["HCKHoE2jyk1qfAwpHQghvYH3cEfT8euCygBzF9AV6bhY"],
            }
        });

        let relayer: RelayerFileConfig = serde_json::from_value(config).unwrap();
        assert!(relayer.validate().is_ok());
        assert_eq!(relayer.id, "solana-relayer");
        assert_eq!(relayer.network_type, ConfigFileNetworkType::Solana);
    }

    #[test]
    fn test_valid_stellar_relayer() {
        let config = json!({
            "id": "stellar-relayer",
            "name": "Stellar Public Relayer",
            "network": "mainnet",
            "network_type": "stellar",
            "signer_id": "stellar_signer",
            "paused": false,
            "policies": {
                "max_fee": 100,
                "timeout_seconds": 10,
                "min_balance": 100
            }
        });

        let relayer: RelayerFileConfig = serde_json::from_value(config).unwrap();
        assert!(relayer.validate().is_ok());
        assert_eq!(relayer.id, "stellar-relayer");
        assert_eq!(relayer.network_type, ConfigFileNetworkType::Stellar);
    }

    #[test]
    fn test_invalid_network_type() {
        let config = json!({
            "id": "test-relayer",
            "network_type": "invalid",
            "signer_id": "test-signer"
        });

        let result = serde_json::from_value::<RelayerFileConfig>(config);
        assert!(result.is_err());
    }

    #[test]
    #[should_panic(expected = "missing field `name`")]
    fn test_missing_required_fields() {
        let config = json!({
            "id": "test-relayer"
        });

        let _relayer: RelayerFileConfig = serde_json::from_value(config).unwrap();
    }
}
