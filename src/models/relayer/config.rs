//! Configuration file representation and parsing for relayers.
//!
//! This module handles the configuration file format for relayers, providing:
//!
//! - **Config Models**: Structures that match the configuration file schema
//! - **Validation**: Config-specific validation rules and constraints
//! - **Conversions**: Bidirectional mapping between config and domain models
//! - **Collections**: Container types for managing multiple relayer configurations
//!
//! Used primarily during application startup to parse relayer settings from config files.
//! Validation is handled by the domain model in mod.rs to ensure reusability.

use super::{Relayer, RelayerNetworkPolicy, RelayerValidationError, RpcConfig};
use crate::config::{ConfigFileError, ConfigFileNetworkType, NetworksFileConfig};
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
    pub gas_price_cap: Option<u128>,
    pub whitelist_receivers: Option<Vec<String>>,
    pub eip1559_pricing: Option<bool>,
    pub private_transactions: Option<bool>,
    pub min_balance: Option<u128>,
    pub gas_limit_estimation: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct AllowedTokenSwapConfig {
    /// Conversion slippage percentage for token. Optional.
    pub slippage_percentage: Option<f32>,
    /// Minimum amount of tokens to swap. Optional.
    pub min_amount: Option<u64>,
    /// Maximum amount of tokens to swap. Optional.
    pub max_amount: Option<u64>,
    /// Minimum amount of tokens to retain after swap. Optional.
    pub retain_min_amount: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct AllowedToken {
    pub mint: String,
    /// Decimals for the token. Optional.
    pub decimals: Option<u8>,
    /// Symbol for the token. Optional.
    pub symbol: Option<String>,
    /// Maximum supported token fee (in lamports) for a transaction. Optional.
    pub max_allowed_fee: Option<u64>,
    /// Swap configuration for the token. Optional.
    pub swap_config: Option<AllowedTokenSwapConfig>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ConfigFileRelayerSolanaFeePaymentStrategy {
    User,
    Relayer,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigFileRelayerSolanaSwapStrategy {
    JupiterSwap,
    JupiterUltra,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct JupiterSwapOptions {
    /// Maximum priority fee (in lamports) for a transaction. Optional.
    pub priority_fee_max_lamports: Option<u64>,
    /// Priority. Optional.
    pub priority_level: Option<String>,

    pub dynamic_compute_unit_limit: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct ConfigFileRelayerSolanaSwapPolicy {
    /// DEX strategy to use for token swaps.
    pub strategy: Option<ConfigFileRelayerSolanaSwapStrategy>,

    /// Cron schedule for executing token swap logic to keep relayer funded. Optional.
    pub cron_schedule: Option<String>,

    /// Min sol balance to execute token swap logic to keep relayer funded. Optional.
    pub min_balance_threshold: Option<u64>,

    /// Swap options for JupiterSwap strategy. Optional.
    pub jupiter_swap_options: Option<JupiterSwapOptions>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ConfigFileRelayerSolanaPolicy {
    /// Determines if the relayer pays the transaction fee or the user. Optional.
    pub fee_payment_strategy: Option<ConfigFileRelayerSolanaFeePaymentStrategy>,

    /// Fee margin percentage for the relayer. Optional.
    pub fee_margin_percentage: Option<f32>,

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

    /// Maximum allowed fee (in lamports) for a transaction. Optional.
    pub max_allowed_fee_lamports: Option<u64>,

    /// Swap dex config to use for token swaps. Optional.
    pub swap_config: Option<ConfigFileRelayerSolanaSwapPolicy>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
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
    #[serde(default)]
    pub custom_rpc_urls: Option<Vec<RpcConfig>>,
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

        let custom_rpc_urls = value
            .get("custom_rpc_urls")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        // Handle both string format (legacy) and object format (new)
                        if let Some(url_str) = v.as_str() {
                            // Convert string to RpcConfig with default weight
                            Some(RpcConfig::new(url_str.to_string()))
                        } else {
                            // Try to parse as a RpcConfig object
                            serde_json::from_value::<RpcConfig>(v.clone()).ok()
                        }
                    })
                    .collect()
            });

        Ok(RelayerFileConfig {
            id,
            name,
            network,
            paused,
            network_type,
            policies,
            signer_id,
            notification_id,
            custom_rpc_urls,
        })
    }
}

impl TryFrom<RelayerFileConfig> for Relayer {
    type Error = ConfigFileError;

    fn try_from(config: RelayerFileConfig) -> Result<Self, Self::Error> {
        // Convert config policies to domain model policies
        let policies = if let Some(config_policies) = config.policies {
            Some(convert_config_policies_to_domain(config_policies)?)
        } else {
            None
        };

        // Create domain relayer
        let relayer = Relayer::new(
            config.id,
            config.name,
            config.network,
            config.paused,
            config.network_type.into(),
            policies,
            config.signer_id,
            config.notification_id,
            config.custom_rpc_urls,
        );

        // Validate using domain validation logic
        relayer.validate().map_err(|e| match e {
            RelayerValidationError::EmptyId => ConfigFileError::MissingField("relayer id".into()),
            RelayerValidationError::InvalidIdFormat => ConfigFileError::InvalidIdFormat(
                "ID must contain only letters, numbers, dashes and underscores".into(),
            ),
            RelayerValidationError::IdTooLong => {
                ConfigFileError::InvalidIdLength("ID length must not exceed 36 characters".into())
            }
            RelayerValidationError::EmptyName => {
                ConfigFileError::MissingField("relayer name".into())
            }
            RelayerValidationError::EmptyNetwork => ConfigFileError::MissingField("network".into()),
            RelayerValidationError::InvalidPolicy(msg) => ConfigFileError::InvalidPolicy(msg),
            RelayerValidationError::InvalidRpcUrl(msg) => {
                ConfigFileError::InvalidFormat(format!("Invalid RPC URL: {}", msg))
            }
            RelayerValidationError::InvalidRpcWeight => {
                ConfigFileError::InvalidFormat("RPC URL weight must be in range 0-100".to_string())
            }
        })?;

        Ok(relayer)
    }
}

fn convert_config_policies_to_domain(
    config_policies: ConfigFileRelayerNetworkPolicy,
) -> Result<RelayerNetworkPolicy, ConfigFileError> {
    match config_policies {
        ConfigFileRelayerNetworkPolicy::Evm(evm_policy) => {
            Ok(RelayerNetworkPolicy::Evm(super::RelayerEvmPolicy {
                min_balance: evm_policy.min_balance,
                gas_limit_estimation: evm_policy.gas_limit_estimation,
                gas_price_cap: evm_policy.gas_price_cap,
                whitelist_receivers: evm_policy.whitelist_receivers,
                eip1559_pricing: evm_policy.eip1559_pricing,
                private_transactions: evm_policy.private_transactions,
            }))
        }
        ConfigFileRelayerNetworkPolicy::Solana(solana_policy) => {
            let swap_config = if let Some(config_swap) = solana_policy.swap_config {
                Some(super::RelayerSolanaSwapPolicy {
                    strategy: config_swap.strategy.map(|s| match s {
                        ConfigFileRelayerSolanaSwapStrategy::JupiterSwap => {
                            super::RelayerSolanaSwapStrategy::JupiterSwap
                        }
                        ConfigFileRelayerSolanaSwapStrategy::JupiterUltra => {
                            super::RelayerSolanaSwapStrategy::JupiterUltra
                        }
                    }),
                    cron_schedule: config_swap.cron_schedule,
                    min_balance_threshold: config_swap.min_balance_threshold,
                    jupiter_swap_options: config_swap.jupiter_swap_options.map(|opts| {
                        super::JupiterSwapOptions {
                            priority_fee_max_lamports: opts.priority_fee_max_lamports,
                            priority_level: opts.priority_level,
                            dynamic_compute_unit_limit: opts.dynamic_compute_unit_limit,
                        }
                    }),
                })
            } else {
                None
            };

            Ok(RelayerNetworkPolicy::Solana(super::RelayerSolanaPolicy {
                allowed_programs: solana_policy.allowed_programs,
                max_signatures: solana_policy.max_signatures,
                max_tx_data_size: solana_policy.max_tx_data_size,
                min_balance: solana_policy.min_balance,
                allowed_tokens: solana_policy.allowed_tokens.map(|tokens| {
                    tokens
                        .into_iter()
                        .map(|t| super::AllowedToken {
                            mint: t.mint,
                            decimals: t.decimals,
                            symbol: t.symbol,
                            max_allowed_fee: t.max_allowed_fee,
                            swap_config: t.swap_config.map(|sc| super::AllowedTokenSwapConfig {
                                slippage_percentage: sc.slippage_percentage,
                                min_amount: sc.min_amount,
                                max_amount: sc.max_amount,
                                retain_min_amount: sc.retain_min_amount,
                            }),
                        })
                        .collect()
                }),
                fee_payment_strategy: solana_policy.fee_payment_strategy.map(|s| match s {
                    ConfigFileRelayerSolanaFeePaymentStrategy::User => {
                        super::RelayerSolanaFeePaymentStrategy::User
                    }
                    ConfigFileRelayerSolanaFeePaymentStrategy::Relayer => {
                        super::RelayerSolanaFeePaymentStrategy::Relayer
                    }
                }),
                fee_margin_percentage: solana_policy.fee_margin_percentage,
                allowed_accounts: solana_policy.allowed_accounts,
                disallowed_accounts: solana_policy.disallowed_accounts,
                max_allowed_fee_lamports: solana_policy.max_allowed_fee_lamports,
                swap_config,
            }))
        }
        ConfigFileRelayerNetworkPolicy::Stellar(stellar_policy) => {
            Ok(RelayerNetworkPolicy::Stellar(super::RelayerStellarPolicy {
                min_balance: stellar_policy.min_balance,
                max_fee: stellar_policy.max_fee,
                timeout_seconds: stellar_policy.timeout_seconds,
            }))
        }
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

    pub fn validate(&self, networks: &NetworksFileConfig) -> Result<(), ConfigFileError> {
        if self.relayers.is_empty() {
            return Err(ConfigFileError::MissingField("relayers".into()));
        }

        let mut ids = HashSet::new();
        for relayer_config in &self.relayers {
            if relayer_config.network.is_empty() {
                return Err(ConfigFileError::InvalidFormat(
                    "relayer.network cannot be empty".into(),
                ));
            }

            if networks
                .get_network(relayer_config.network_type, &relayer_config.network)
                .is_none()
            {
                return Err(ConfigFileError::InvalidReference(format!(
                    "Relayer '{}' references non-existent network '{}' for type '{:?}'",
                    relayer_config.id, relayer_config.network, relayer_config.network_type
                )));
            }

            // Convert to domain model and validate
            let relayer = Relayer::try_from(relayer_config.clone())?;
            relayer.validate().map_err(|e| match e {
                RelayerValidationError::EmptyId => {
                    ConfigFileError::MissingField("relayer id".into())
                }
                RelayerValidationError::InvalidIdFormat => ConfigFileError::InvalidIdFormat(
                    "ID must contain only letters, numbers, dashes and underscores".into(),
                ),
                RelayerValidationError::IdTooLong => ConfigFileError::InvalidIdLength(
                    "ID length must not exceed 36 characters".into(),
                ),
                RelayerValidationError::EmptyName => {
                    ConfigFileError::MissingField("relayer name".into())
                }
                RelayerValidationError::EmptyNetwork => {
                    ConfigFileError::MissingField("network".into())
                }
                RelayerValidationError::InvalidPolicy(msg) => ConfigFileError::InvalidPolicy(msg),
                RelayerValidationError::InvalidRpcUrl(msg) => {
                    ConfigFileError::InvalidFormat(format!("Invalid RPC URL: {}", msg))
                }
                RelayerValidationError::InvalidRpcWeight => ConfigFileError::InvalidFormat(
                    "RPC URL weight must be in range 0-100".to_string(),
                ),
            })?;

            if !ids.insert(relayer_config.id.clone()) {
                return Err(ConfigFileError::DuplicateId(relayer_config.id.clone()));
            }
        }
        Ok(())
    }
}
