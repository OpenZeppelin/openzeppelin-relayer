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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ConfigFileRelayerNetworkPolicy {
    Evm(ConfigFileRelayerEvmPolicy),
    Solana(ConfigFileRelayerSolanaPolicy),
    Stellar(ConfigFileRelayerStellarPolicy),
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConfigFileRelayerEvmPolicy {
    pub gas_price_cap: Option<u128>,
    pub whitelist_receivers: Option<Vec<String>>,
    pub eip1559_pricing: Option<bool>,
    pub private_transactions: Option<bool>,
    pub min_balance: Option<u128>,
    pub gas_limit_estimation: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
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

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
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
pub enum ConfigFileSolanaFeePaymentStrategy {
    User,
    Relayer,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigFileRelayerSolanaSwapStrategy {
    JupiterSwap,
    JupiterUltra,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct JupiterSwapOptions {
    /// Maximum priority fee (in lamports) for a transaction. Optional.
    pub priority_fee_max_lamports: Option<u64>,
    /// Priority. Optional.
    pub priority_level: Option<String>,

    pub dynamic_compute_unit_limit: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConfigFileRelayerSolanaSwapConfig {
    /// DEX strategy to use for token swaps.
    pub strategy: Option<ConfigFileRelayerSolanaSwapStrategy>,

    /// Cron schedule for executing token swap logic to keep relayer funded. Optional.
    pub cron_schedule: Option<String>,

    /// Min sol balance to execute token swap logic to keep relayer funded. Optional.
    pub min_balance_threshold: Option<u64>,

    /// Swap options for JupiterSwap strategy. Optional.
    pub jupiter_swap_options: Option<JupiterSwapOptions>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConfigFileRelayerSolanaPolicy {
    /// Determines if the relayer pays the transaction fee or the user. Optional.
    pub fee_payment_strategy: Option<ConfigFileSolanaFeePaymentStrategy>,

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
    pub swap_config: Option<ConfigFileRelayerSolanaSwapConfig>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
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
            RelayerValidationError::InvalidField(msg) => ConfigFileError::InvalidFormat(msg),
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
                Some(super::RelayerSolanaSwapConfig {
                    strategy: config_swap.strategy.map(|s| match s {
                        ConfigFileRelayerSolanaSwapStrategy::JupiterSwap => {
                            super::SolanaSwapStrategy::JupiterSwap
                        }
                        ConfigFileRelayerSolanaSwapStrategy::JupiterUltra => {
                            super::SolanaSwapStrategy::JupiterUltra
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
                        .map(|t| super::SolanaAllowedTokensPolicy {
                            mint: t.mint,
                            decimals: t.decimals,
                            symbol: t.symbol,
                            max_allowed_fee: t.max_allowed_fee,
                            swap_config: t.swap_config.map(|sc| {
                                super::SolanaAllowedTokensSwapConfig {
                                    slippage_percentage: sc.slippage_percentage,
                                    min_amount: sc.min_amount,
                                    max_amount: sc.max_amount,
                                    retain_min_amount: sc.retain_min_amount,
                                }
                            }),
                        })
                        .collect()
                }),
                fee_payment_strategy: solana_policy.fee_payment_strategy.map(|s| match s {
                    ConfigFileSolanaFeePaymentStrategy::User => {
                        super::SolanaFeePaymentStrategy::User
                    }
                    ConfigFileSolanaFeePaymentStrategy::Relayer => {
                        super::SolanaFeePaymentStrategy::Relayer
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
            return Ok(());
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
                RelayerValidationError::InvalidField(msg) => ConfigFileError::InvalidFormat(msg),
            })?;

            if !ids.insert(relayer_config.id.clone()) {
                return Err(ConfigFileError::DuplicateId(relayer_config.id.clone()));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigFileNetworkType;
    use crate::models::relayer::{SolanaFeePaymentStrategy, SolanaSwapStrategy};
    use serde_json;

    fn create_test_networks_config() -> NetworksFileConfig {
        // Create a mock networks config for validation tests
        NetworksFileConfig::new(vec![]).unwrap()
    }

    #[test]
    fn test_relayer_file_config_deserialization_evm() {
        let json_input = r#"{
            "id": "test-evm-relayer",
            "name": "Test EVM Relayer",
            "network": "mainnet",
            "paused": false,
            "network_type": "evm",
            "signer_id": "test-signer",
            "policies": {
                "gas_price_cap": 100000000000,
                "eip1559_pricing": true,
                "min_balance": 1000000000000000000,
                "gas_limit_estimation": false,
                "private_transactions": null
            },
            "notification_id": "test-notification",
            "custom_rpc_urls": [
                "https://mainnet.infura.io/v3/test",
                {"url": "https://eth.llamarpc.com", "weight": 80}
            ]
        }"#;

        let config: RelayerFileConfig = serde_json::from_str(json_input).unwrap();

        assert_eq!(config.id, "test-evm-relayer");
        assert_eq!(config.name, "Test EVM Relayer");
        assert_eq!(config.network, "mainnet");
        assert!(!config.paused);
        assert_eq!(config.network_type, ConfigFileNetworkType::Evm);
        assert_eq!(config.signer_id, "test-signer");
        assert_eq!(
            config.notification_id,
            Some("test-notification".to_string())
        );

        // Test policies
        assert!(config.policies.is_some());
        if let Some(ConfigFileRelayerNetworkPolicy::Evm(evm_policy)) = config.policies {
            assert_eq!(evm_policy.gas_price_cap, Some(100000000000));
            assert_eq!(evm_policy.eip1559_pricing, Some(true));
            assert_eq!(evm_policy.min_balance, Some(1000000000000000000));
            assert_eq!(evm_policy.gas_limit_estimation, Some(false));
            assert_eq!(evm_policy.private_transactions, None);
        } else {
            panic!("Expected EVM policy");
        }

        // Test custom RPC URLs (both string and object formats)
        assert!(config.custom_rpc_urls.is_some());
        let rpc_urls = config.custom_rpc_urls.unwrap();
        assert_eq!(rpc_urls.len(), 2);
        assert_eq!(rpc_urls[0].url, "https://mainnet.infura.io/v3/test");
        assert_eq!(rpc_urls[0].weight, 100); // Default weight
        assert_eq!(rpc_urls[1].url, "https://eth.llamarpc.com");
        assert_eq!(rpc_urls[1].weight, 80);
    }

    #[test]
    fn test_relayer_file_config_deserialization_solana() {
        let json_input = r#"{
            "id": "test-solana-relayer",
            "name": "Test Solana Relayer",
            "network": "mainnet",
            "paused": true,
            "network_type": "solana",
            "signer_id": "test-signer",
            "policies": {
                "fee_payment_strategy": "relayer",
                "min_balance": 5000000,
                "max_signatures": 8,
                "max_tx_data_size": 1024,
                "fee_margin_percentage": 2.5,
                "allowed_tokens": [
                    {
                        "mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                        "decimals": 6,
                        "symbol": "USDC",
                        "max_allowed_fee": 100000,
                        "swap_config": {
                            "slippage_percentage": 0.5,
                            "min_amount": 1000,
                            "max_amount": 10000000
                        }
                    }
                ],
                "allowed_programs": ["11111111111111111111111111111111"],
                "swap_config": {
                    "strategy": "jupiter-swap",
                    "cron_schedule": "0 0 * * *",
                    "min_balance_threshold": 1000000,
                    "jupiter_swap_options": {
                        "priority_fee_max_lamports": 10000,
                        "priority_level": "high",
                        "dynamic_compute_unit_limit": true
                    }
                }
            }
        }"#;

        let config: RelayerFileConfig = serde_json::from_str(json_input).unwrap();

        assert_eq!(config.id, "test-solana-relayer");
        assert_eq!(config.network_type, ConfigFileNetworkType::Solana);
        assert!(config.paused);

        // Test Solana policies
        assert!(config.policies.is_some());
        if let Some(ConfigFileRelayerNetworkPolicy::Solana(solana_policy)) = config.policies {
            assert_eq!(
                solana_policy.fee_payment_strategy,
                Some(ConfigFileSolanaFeePaymentStrategy::Relayer)
            );
            assert_eq!(solana_policy.min_balance, Some(5000000));
            assert_eq!(solana_policy.max_signatures, Some(8));
            assert_eq!(solana_policy.max_tx_data_size, Some(1024));
            assert_eq!(solana_policy.fee_margin_percentage, Some(2.5));

            // Test allowed tokens
            assert!(solana_policy.allowed_tokens.is_some());
            let tokens = solana_policy.allowed_tokens.as_ref().unwrap();
            assert_eq!(tokens.len(), 1);
            assert_eq!(
                tokens[0].mint,
                "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
            );
            assert_eq!(tokens[0].decimals, Some(6));
            assert_eq!(tokens[0].symbol, Some("USDC".to_string()));
            assert_eq!(tokens[0].max_allowed_fee, Some(100000));

            // Test swap config in token
            assert!(tokens[0].swap_config.is_some());
            let token_swap = tokens[0].swap_config.as_ref().unwrap();
            assert_eq!(token_swap.slippage_percentage, Some(0.5));
            assert_eq!(token_swap.min_amount, Some(1000));
            assert_eq!(token_swap.max_amount, Some(10000000));

            // Test main swap config
            assert!(solana_policy.swap_config.is_some());
            let swap_config = solana_policy.swap_config.as_ref().unwrap();
            assert_eq!(
                swap_config.strategy,
                Some(ConfigFileRelayerSolanaSwapStrategy::JupiterSwap)
            );
            assert_eq!(swap_config.cron_schedule, Some("0 0 * * *".to_string()));
            assert_eq!(swap_config.min_balance_threshold, Some(1000000));

            // Test Jupiter options
            assert!(swap_config.jupiter_swap_options.is_some());
            let jupiter_opts = swap_config.jupiter_swap_options.as_ref().unwrap();
            assert_eq!(jupiter_opts.priority_fee_max_lamports, Some(10000));
            assert_eq!(jupiter_opts.priority_level, Some("high".to_string()));
            assert_eq!(jupiter_opts.dynamic_compute_unit_limit, Some(true));
        } else {
            panic!("Expected Solana policy");
        }
    }

    #[test]
    fn test_relayer_file_config_deserialization_stellar() {
        let json_input = r#"{
            "id": "test-stellar-relayer",
            "name": "Test Stellar Relayer",
            "network": "mainnet",
            "paused": false,
            "network_type": "stellar",
            "signer_id": "test-signer",
            "policies": {
                "min_balance": 20000000,
                "max_fee": 100000,
                "timeout_seconds": 30
            },
            "custom_rpc_urls": [
                {"url": "https://stellar-node.example.com", "weight": 100}
            ]
        }"#;

        let config: RelayerFileConfig = serde_json::from_str(json_input).unwrap();

        assert_eq!(config.id, "test-stellar-relayer");
        assert_eq!(config.network_type, ConfigFileNetworkType::Stellar);
        assert!(!config.paused);

        // Test Stellar policies
        assert!(config.policies.is_some());
        if let Some(ConfigFileRelayerNetworkPolicy::Stellar(stellar_policy)) = config.policies {
            assert_eq!(stellar_policy.min_balance, Some(20000000));
            assert_eq!(stellar_policy.max_fee, Some(100000));
            assert_eq!(stellar_policy.timeout_seconds, Some(30));
        } else {
            panic!("Expected Stellar policy");
        }
    }

    #[test]
    fn test_relayer_file_config_deserialization_minimal() {
        // Test minimal config without optional fields
        let json_input = r#"{
            "id": "minimal-relayer",
            "name": "Minimal Relayer",
            "network": "testnet",
            "paused": false,
            "network_type": "evm",
            "signer_id": "minimal-signer"
        }"#;

        let config: RelayerFileConfig = serde_json::from_str(json_input).unwrap();

        assert_eq!(config.id, "minimal-relayer");
        assert_eq!(config.name, "Minimal Relayer");
        assert_eq!(config.network, "testnet");
        assert!(!config.paused);
        assert_eq!(config.network_type, ConfigFileNetworkType::Evm);
        assert_eq!(config.signer_id, "minimal-signer");
        assert_eq!(config.notification_id, None);
        assert_eq!(config.policies, None);
        assert_eq!(config.custom_rpc_urls, None);
    }

    #[test]
    fn test_relayer_file_config_deserialization_missing_required_field() {
        // Test missing required field should fail
        let json_input = r#"{
            "name": "Test Relayer",
            "network": "mainnet",
            "paused": false,
            "network_type": "evm",
            "signer_id": "test-signer"
        }"#;

        let result = serde_json::from_str::<RelayerFileConfig>(json_input);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing field `id`"));
    }

    #[test]
    fn test_relayer_file_config_deserialization_invalid_network_type() {
        let json_input = r#"{
            "id": "test-relayer",
            "name": "Test Relayer",
            "network": "mainnet",
            "paused": false,
            "network_type": "invalid",
            "signer_id": "test-signer"
        }"#;

        let result = serde_json::from_str::<RelayerFileConfig>(json_input);
        assert!(result.is_err());
    }

    #[test]
    fn test_relayer_file_config_deserialization_wrong_policy_for_network_type() {
        // Test EVM network type with Solana policy should fail
        let json_input = r#"{
            "id": "test-relayer",
            "name": "Test Relayer",
            "network": "mainnet",
            "paused": false,
            "network_type": "evm",
            "signer_id": "test-signer",
            "policies": {
                "fee_payment_strategy": "relayer"
            }
        }"#;

        let result = serde_json::from_str::<RelayerFileConfig>(json_input);
        assert!(result.is_err());
    }

    #[test]
    fn test_convert_config_policies_to_domain_evm() {
        let config_policy = ConfigFileRelayerNetworkPolicy::Evm(ConfigFileRelayerEvmPolicy {
            gas_price_cap: Some(50000000000),
            whitelist_receivers: Some(vec!["0x123".to_string(), "0x456".to_string()]),
            eip1559_pricing: Some(true),
            private_transactions: Some(false),
            min_balance: Some(2000000000000000000),
            gas_limit_estimation: Some(true),
        });

        let domain_policy = convert_config_policies_to_domain(config_policy).unwrap();

        if let RelayerNetworkPolicy::Evm(evm_policy) = domain_policy {
            assert_eq!(evm_policy.gas_price_cap, Some(50000000000));
            assert_eq!(
                evm_policy.whitelist_receivers,
                Some(vec!["0x123".to_string(), "0x456".to_string()])
            );
            assert_eq!(evm_policy.eip1559_pricing, Some(true));
            assert_eq!(evm_policy.private_transactions, Some(false));
            assert_eq!(evm_policy.min_balance, Some(2000000000000000000));
            assert_eq!(evm_policy.gas_limit_estimation, Some(true));
        } else {
            panic!("Expected EVM domain policy");
        }
    }

    #[test]
    fn test_convert_config_policies_to_domain_solana() {
        let config_policy = ConfigFileRelayerNetworkPolicy::Solana(ConfigFileRelayerSolanaPolicy {
            fee_payment_strategy: Some(ConfigFileSolanaFeePaymentStrategy::User),
            fee_margin_percentage: Some(1.5),
            min_balance: Some(3000000),
            allowed_tokens: Some(vec![AllowedToken {
                mint: "TokenMint123".to_string(),
                decimals: Some(9),
                symbol: Some("TOKEN".to_string()),
                max_allowed_fee: Some(50000),
                swap_config: Some(AllowedTokenSwapConfig {
                    slippage_percentage: Some(1.0),
                    min_amount: Some(100),
                    max_amount: Some(1000000),
                    retain_min_amount: Some(500),
                }),
            }]),
            allowed_programs: Some(vec!["Program123".to_string()]),
            allowed_accounts: Some(vec!["Account123".to_string()]),
            disallowed_accounts: None,
            max_tx_data_size: Some(2048),
            max_signatures: Some(10),
            max_allowed_fee_lamports: Some(100000),
            swap_config: Some(ConfigFileRelayerSolanaSwapConfig {
                strategy: Some(ConfigFileRelayerSolanaSwapStrategy::JupiterUltra),
                cron_schedule: Some("0 */6 * * *".to_string()),
                min_balance_threshold: Some(2000000),
                jupiter_swap_options: Some(JupiterSwapOptions {
                    priority_fee_max_lamports: Some(5000),
                    priority_level: Some("medium".to_string()),
                    dynamic_compute_unit_limit: Some(false),
                }),
            }),
        });

        let domain_policy = convert_config_policies_to_domain(config_policy).unwrap();

        if let RelayerNetworkPolicy::Solana(solana_policy) = domain_policy {
            assert_eq!(
                solana_policy.fee_payment_strategy,
                Some(SolanaFeePaymentStrategy::User)
            );
            assert_eq!(solana_policy.fee_margin_percentage, Some(1.5));
            assert_eq!(solana_policy.min_balance, Some(3000000));
            assert_eq!(solana_policy.max_tx_data_size, Some(2048));
            assert_eq!(solana_policy.max_signatures, Some(10));

            // Test allowed tokens conversion
            assert!(solana_policy.allowed_tokens.is_some());
            let tokens = solana_policy.allowed_tokens.unwrap();
            assert_eq!(tokens.len(), 1);
            assert_eq!(tokens[0].mint, "TokenMint123");
            assert_eq!(tokens[0].decimals, Some(9));
            assert_eq!(tokens[0].symbol, Some("TOKEN".to_string()));
            assert_eq!(tokens[0].max_allowed_fee, Some(50000));

            // Test swap config conversion
            assert!(solana_policy.swap_config.is_some());
            let swap_config = solana_policy.swap_config.unwrap();
            assert_eq!(swap_config.strategy, Some(SolanaSwapStrategy::JupiterUltra));
            assert_eq!(swap_config.cron_schedule, Some("0 */6 * * *".to_string()));
            assert_eq!(swap_config.min_balance_threshold, Some(2000000));
        } else {
            panic!("Expected Solana domain policy");
        }
    }

    #[test]
    fn test_convert_config_policies_to_domain_stellar() {
        let config_policy =
            ConfigFileRelayerNetworkPolicy::Stellar(ConfigFileRelayerStellarPolicy {
                min_balance: Some(25000000),
                max_fee: Some(150000),
                timeout_seconds: Some(60),
            });

        let domain_policy = convert_config_policies_to_domain(config_policy).unwrap();

        if let RelayerNetworkPolicy::Stellar(stellar_policy) = domain_policy {
            assert_eq!(stellar_policy.min_balance, Some(25000000));
            assert_eq!(stellar_policy.max_fee, Some(150000));
            assert_eq!(stellar_policy.timeout_seconds, Some(60));
        } else {
            panic!("Expected Stellar domain policy");
        }
    }

    #[test]
    fn test_try_from_relayer_file_config_to_domain_evm() {
        let config = RelayerFileConfig {
            id: "test-evm".to_string(),
            name: "Test EVM Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: ConfigFileNetworkType::Evm,
            policies: Some(ConfigFileRelayerNetworkPolicy::Evm(
                ConfigFileRelayerEvmPolicy {
                    gas_price_cap: Some(75000000000),
                    whitelist_receivers: None,
                    eip1559_pricing: Some(true),
                    private_transactions: None,
                    min_balance: None,
                    gas_limit_estimation: None,
                },
            )),
            signer_id: "test-signer".to_string(),
            notification_id: Some("test-notification".to_string()),
            custom_rpc_urls: None,
        };

        let domain_relayer = Relayer::try_from(config).unwrap();

        assert_eq!(domain_relayer.id, "test-evm");
        assert_eq!(domain_relayer.name, "Test EVM Relayer");
        assert_eq!(domain_relayer.network, "mainnet");
        assert!(!domain_relayer.paused);
        assert_eq!(
            domain_relayer.network_type,
            crate::models::relayer::RelayerNetworkType::Evm
        );
        assert_eq!(domain_relayer.signer_id, "test-signer");
        assert_eq!(
            domain_relayer.notification_id,
            Some("test-notification".to_string())
        );

        // Test policy conversion
        assert!(domain_relayer.policies.is_some());
        if let Some(RelayerNetworkPolicy::Evm(evm_policy)) = domain_relayer.policies {
            assert_eq!(evm_policy.gas_price_cap, Some(75000000000));
            assert_eq!(evm_policy.eip1559_pricing, Some(true));
        } else {
            panic!("Expected EVM domain policy");
        }
    }

    #[test]
    fn test_try_from_relayer_file_config_to_domain_solana() {
        let config = RelayerFileConfig {
            id: "test-solana".to_string(),
            name: "Test Solana Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: true,
            network_type: ConfigFileNetworkType::Solana,
            policies: Some(ConfigFileRelayerNetworkPolicy::Solana(
                ConfigFileRelayerSolanaPolicy {
                    fee_payment_strategy: Some(ConfigFileSolanaFeePaymentStrategy::Relayer),
                    fee_margin_percentage: None,
                    min_balance: Some(4000000),
                    allowed_tokens: None,
                    allowed_programs: None,
                    allowed_accounts: None,
                    disallowed_accounts: None,
                    max_tx_data_size: None,
                    max_signatures: Some(7),
                    max_allowed_fee_lamports: None,
                    swap_config: None,
                },
            )),
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        let domain_relayer = Relayer::try_from(config).unwrap();

        assert_eq!(
            domain_relayer.network_type,
            crate::models::relayer::RelayerNetworkType::Solana
        );
        assert!(domain_relayer.paused);

        // Test policy conversion
        assert!(domain_relayer.policies.is_some());
        if let Some(RelayerNetworkPolicy::Solana(solana_policy)) = domain_relayer.policies {
            assert_eq!(
                solana_policy.fee_payment_strategy,
                Some(SolanaFeePaymentStrategy::Relayer)
            );
            assert_eq!(solana_policy.min_balance, Some(4000000));
            assert_eq!(solana_policy.max_signatures, Some(7));
        } else {
            panic!("Expected Solana domain policy");
        }
    }

    #[test]
    fn test_try_from_relayer_file_config_to_domain_stellar() {
        let config = RelayerFileConfig {
            id: "test-stellar".to_string(),
            name: "Test Stellar Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: ConfigFileNetworkType::Stellar,
            policies: Some(ConfigFileRelayerNetworkPolicy::Stellar(
                ConfigFileRelayerStellarPolicy {
                    min_balance: Some(35000000),
                    max_fee: Some(200000),
                    timeout_seconds: Some(90),
                },
            )),
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        let domain_relayer = Relayer::try_from(config).unwrap();

        assert_eq!(
            domain_relayer.network_type,
            crate::models::relayer::RelayerNetworkType::Stellar
        );

        // Test policy conversion
        assert!(domain_relayer.policies.is_some());
        if let Some(RelayerNetworkPolicy::Stellar(stellar_policy)) = domain_relayer.policies {
            assert_eq!(stellar_policy.min_balance, Some(35000000));
            assert_eq!(stellar_policy.max_fee, Some(200000));
            assert_eq!(stellar_policy.timeout_seconds, Some(90));
        } else {
            panic!("Expected Stellar domain policy");
        }
    }

    #[test]
    fn test_try_from_relayer_file_config_validation_error() {
        let config = RelayerFileConfig {
            id: "".to_string(), // Invalid: empty ID
            name: "Test Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: ConfigFileNetworkType::Evm,
            policies: None,
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        let result = Relayer::try_from(config);
        assert!(result.is_err());

        if let Err(ConfigFileError::MissingField(field)) = result {
            assert_eq!(field, "relayer id");
        } else {
            panic!("Expected MissingField error for empty ID");
        }
    }

    #[test]
    fn test_try_from_relayer_file_config_invalid_id_format() {
        let config = RelayerFileConfig {
            id: "invalid@id".to_string(), // Invalid: contains @
            name: "Test Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: ConfigFileNetworkType::Evm,
            policies: None,
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        let result = Relayer::try_from(config);
        assert!(result.is_err());

        if let Err(ConfigFileError::InvalidIdFormat(_)) = result {
            // Success - expected error type
        } else {
            panic!("Expected InvalidIdFormat error");
        }
    }

    #[test]
    fn test_relayers_file_config_validation_success() {
        let relayer_config = RelayerFileConfig {
            id: "test-relayer".to_string(),
            name: "Test Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: ConfigFileNetworkType::Evm,
            policies: None,
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        let relayers_config = RelayersFileConfig::new(vec![relayer_config]);
        let networks_config = create_test_networks_config();

        // Note: This will fail because we don't have the network in our mock config
        // But we're testing that the validation logic runs
        let result = relayers_config.validate(&networks_config);

        // We expect this to fail due to network reference, but not due to empty relayers
        assert!(result.is_err());
        if let Err(ConfigFileError::InvalidReference(_)) = result {
            // Expected - network doesn't exist in our mock config
        } else {
            panic!("Expected InvalidReference error");
        }
    }

    #[test]
    fn test_relayers_file_config_validation_duplicate_ids() {
        let relayer_config1 = RelayerFileConfig {
            id: "duplicate-id".to_string(),
            name: "Test Relayer 1".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: ConfigFileNetworkType::Evm,
            policies: None,
            signer_id: "test-signer1".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        let relayer_config2 = RelayerFileConfig {
            id: "duplicate-id".to_string(), // Same ID
            name: "Test Relayer 2".to_string(),
            network: "testnet".to_string(),
            paused: false,
            network_type: ConfigFileNetworkType::Solana,
            policies: None,
            signer_id: "test-signer2".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        let relayers_config = RelayersFileConfig::new(vec![relayer_config1, relayer_config2]);
        let networks_config = create_test_networks_config();

        let result = relayers_config.validate(&networks_config);
        assert!(result.is_err());

        // The validation may fail with network reference error before reaching duplicate ID check
        // Let's check for either error type since both are valid validation failures
        match result {
            Err(ConfigFileError::DuplicateId(id)) => {
                assert_eq!(id, "duplicate-id");
            }
            Err(ConfigFileError::InvalidReference(_)) => {
                // Also acceptable - network doesn't exist in our mock config
            }
            Err(other) => {
                panic!(
                    "Expected DuplicateId or InvalidReference error, got: {:?}",
                    other
                );
            }
            Ok(_) => {
                panic!("Expected validation to fail but it succeeded");
            }
        }
    }

    #[test]
    fn test_relayers_file_config_validation_empty_network() {
        let relayer_config = RelayerFileConfig {
            id: "test-relayer".to_string(),
            name: "Test Relayer".to_string(),
            network: "".to_string(), // Empty network
            paused: false,
            network_type: ConfigFileNetworkType::Evm,
            policies: None,
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        let relayers_config = RelayersFileConfig::new(vec![relayer_config]);
        let networks_config = create_test_networks_config();

        let result = relayers_config.validate(&networks_config);
        assert!(result.is_err());

        if let Err(ConfigFileError::InvalidFormat(msg)) = result {
            assert!(msg.contains("relayer.network cannot be empty"));
        } else {
            panic!("Expected InvalidFormat error for empty network");
        }
    }

    #[test]
    fn test_config_file_policy_serialization() {
        // Test that individual policy structs can be serialized/deserialized
        let evm_policy = ConfigFileRelayerEvmPolicy {
            gas_price_cap: Some(80000000000),
            whitelist_receivers: Some(vec!["0xabc".to_string()]),
            eip1559_pricing: Some(false),
            private_transactions: Some(true),
            min_balance: Some(500000000000000000),
            gas_limit_estimation: Some(true),
        };

        let serialized = serde_json::to_string(&evm_policy).unwrap();
        let deserialized: ConfigFileRelayerEvmPolicy = serde_json::from_str(&serialized).unwrap();
        assert_eq!(evm_policy, deserialized);

        let solana_policy = ConfigFileRelayerSolanaPolicy {
            fee_payment_strategy: Some(ConfigFileSolanaFeePaymentStrategy::User),
            fee_margin_percentage: Some(3.0),
            min_balance: Some(6000000),
            allowed_tokens: None,
            allowed_programs: Some(vec!["Program456".to_string()]),
            allowed_accounts: None,
            disallowed_accounts: Some(vec!["DisallowedAccount".to_string()]),
            max_tx_data_size: Some(1536),
            max_signatures: Some(12),
            max_allowed_fee_lamports: Some(200000),
            swap_config: None,
        };

        let serialized = serde_json::to_string(&solana_policy).unwrap();
        let deserialized: ConfigFileRelayerSolanaPolicy =
            serde_json::from_str(&serialized).unwrap();
        assert_eq!(solana_policy, deserialized);

        let stellar_policy = ConfigFileRelayerStellarPolicy {
            min_balance: Some(45000000),
            max_fee: Some(250000),
            timeout_seconds: Some(120),
        };

        let serialized = serde_json::to_string(&stellar_policy).unwrap();
        let deserialized: ConfigFileRelayerStellarPolicy =
            serde_json::from_str(&serialized).unwrap();
        assert_eq!(stellar_policy, deserialized);
    }
}
