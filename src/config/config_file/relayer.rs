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
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ConfigFileRelayerSolanaPolicy {
    pub max_retries: Option<u32>,
    pub confirmation_blocks: Option<u64>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ConfigFileRelayerStellarPolicy {
    pub max_fee: Option<u32>,
    pub timeout_seconds: Option<u64>,
    pub min_account_balance: Option<String>,
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

        // Construct and return the struct
        Ok(RelayerFileConfig {
            id,
            name,
            network,
            paused,
            network_type,
            policies,
            signer_id,
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
mod tests {}
