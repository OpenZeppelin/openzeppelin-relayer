//! Relayer domain model and business logic.
//!
//! This module provides the central `Relayer` type that represents relayers
//! throughout the relayer system, including:
//!
//! - **Domain Model**: Core `Relayer` struct with validation and configuration
//! - **Business Logic**: Update operations and validation rules  
//! - **Error Handling**: Comprehensive validation error types
//! - **Interoperability**: Conversions between API, config, and repository representations
//!
//! The relayer model supports multiple network types (EVM, Solana, Stellar) with
//! network-specific policies and configurations.

mod config;
pub use config::*;

pub mod request;
pub use request::*;

mod response;
pub use response::*;

pub mod repository;
pub use repository::*;

mod rpc_config;
pub use rpc_config::*;

use crate::{
    config::ConfigFileNetworkType,
    constants::ID_REGEX,
    utils::{deserialize_optional_u128, serialize_optional_u128},
};
use apalis_cron::Schedule;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use utoipa::ToSchema;
use validator::Validate;

/// Network type enum for relayers
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RelayerNetworkType {
    Evm,
    Solana,
    Stellar,
}

impl std::fmt::Display for RelayerNetworkType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelayerNetworkType::Evm => write!(f, "evm"),
            RelayerNetworkType::Solana => write!(f, "solana"),
            RelayerNetworkType::Stellar => write!(f, "stellar"),
        }
    }
}

impl From<ConfigFileNetworkType> for RelayerNetworkType {
    fn from(config_type: ConfigFileNetworkType) -> Self {
        match config_type {
            ConfigFileNetworkType::Evm => RelayerNetworkType::Evm,
            ConfigFileNetworkType::Solana => RelayerNetworkType::Solana,
            ConfigFileNetworkType::Stellar => RelayerNetworkType::Stellar,
        }
    }
}

impl From<RelayerNetworkType> for ConfigFileNetworkType {
    fn from(domain_type: RelayerNetworkType) -> Self {
        match domain_type {
            RelayerNetworkType::Evm => ConfigFileNetworkType::Evm,
            RelayerNetworkType::Solana => ConfigFileNetworkType::Solana,
            RelayerNetworkType::Stellar => ConfigFileNetworkType::Stellar,
        }
    }
}

/// EVM-specific relayer policy configuration
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct RelayerEvmPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(
        serialize_with = "serialize_optional_u128",
        deserialize_with = "deserialize_optional_u128",
        default
    )]
    pub min_balance: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gas_limit_estimation: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(
        serialize_with = "serialize_optional_u128",
        deserialize_with = "deserialize_optional_u128",
        default
    )]
    pub gas_price_cap: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whitelist_receivers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eip1559_pricing: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_transactions: Option<bool>,
}

/// Solana token swap configuration
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct SolanaAllowedTokensSwapConfig {
    /// Conversion slippage percentage for token. Optional.
    #[schema(nullable = false)]
    pub slippage_percentage: Option<f32>,
    /// Minimum amount of tokens to swap. Optional.
    #[schema(nullable = false)]
    pub min_amount: Option<u64>,
    /// Maximum amount of tokens to swap. Optional.
    #[schema(nullable = false)]
    pub max_amount: Option<u64>,
    /// Minimum amount of tokens to retain after swap. Optional.
    #[schema(nullable = false)]
    pub retain_min_amount: Option<u64>,
}

/// Configuration for allowed token handling on Solana
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SolanaAllowedTokensPolicy {
    pub mint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub decimals: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub max_allowed_fee: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub swap_config: Option<SolanaAllowedTokensSwapConfig>,
}

impl SolanaAllowedTokensPolicy {
    /// Create a new AllowedToken with required parameters
    pub fn new(
        mint: String,
        max_allowed_fee: Option<u64>,
        swap_config: Option<SolanaAllowedTokensSwapConfig>,
    ) -> Self {
        Self {
            mint,
            decimals: None,
            symbol: None,
            max_allowed_fee,
            swap_config,
        }
    }

    /// Create a new partial AllowedToken (alias for `new` for backward compatibility)
    pub fn new_partial(
        mint: String,
        max_allowed_fee: Option<u64>,
        swap_config: Option<SolanaAllowedTokensSwapConfig>,
    ) -> Self {
        Self::new(mint, max_allowed_fee, swap_config)
    }
}

/// Solana fee payment strategy
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum SolanaFeePaymentStrategy {
    #[default]
    User,
    Relayer,
}

/// Solana swap strategy
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SolanaSwapStrategy {
    JupiterSwap,
    JupiterUltra,
    #[default]
    Noop,
}

/// Jupiter swap options
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct JupiterSwapOptions {
    /// Maximum priority fee (in lamports) for a transaction. Optional.
    #[schema(nullable = false)]
    pub priority_fee_max_lamports: Option<u64>,
    /// Priority. Optional.
    #[schema(nullable = false)]
    pub priority_level: Option<String>,
    #[schema(nullable = false)]
    pub dynamic_compute_unit_limit: Option<bool>,
}

/// Solana swap policy configuration
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct RelayerSolanaSwapConfig {
    /// DEX strategy to use for token swaps.
    #[schema(nullable = false)]
    pub strategy: Option<SolanaSwapStrategy>,
    /// Cron schedule for executing token swap logic to keep relayer funded. Optional.
    #[schema(nullable = false)]
    pub cron_schedule: Option<String>,
    /// Min sol balance to execute token swap logic to keep relayer funded. Optional.
    #[schema(nullable = false)]
    pub min_balance_threshold: Option<u64>,
    /// Swap options for JupiterSwap strategy. Optional.
    #[schema(nullable = false)]
    pub jupiter_swap_options: Option<JupiterSwapOptions>,
}

/// Solana-specific relayer policy configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct RelayerSolanaPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_programs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_signatures: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tx_data_size: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_balance: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_tokens: Option<Vec<SolanaAllowedTokensPolicy>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_payment_strategy: Option<SolanaFeePaymentStrategy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_margin_percentage: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_accounts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disallowed_accounts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_allowed_fee_lamports: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swap_config: Option<RelayerSolanaSwapConfig>,
}

impl RelayerSolanaPolicy {
    /// Get allowed tokens for this policy
    pub fn get_allowed_tokens(&self) -> Vec<SolanaAllowedTokensPolicy> {
        self.allowed_tokens.clone().unwrap_or_default()
    }

    /// Get allowed token entry by mint address
    pub fn get_allowed_token_entry(&self, mint: &str) -> Option<SolanaAllowedTokensPolicy> {
        self.allowed_tokens
            .clone()
            .unwrap_or_default()
            .into_iter()
            .find(|entry| entry.mint == mint)
    }

    /// Get swap configuration for this policy
    pub fn get_swap_config(&self) -> Option<RelayerSolanaSwapConfig> {
        self.swap_config.clone()
    }

    /// Get allowed token decimals by mint address
    pub fn get_allowed_token_decimals(&self, mint: &str) -> Option<u8> {
        self.get_allowed_token_entry(mint)
            .and_then(|entry| entry.decimals)
    }
}
/// Stellar-specific relayer policy configuration
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct RelayerStellarPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_balance: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_fee: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
}

/// Network-specific policy for relayers
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(tag = "network_type")]
pub enum RelayerNetworkPolicy {
    #[serde(rename = "evm")]
    Evm(RelayerEvmPolicy),
    #[serde(rename = "solana")]
    Solana(RelayerSolanaPolicy),
    #[serde(rename = "stellar")]
    Stellar(RelayerStellarPolicy),
}

impl RelayerNetworkPolicy {
    /// Get EVM policy, returning default if not EVM
    pub fn get_evm_policy(&self) -> RelayerEvmPolicy {
        match self {
            Self::Evm(policy) => policy.clone(),
            _ => RelayerEvmPolicy::default(),
        }
    }

    /// Get Solana policy, returning default if not Solana
    pub fn get_solana_policy(&self) -> RelayerSolanaPolicy {
        match self {
            Self::Solana(policy) => policy.clone(),
            _ => RelayerSolanaPolicy::default(),
        }
    }

    /// Get Stellar policy, returning default if not Stellar
    pub fn get_stellar_policy(&self) -> RelayerStellarPolicy {
        match self {
            Self::Stellar(policy) => policy.clone(),
            _ => RelayerStellarPolicy::default(),
        }
    }
}

/// Core relayer domain model
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct Relayer {
    #[validate(
        length(min = 1, max = 36, message = "ID must be between 1 and 36 characters"),
        regex(
            path = "*ID_REGEX",
            message = "ID must contain only letters, numbers, dashes and underscores"
        )
    )]
    pub id: String,

    #[validate(length(min = 1, message = "Name cannot be empty"))]
    pub name: String,

    #[validate(length(min = 1, message = "Network cannot be empty"))]
    pub network: String,

    pub paused: bool,
    pub network_type: RelayerNetworkType,
    pub policies: Option<RelayerNetworkPolicy>,

    #[validate(length(min = 1, message = "Signer ID cannot be empty"))]
    pub signer_id: String,

    pub notification_id: Option<String>,
    pub custom_rpc_urls: Option<Vec<RpcConfig>>,
}

impl Relayer {
    /// Creates a new relayer
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        name: String,
        network: String,
        paused: bool,
        network_type: RelayerNetworkType,
        policies: Option<RelayerNetworkPolicy>,
        signer_id: String,
        notification_id: Option<String>,
        custom_rpc_urls: Option<Vec<RpcConfig>>,
    ) -> Self {
        Self {
            id,
            name,
            network,
            paused,
            network_type,
            policies,
            signer_id,
            notification_id,
            custom_rpc_urls,
        }
    }

    /// Validates the relayer using both validator crate and custom validation
    pub fn validate(&self) -> Result<(), RelayerValidationError> {
        // Check for empty ID specifically first
        if self.id.is_empty() {
            return Err(RelayerValidationError::EmptyId);
        }

        // Check for ID too long
        if self.id.len() > 36 {
            return Err(RelayerValidationError::IdTooLong);
        }

        // First run validator crate validation
        Validate::validate(self).map_err(|validation_errors| {
            // Convert validator errors to our custom error type
            for (field, errors) in validation_errors.field_errors() {
                if let Some(error) = errors.first() {
                    let field_str = field.as_ref();
                    return match (field_str, error.code.as_ref()) {
                        ("id", "regex") => RelayerValidationError::InvalidIdFormat,
                        ("name", "length") => RelayerValidationError::EmptyName,
                        ("network", "length") => RelayerValidationError::EmptyNetwork,
                        ("signer_id", "length") => RelayerValidationError::InvalidPolicy(
                            "Signer ID cannot be empty".to_string(),
                        ),
                        _ => RelayerValidationError::InvalidIdFormat, // fallback
                    };
                }
            }
            // Fallback error
            RelayerValidationError::InvalidIdFormat
        })?;

        // Run custom validation
        self.validate_policies()?;
        self.validate_custom_rpc_urls()?;

        Ok(())
    }

    /// Validates network-specific policies
    fn validate_policies(&self) -> Result<(), RelayerValidationError> {
        match (&self.network_type, &self.policies) {
            (RelayerNetworkType::Solana, Some(RelayerNetworkPolicy::Solana(policy))) => {
                self.validate_solana_policy(policy)?;
            }
            (RelayerNetworkType::Evm, Some(RelayerNetworkPolicy::Evm(_))) => {
                // EVM policies don't need special validation currently
            }
            (RelayerNetworkType::Stellar, Some(RelayerNetworkPolicy::Stellar(_))) => {
                // Stellar policies don't need special validation currently
            }
            // Mismatched network type and policy type
            (network_type, Some(policy)) => {
                let policy_type = match policy {
                    RelayerNetworkPolicy::Evm(_) => "EVM",
                    RelayerNetworkPolicy::Solana(_) => "Solana",
                    RelayerNetworkPolicy::Stellar(_) => "Stellar",
                };
                let network_type_str = format!("{:?}", network_type);
                return Err(RelayerValidationError::InvalidPolicy(format!(
                    "Network type {} does not match policy type {}",
                    network_type_str, policy_type
                )));
            }
            // No policies is fine
            (_, None) => {}
        }
        Ok(())
    }

    /// Validates Solana-specific policies
    fn validate_solana_policy(
        &self,
        policy: &RelayerSolanaPolicy,
    ) -> Result<(), RelayerValidationError> {
        // Validate public keys
        self.validate_solana_pub_keys(&policy.allowed_accounts)?;
        self.validate_solana_pub_keys(&policy.disallowed_accounts)?;
        self.validate_solana_pub_keys(&policy.allowed_programs)?;

        // Validate allowed tokens mint addresses
        if let Some(tokens) = &policy.allowed_tokens {
            let mint_keys: Vec<String> = tokens.iter().map(|t| t.mint.clone()).collect();
            self.validate_solana_pub_keys(&Some(mint_keys))?;
        }

        // Validate fee margin percentage
        if let Some(fee_margin) = policy.fee_margin_percentage {
            if fee_margin < 0.0 {
                return Err(RelayerValidationError::InvalidPolicy(
                    "Negative fee margin percentage values are not accepted".into(),
                ));
            }
        }

        // Check for conflicting allowed/disallowed accounts
        if policy.allowed_accounts.is_some() && policy.disallowed_accounts.is_some() {
            return Err(RelayerValidationError::InvalidPolicy(
                "allowed_accounts and disallowed_accounts cannot be both present".into(),
            ));
        }

        // Validate swap configuration
        if let Some(swap_config) = &policy.swap_config {
            self.validate_solana_swap_config(swap_config, policy)?;
        }

        Ok(())
    }

    /// Validates Solana public key format
    fn validate_solana_pub_keys(
        &self,
        keys: &Option<Vec<String>>,
    ) -> Result<(), RelayerValidationError> {
        if let Some(keys) = keys {
            let solana_pub_key_regex =
                Regex::new(r"^[1-9A-HJ-NP-Za-km-z]{32,44}$").map_err(|e| {
                    RelayerValidationError::InvalidPolicy(format!("Regex compilation error: {}", e))
                })?;

            for key in keys {
                if !solana_pub_key_regex.is_match(key) {
                    return Err(RelayerValidationError::InvalidPolicy(
                        "Public key must be a valid Solana address".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Validates Solana swap configuration
    fn validate_solana_swap_config(
        &self,
        swap_config: &RelayerSolanaSwapConfig,
        policy: &RelayerSolanaPolicy,
    ) -> Result<(), RelayerValidationError> {
        // Swap config only supported for user fee payment strategy
        if let Some(fee_payment_strategy) = &policy.fee_payment_strategy {
            if *fee_payment_strategy == SolanaFeePaymentStrategy::Relayer {
                return Err(RelayerValidationError::InvalidPolicy(
                    "Swap config only supported for user fee payment strategy".into(),
                ));
            }
        }

        // Validate strategy-specific restrictions
        if let Some(strategy) = &swap_config.strategy {
            match strategy {
                SolanaSwapStrategy::JupiterSwap | SolanaSwapStrategy::JupiterUltra => {
                    if self.network != "mainnet-beta" {
                        return Err(RelayerValidationError::InvalidPolicy(format!(
                            "{:?} strategy is only supported on mainnet-beta",
                            strategy
                        )));
                    }
                }
                SolanaSwapStrategy::Noop => {
                    // No-op strategy doesn't need validation
                }
            }
        }

        // Validate cron schedule
        if let Some(cron_schedule) = &swap_config.cron_schedule {
            if cron_schedule.is_empty() {
                return Err(RelayerValidationError::InvalidPolicy(
                    "Empty cron schedule is not accepted".into(),
                ));
            }

            Schedule::from_str(cron_schedule).map_err(|_| {
                RelayerValidationError::InvalidPolicy("Invalid cron schedule format".into())
            })?;
        }

        // Validate Jupiter swap options
        if let Some(jupiter_options) = &swap_config.jupiter_swap_options {
            // Jupiter options only valid for JupiterSwap strategy
            if swap_config.strategy != Some(SolanaSwapStrategy::JupiterSwap) {
                return Err(RelayerValidationError::InvalidPolicy(
                    "JupiterSwap options are only valid for JupiterSwap strategy".into(),
                ));
            }

            if let Some(max_lamports) = jupiter_options.priority_fee_max_lamports {
                if max_lamports == 0 {
                    return Err(RelayerValidationError::InvalidPolicy(
                        "Max lamports must be greater than 0".into(),
                    ));
                }
            }

            if let Some(priority_level) = &jupiter_options.priority_level {
                if priority_level.is_empty() {
                    return Err(RelayerValidationError::InvalidPolicy(
                        "Priority level cannot be empty".into(),
                    ));
                }

                let valid_levels = ["medium", "high", "veryHigh"];
                if !valid_levels.contains(&priority_level.as_str()) {
                    return Err(RelayerValidationError::InvalidPolicy(
                        "Priority level must be one of: medium, high, veryHigh".into(),
                    ));
                }
            }

            // Priority level and max lamports must be used together
            match (
                &jupiter_options.priority_level,
                jupiter_options.priority_fee_max_lamports,
            ) {
                (Some(_), None) => {
                    return Err(RelayerValidationError::InvalidPolicy(
                        "Priority Fee Max lamports must be set if priority level is set".into(),
                    ));
                }
                (None, Some(_)) => {
                    return Err(RelayerValidationError::InvalidPolicy(
                        "Priority level must be set if priority fee max lamports is set".into(),
                    ));
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Validates custom RPC URL configurations
    fn validate_custom_rpc_urls(&self) -> Result<(), RelayerValidationError> {
        if let Some(configs) = &self.custom_rpc_urls {
            for config in configs {
                reqwest::Url::parse(&config.url)
                    .map_err(|_| RelayerValidationError::InvalidRpcUrl(config.url.clone()))?;

                if config.weight > 100 {
                    return Err(RelayerValidationError::InvalidRpcWeight);
                }
            }
        }
        Ok(())
    }

    /// Apply JSON Merge Patch (RFC 7396) directly to the domain object
    ///
    /// This method:
    /// 1. Converts domain object to JSON
    /// 2. Applies JSON merge patch
    /// 3. Converts back to domain object
    /// 4. Validates the final result
    ///
    /// This approach provides true JSON Merge Patch semantics while maintaining validation.
    pub fn apply_json_patch(
        &self,
        patch: &serde_json::Value,
    ) -> Result<Self, RelayerValidationError> {
        // 1. Convert current domain object to JSON
        let mut domain_json = serde_json::to_value(self).map_err(|e| {
            RelayerValidationError::InvalidField(format!("Serialization error: {}", e))
        })?;

        // 2. Apply JSON Merge Patch
        json_patch::merge(&mut domain_json, patch);

        // 3. Convert back to domain object
        let updated: Relayer = serde_json::from_value(domain_json).map_err(|e| {
            RelayerValidationError::InvalidField(format!("Invalid result after patch: {}", e))
        })?;

        // 4. Validate the final result
        updated.validate()?;

        Ok(updated)
    }
}

/// Validation errors for relayers
#[derive(Debug, thiserror::Error)]
pub enum RelayerValidationError {
    #[error("Relayer ID cannot be empty")]
    EmptyId,
    #[error("Relayer ID must contain only letters, numbers, dashes and underscores and must be at most 36 characters long")]
    InvalidIdFormat,
    #[error("Relayer ID must not exceed 36 characters")]
    IdTooLong,
    #[error("Relayer name cannot be empty")]
    EmptyName,
    #[error("Network cannot be empty")]
    EmptyNetwork,
    #[error("Invalid relayer policy: {0}")]
    InvalidPolicy(String),
    #[error("Invalid RPC URL: {0}")]
    InvalidRpcUrl(String),
    #[error("RPC URL weight must be in range 0-100")]
    InvalidRpcWeight,
    #[error("Invalid field: {0}")]
    InvalidField(String),
}

/// Centralized conversion from RelayerValidationError to ApiError
impl From<RelayerValidationError> for crate::models::ApiError {
    fn from(error: RelayerValidationError) -> Self {
        use crate::models::ApiError;

        ApiError::BadRequest(match error {
            RelayerValidationError::EmptyId => "ID cannot be empty".to_string(),
            RelayerValidationError::InvalidIdFormat => {
                "ID must contain only letters, numbers, dashes and underscores and must be at most 36 characters long".to_string()
            }
            RelayerValidationError::IdTooLong => {
                "ID must not exceed 36 characters".to_string()
            }
            RelayerValidationError::EmptyName => "Name cannot be empty".to_string(),
            RelayerValidationError::EmptyNetwork => "Network cannot be empty".to_string(),
            RelayerValidationError::InvalidPolicy(msg) => {
                format!("Invalid relayer policy: {}", msg)
            }
            RelayerValidationError::InvalidRpcUrl(url) => {
                format!("Invalid RPC URL: {}", url)
            }
            RelayerValidationError::InvalidRpcWeight => {
                "RPC URL weight must be in range 0-100".to_string()
            }
            RelayerValidationError::InvalidField(msg) => msg.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ===== RelayerNetworkType Tests =====

    #[test]
    fn test_relayer_network_type_display() {
        assert_eq!(RelayerNetworkType::Evm.to_string(), "evm");
        assert_eq!(RelayerNetworkType::Solana.to_string(), "solana");
        assert_eq!(RelayerNetworkType::Stellar.to_string(), "stellar");
    }

    #[test]
    fn test_relayer_network_type_from_config_file_type() {
        assert_eq!(
            RelayerNetworkType::from(ConfigFileNetworkType::Evm),
            RelayerNetworkType::Evm
        );
        assert_eq!(
            RelayerNetworkType::from(ConfigFileNetworkType::Solana),
            RelayerNetworkType::Solana
        );
        assert_eq!(
            RelayerNetworkType::from(ConfigFileNetworkType::Stellar),
            RelayerNetworkType::Stellar
        );
    }

    #[test]
    fn test_config_file_network_type_from_relayer_type() {
        assert_eq!(
            ConfigFileNetworkType::from(RelayerNetworkType::Evm),
            ConfigFileNetworkType::Evm
        );
        assert_eq!(
            ConfigFileNetworkType::from(RelayerNetworkType::Solana),
            ConfigFileNetworkType::Solana
        );
        assert_eq!(
            ConfigFileNetworkType::from(RelayerNetworkType::Stellar),
            ConfigFileNetworkType::Stellar
        );
    }

    #[test]
    fn test_relayer_network_type_serialization() {
        let evm_type = RelayerNetworkType::Evm;
        let serialized = serde_json::to_string(&evm_type).unwrap();
        assert_eq!(serialized, "\"evm\"");

        let deserialized: RelayerNetworkType = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, RelayerNetworkType::Evm);

        // Test all types
        let types = vec![
            (RelayerNetworkType::Evm, "\"evm\""),
            (RelayerNetworkType::Solana, "\"solana\""),
            (RelayerNetworkType::Stellar, "\"stellar\""),
        ];

        for (network_type, expected_json) in types {
            let serialized = serde_json::to_string(&network_type).unwrap();
            assert_eq!(serialized, expected_json);

            let deserialized: RelayerNetworkType = serde_json::from_str(&serialized).unwrap();
            assert_eq!(deserialized, network_type);
        }
    }

    // ===== Policy Struct Tests =====

    #[test]
    fn test_relayer_evm_policy_default() {
        let default_policy = RelayerEvmPolicy::default();
        assert_eq!(default_policy.min_balance, None);
        assert_eq!(default_policy.gas_limit_estimation, None);
        assert_eq!(default_policy.gas_price_cap, None);
        assert_eq!(default_policy.whitelist_receivers, None);
        assert_eq!(default_policy.eip1559_pricing, None);
        assert_eq!(default_policy.private_transactions, None);
    }

    #[test]
    fn test_relayer_evm_policy_serialization() {
        let policy = RelayerEvmPolicy {
            min_balance: Some(1000000000000000000),
            gas_limit_estimation: Some(true),
            gas_price_cap: Some(50000000000),
            whitelist_receivers: Some(vec!["0x123".to_string(), "0x456".to_string()]),
            eip1559_pricing: Some(false),
            private_transactions: Some(true),
        };

        let serialized = serde_json::to_string(&policy).unwrap();
        let deserialized: RelayerEvmPolicy = serde_json::from_str(&serialized).unwrap();
        assert_eq!(policy, deserialized);
    }

    #[test]
    fn test_allowed_token_new() {
        let token = SolanaAllowedTokensPolicy::new(
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            Some(100000),
            None,
        );

        assert_eq!(token.mint, "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
        assert_eq!(token.max_allowed_fee, Some(100000));
        assert_eq!(token.decimals, None);
        assert_eq!(token.symbol, None);
        assert_eq!(token.swap_config, None);
    }

    #[test]
    fn test_allowed_token_new_partial() {
        let swap_config = SolanaAllowedTokensSwapConfig {
            slippage_percentage: Some(0.5),
            min_amount: Some(1000),
            max_amount: Some(10000000),
            retain_min_amount: Some(500),
        };

        let token = SolanaAllowedTokensPolicy::new_partial(
            "TokenMint123".to_string(),
            Some(50000),
            Some(swap_config.clone()),
        );

        assert_eq!(token.mint, "TokenMint123");
        assert_eq!(token.max_allowed_fee, Some(50000));
        assert_eq!(token.swap_config, Some(swap_config));
    }

    #[test]
    fn test_allowed_token_swap_config_default() {
        let config = AllowedTokenSwapConfig::default();
        assert_eq!(config.slippage_percentage, None);
        assert_eq!(config.min_amount, None);
        assert_eq!(config.max_amount, None);
        assert_eq!(config.retain_min_amount, None);
    }

    #[test]
    fn test_relayer_solana_fee_payment_strategy_default() {
        let default_strategy = SolanaFeePaymentStrategy::default();
        assert_eq!(default_strategy, SolanaFeePaymentStrategy::User);
    }

    #[test]
    fn test_relayer_solana_swap_strategy_default() {
        let default_strategy = SolanaSwapStrategy::default();
        assert_eq!(default_strategy, SolanaSwapStrategy::Noop);
    }

    #[test]
    fn test_jupiter_swap_options_default() {
        let options = JupiterSwapOptions::default();
        assert_eq!(options.priority_fee_max_lamports, None);
        assert_eq!(options.priority_level, None);
        assert_eq!(options.dynamic_compute_unit_limit, None);
    }

    #[test]
    fn test_relayer_solana_swap_policy_default() {
        let policy = RelayerSolanaSwapConfig::default();
        assert_eq!(policy.strategy, None);
        assert_eq!(policy.cron_schedule, None);
        assert_eq!(policy.min_balance_threshold, None);
        assert_eq!(policy.jupiter_swap_options, None);
    }

    #[test]
    fn test_relayer_solana_policy_default() {
        let policy = RelayerSolanaPolicy::default();
        assert_eq!(policy.allowed_programs, None);
        assert_eq!(policy.max_signatures, None);
        assert_eq!(policy.max_tx_data_size, None);
        assert_eq!(policy.min_balance, None);
        assert_eq!(policy.allowed_tokens, None);
        assert_eq!(policy.fee_payment_strategy, None);
        assert_eq!(policy.fee_margin_percentage, None);
        assert_eq!(policy.allowed_accounts, None);
        assert_eq!(policy.disallowed_accounts, None);
        assert_eq!(policy.max_allowed_fee_lamports, None);
        assert_eq!(policy.swap_config, None);
    }

    #[test]
    fn test_relayer_solana_policy_get_allowed_tokens() {
        let token1 = SolanaAllowedTokensPolicy::new("mint1".to_string(), Some(1000), None);
        let token2 = SolanaAllowedTokensPolicy::new("mint2".to_string(), Some(2000), None);

        let policy = RelayerSolanaPolicy {
            allowed_tokens: Some(vec![token1.clone(), token2.clone()]),
            ..RelayerSolanaPolicy::default()
        };

        let tokens = policy.get_allowed_tokens();
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0], token1);
        assert_eq!(tokens[1], token2);

        // Test empty case
        let empty_policy = RelayerSolanaPolicy::default();
        let empty_tokens = empty_policy.get_allowed_tokens();
        assert_eq!(empty_tokens.len(), 0);
    }

    #[test]
    fn test_relayer_solana_policy_get_allowed_token_entry() {
        let token1 = SolanaAllowedTokensPolicy::new("mint1".to_string(), Some(1000), None);
        let token2 = SolanaAllowedTokensPolicy::new("mint2".to_string(), Some(2000), None);

        let policy = RelayerSolanaPolicy {
            allowed_tokens: Some(vec![token1.clone(), token2.clone()]),
            ..RelayerSolanaPolicy::default()
        };

        let found_token = policy.get_allowed_token_entry("mint1").unwrap();
        assert_eq!(found_token, token1);

        let not_found = policy.get_allowed_token_entry("mint3");
        assert!(not_found.is_none());

        // Test empty case
        let empty_policy = RelayerSolanaPolicy::default();
        let empty_result = empty_policy.get_allowed_token_entry("mint1");
        assert!(empty_result.is_none());
    }

    #[test]
    fn test_relayer_solana_policy_get_swap_config() {
        let swap_config = RelayerSolanaSwapConfig {
            strategy: Some(SolanaSwapStrategy::JupiterSwap),
            cron_schedule: Some("0 0 * * *".to_string()),
            min_balance_threshold: Some(1000000),
            jupiter_swap_options: None,
        };

        let policy = RelayerSolanaPolicy {
            swap_config: Some(swap_config.clone()),
            ..RelayerSolanaPolicy::default()
        };

        let retrieved_config = policy.get_swap_config().unwrap();
        assert_eq!(retrieved_config, swap_config);

        // Test None case
        let empty_policy = RelayerSolanaPolicy::default();
        assert!(empty_policy.get_swap_config().is_none());
    }

    #[test]
    fn test_relayer_solana_policy_get_allowed_token_decimals() {
        let mut token1 = SolanaAllowedTokensPolicy::new("mint1".to_string(), Some(1000), None);
        token1.decimals = Some(9);

        let token2 = SolanaAllowedTokensPolicy::new("mint2".to_string(), Some(2000), None);
        // token2.decimals is None

        let policy = RelayerSolanaPolicy {
            allowed_tokens: Some(vec![token1, token2]),
            ..RelayerSolanaPolicy::default()
        };

        assert_eq!(policy.get_allowed_token_decimals("mint1"), Some(9));
        assert_eq!(policy.get_allowed_token_decimals("mint2"), None);
        assert_eq!(policy.get_allowed_token_decimals("mint3"), None);
    }

    #[test]
    fn test_relayer_stellar_policy_default() {
        let policy = RelayerStellarPolicy::default();
        assert_eq!(policy.min_balance, None);
        assert_eq!(policy.max_fee, None);
        assert_eq!(policy.timeout_seconds, None);
    }

    // ===== RelayerNetworkPolicy Tests =====

    #[test]
    fn test_relayer_network_policy_get_evm_policy() {
        let evm_policy = RelayerEvmPolicy {
            gas_price_cap: Some(50000000000),
            ..RelayerEvmPolicy::default()
        };

        let network_policy = RelayerNetworkPolicy::Evm(evm_policy.clone());
        assert_eq!(network_policy.get_evm_policy(), evm_policy);

        // Test non-EVM policy returns default
        let solana_policy = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy::default());
        assert_eq!(solana_policy.get_evm_policy(), RelayerEvmPolicy::default());

        let stellar_policy = RelayerNetworkPolicy::Stellar(RelayerStellarPolicy::default());
        assert_eq!(stellar_policy.get_evm_policy(), RelayerEvmPolicy::default());
    }

    #[test]
    fn test_relayer_network_policy_get_solana_policy() {
        let solana_policy = RelayerSolanaPolicy {
            min_balance: Some(5000000),
            ..RelayerSolanaPolicy::default()
        };

        let network_policy = RelayerNetworkPolicy::Solana(solana_policy.clone());
        assert_eq!(network_policy.get_solana_policy(), solana_policy);

        // Test non-Solana policy returns default
        let evm_policy = RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default());
        assert_eq!(
            evm_policy.get_solana_policy(),
            RelayerSolanaPolicy::default()
        );

        let stellar_policy = RelayerNetworkPolicy::Stellar(RelayerStellarPolicy::default());
        assert_eq!(
            stellar_policy.get_solana_policy(),
            RelayerSolanaPolicy::default()
        );
    }

    #[test]
    fn test_relayer_network_policy_get_stellar_policy() {
        let stellar_policy = RelayerStellarPolicy {
            min_balance: Some(20000000),
            max_fee: Some(100000),
            timeout_seconds: Some(30),
        };

        let network_policy = RelayerNetworkPolicy::Stellar(stellar_policy.clone());
        assert_eq!(network_policy.get_stellar_policy(), stellar_policy);

        // Test non-Stellar policy returns default
        let evm_policy = RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default());
        assert_eq!(
            evm_policy.get_stellar_policy(),
            RelayerStellarPolicy::default()
        );

        let solana_policy = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy::default());
        assert_eq!(
            solana_policy.get_stellar_policy(),
            RelayerStellarPolicy::default()
        );
    }

    // ===== Relayer Construction and Basic Tests =====

    #[test]
    fn test_relayer_new() {
        let relayer = Relayer::new(
            "test-relayer".to_string(),
            "Test Relayer".to_string(),
            "mainnet".to_string(),
            false,
            RelayerNetworkType::Evm,
            Some(RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default())),
            "test-signer".to_string(),
            Some("test-notification".to_string()),
            None,
        );

        assert_eq!(relayer.id, "test-relayer");
        assert_eq!(relayer.name, "Test Relayer");
        assert_eq!(relayer.network, "mainnet");
        assert!(!relayer.paused);
        assert_eq!(relayer.network_type, RelayerNetworkType::Evm);
        assert_eq!(relayer.signer_id, "test-signer");
        assert_eq!(
            relayer.notification_id,
            Some("test-notification".to_string())
        );
        assert!(relayer.policies.is_some());
        assert_eq!(relayer.custom_rpc_urls, None);
    }

    // ===== Relayer Validation Tests =====

    #[test]
    fn test_relayer_validation_success() {
        let relayer = Relayer::new(
            "valid-relayer-id".to_string(),
            "Valid Relayer".to_string(),
            "mainnet".to_string(),
            false,
            RelayerNetworkType::Evm,
            None,
            "valid-signer".to_string(),
            None,
            None,
        );

        assert!(relayer.validate().is_ok());
    }

    #[test]
    fn test_relayer_validation_empty_id() {
        let relayer = Relayer::new(
            "".to_string(), // Empty ID
            "Valid Relayer".to_string(),
            "mainnet".to_string(),
            false,
            RelayerNetworkType::Evm,
            None,
            "valid-signer".to_string(),
            None,
            None,
        );

        let result = relayer.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RelayerValidationError::EmptyId
        ));
    }

    #[test]
    fn test_relayer_validation_id_too_long() {
        let long_id = "a".repeat(37); // 37 characters, exceeds 36 limit
        let relayer = Relayer::new(
            long_id,
            "Valid Relayer".to_string(),
            "mainnet".to_string(),
            false,
            RelayerNetworkType::Evm,
            None,
            "valid-signer".to_string(),
            None,
            None,
        );

        let result = relayer.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RelayerValidationError::IdTooLong
        ));
    }

    #[test]
    fn test_relayer_validation_invalid_id_format() {
        let relayer = Relayer::new(
            "invalid@id".to_string(), // Contains invalid character @
            "Valid Relayer".to_string(),
            "mainnet".to_string(),
            false,
            RelayerNetworkType::Evm,
            None,
            "valid-signer".to_string(),
            None,
            None,
        );

        let result = relayer.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RelayerValidationError::InvalidIdFormat
        ));
    }

    #[test]
    fn test_relayer_validation_empty_name() {
        let relayer = Relayer::new(
            "valid-id".to_string(),
            "".to_string(), // Empty name
            "mainnet".to_string(),
            false,
            RelayerNetworkType::Evm,
            None,
            "valid-signer".to_string(),
            None,
            None,
        );

        let result = relayer.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RelayerValidationError::EmptyName
        ));
    }

    #[test]
    fn test_relayer_validation_empty_network() {
        let relayer = Relayer::new(
            "valid-id".to_string(),
            "Valid Relayer".to_string(),
            "".to_string(), // Empty network
            false,
            RelayerNetworkType::Evm,
            None,
            "valid-signer".to_string(),
            None,
            None,
        );

        let result = relayer.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RelayerValidationError::EmptyNetwork
        ));
    }

    #[test]
    fn test_relayer_validation_empty_signer_id() {
        let relayer = Relayer::new(
            "valid-id".to_string(),
            "Valid Relayer".to_string(),
            "mainnet".to_string(),
            false,
            RelayerNetworkType::Evm,
            None,
            "".to_string(), // Empty signer ID
            None,
            None,
        );

        let result = relayer.validate();
        assert!(result.is_err());
        // This should trigger InvalidPolicy error due to empty signer ID
        if let Err(RelayerValidationError::InvalidPolicy(msg)) = result {
            assert!(msg.contains("Signer ID cannot be empty"));
        } else {
            panic!("Expected InvalidPolicy error for empty signer ID");
        }
    }

    #[test]
    fn test_relayer_validation_mismatched_network_type_and_policy() {
        let relayer = Relayer::new(
            "valid-id".to_string(),
            "Valid Relayer".to_string(),
            "mainnet".to_string(),
            false,
            RelayerNetworkType::Evm, // EVM network type
            Some(RelayerNetworkPolicy::Solana(RelayerSolanaPolicy::default())), // But Solana policy
            "valid-signer".to_string(),
            None,
            None,
        );

        let result = relayer.validate();
        assert!(result.is_err());
        if let Err(RelayerValidationError::InvalidPolicy(msg)) = result {
            assert!(msg.contains("Network type") && msg.contains("does not match policy type"));
        } else {
            panic!("Expected InvalidPolicy error for mismatched network type and policy");
        }
    }

    #[test]
    fn test_relayer_validation_invalid_rpc_url() {
        let relayer = Relayer::new(
            "valid-id".to_string(),
            "Valid Relayer".to_string(),
            "mainnet".to_string(),
            false,
            RelayerNetworkType::Evm,
            None,
            "valid-signer".to_string(),
            None,
            Some(vec![RpcConfig::new("invalid-url".to_string())]), // Invalid URL
        );

        let result = relayer.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RelayerValidationError::InvalidRpcUrl(_)
        ));
    }

    #[test]
    fn test_relayer_validation_invalid_rpc_weight() {
        let relayer = Relayer::new(
            "valid-id".to_string(),
            "Valid Relayer".to_string(),
            "mainnet".to_string(),
            false,
            RelayerNetworkType::Evm,
            None,
            "valid-signer".to_string(),
            None,
            Some(vec![RpcConfig {
                url: "https://example.com".to_string(),
                weight: 150,
            }]), // Weight > 100
        );

        let result = relayer.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RelayerValidationError::InvalidRpcWeight
        ));
    }

    // ===== Solana-specific Validation Tests =====

    #[test]
    fn test_relayer_validation_solana_invalid_public_key() {
        let policy = RelayerSolanaPolicy {
            allowed_programs: Some(vec!["invalid-pubkey".to_string()]), // Invalid Solana pubkey
            ..RelayerSolanaPolicy::default()
        };

        let relayer = Relayer::new(
            "valid-id".to_string(),
            "Valid Relayer".to_string(),
            "mainnet".to_string(),
            false,
            RelayerNetworkType::Solana,
            Some(RelayerNetworkPolicy::Solana(policy)),
            "valid-signer".to_string(),
            None,
            None,
        );

        let result = relayer.validate();
        assert!(result.is_err());
        if let Err(RelayerValidationError::InvalidPolicy(msg)) = result {
            assert!(msg.contains("Public key must be a valid Solana address"));
        } else {
            panic!("Expected InvalidPolicy error for invalid Solana public key");
        }
    }

    #[test]
    fn test_relayer_validation_solana_valid_public_key() {
        let policy = RelayerSolanaPolicy {
            allowed_programs: Some(vec!["11111111111111111111111111111111".to_string()]), // Valid Solana pubkey
            ..RelayerSolanaPolicy::default()
        };

        let relayer = Relayer::new(
            "valid-id".to_string(),
            "Valid Relayer".to_string(),
            "mainnet".to_string(),
            false,
            RelayerNetworkType::Solana,
            Some(RelayerNetworkPolicy::Solana(policy)),
            "valid-signer".to_string(),
            None,
            None,
        );

        assert!(relayer.validate().is_ok());
    }

    #[test]
    fn test_relayer_validation_solana_negative_fee_margin() {
        let policy = RelayerSolanaPolicy {
            fee_margin_percentage: Some(-1.0), // Negative fee margin
            ..RelayerSolanaPolicy::default()
        };

        let relayer = Relayer::new(
            "valid-id".to_string(),
            "Valid Relayer".to_string(),
            "mainnet".to_string(),
            false,
            RelayerNetworkType::Solana,
            Some(RelayerNetworkPolicy::Solana(policy)),
            "valid-signer".to_string(),
            None,
            None,
        );

        let result = relayer.validate();
        assert!(result.is_err());
        if let Err(RelayerValidationError::InvalidPolicy(msg)) = result {
            assert!(msg.contains("Negative fee margin percentage values are not accepted"));
        } else {
            panic!("Expected InvalidPolicy error for negative fee margin");
        }
    }

    #[test]
    fn test_relayer_validation_solana_conflicting_accounts() {
        let policy = RelayerSolanaPolicy {
            allowed_accounts: Some(vec!["11111111111111111111111111111111".to_string()]),
            disallowed_accounts: Some(vec!["22222222222222222222222222222222".to_string()]),
            ..RelayerSolanaPolicy::default()
        };

        let relayer = Relayer::new(
            "valid-id".to_string(),
            "Valid Relayer".to_string(),
            "mainnet".to_string(),
            false,
            RelayerNetworkType::Solana,
            Some(RelayerNetworkPolicy::Solana(policy)),
            "valid-signer".to_string(),
            None,
            None,
        );

        let result = relayer.validate();
        assert!(result.is_err());
        if let Err(RelayerValidationError::InvalidPolicy(msg)) = result {
            assert!(msg.contains("allowed_accounts and disallowed_accounts cannot be both present"));
        } else {
            panic!("Expected InvalidPolicy error for conflicting accounts");
        }
    }

    #[test]
    fn test_relayer_validation_solana_swap_config_wrong_fee_payment_strategy() {
        let swap_config = RelayerSolanaSwapConfig {
            strategy: Some(SolanaSwapStrategy::JupiterSwap),
            ..RelayerSolanaSwapConfig::default()
        };

        let policy = RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer), // Relayer strategy
            swap_config: Some(swap_config),                                // But has swap config
            ..RelayerSolanaPolicy::default()
        };

        let relayer = Relayer::new(
            "valid-id".to_string(),
            "Valid Relayer".to_string(),
            "mainnet".to_string(),
            false,
            RelayerNetworkType::Solana,
            Some(RelayerNetworkPolicy::Solana(policy)),
            "valid-signer".to_string(),
            None,
            None,
        );

        let result = relayer.validate();
        assert!(result.is_err());
        if let Err(RelayerValidationError::InvalidPolicy(msg)) = result {
            assert!(msg.contains("Swap config only supported for user fee payment strategy"));
        } else {
            panic!("Expected InvalidPolicy error for swap config with relayer fee payment");
        }
    }

    #[test]
    fn test_relayer_validation_solana_jupiter_strategy_wrong_network() {
        let swap_config = RelayerSolanaSwapConfig {
            strategy: Some(SolanaSwapStrategy::JupiterSwap),
            ..RelayerSolanaSwapConfig::default()
        };

        let policy = RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::User),
            swap_config: Some(swap_config),
            ..RelayerSolanaPolicy::default()
        };

        let relayer = Relayer::new(
            "valid-id".to_string(),
            "Valid Relayer".to_string(),
            "testnet".to_string(), // Not mainnet-beta
            false,
            RelayerNetworkType::Solana,
            Some(RelayerNetworkPolicy::Solana(policy)),
            "valid-signer".to_string(),
            None,
            None,
        );

        let result = relayer.validate();
        assert!(result.is_err());
        if let Err(RelayerValidationError::InvalidPolicy(msg)) = result {
            assert!(msg.contains("strategy is only supported on mainnet-beta"));
        } else {
            panic!("Expected InvalidPolicy error for Jupiter strategy on wrong network");
        }
    }

    #[test]
    fn test_relayer_validation_solana_empty_cron_schedule() {
        let swap_config = RelayerSolanaSwapConfig {
            strategy: Some(SolanaSwapStrategy::JupiterSwap),
            cron_schedule: Some("".to_string()), // Empty cron schedule
            ..RelayerSolanaSwapConfig::default()
        };

        let policy = RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::User),
            swap_config: Some(swap_config),
            ..RelayerSolanaPolicy::default()
        };

        let relayer = Relayer::new(
            "valid-id".to_string(),
            "Valid Relayer".to_string(),
            "mainnet-beta".to_string(),
            false,
            RelayerNetworkType::Solana,
            Some(RelayerNetworkPolicy::Solana(policy)),
            "valid-signer".to_string(),
            None,
            None,
        );

        let result = relayer.validate();
        assert!(result.is_err());
        if let Err(RelayerValidationError::InvalidPolicy(msg)) = result {
            assert!(msg.contains("Empty cron schedule is not accepted"));
        } else {
            panic!("Expected InvalidPolicy error for empty cron schedule");
        }
    }

    #[test]
    fn test_relayer_validation_solana_invalid_cron_schedule() {
        let swap_config = RelayerSolanaSwapConfig {
            strategy: Some(SolanaSwapStrategy::JupiterSwap),
            cron_schedule: Some("invalid cron".to_string()), // Invalid cron format
            ..RelayerSolanaSwapConfig::default()
        };

        let policy = RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::User),
            swap_config: Some(swap_config),
            ..RelayerSolanaPolicy::default()
        };

        let relayer = Relayer::new(
            "valid-id".to_string(),
            "Valid Relayer".to_string(),
            "mainnet-beta".to_string(),
            false,
            RelayerNetworkType::Solana,
            Some(RelayerNetworkPolicy::Solana(policy)),
            "valid-signer".to_string(),
            None,
            None,
        );

        let result = relayer.validate();
        assert!(result.is_err());
        if let Err(RelayerValidationError::InvalidPolicy(msg)) = result {
            assert!(msg.contains("Invalid cron schedule format"));
        } else {
            panic!("Expected InvalidPolicy error for invalid cron schedule");
        }
    }

    #[test]
    fn test_relayer_validation_solana_jupiter_options_wrong_strategy() {
        let jupiter_options = JupiterSwapOptions {
            priority_fee_max_lamports: Some(10000),
            priority_level: Some("high".to_string()),
            dynamic_compute_unit_limit: Some(true),
        };

        let swap_config = RelayerSolanaSwapConfig {
            strategy: Some(SolanaSwapStrategy::JupiterUltra), // Wrong strategy
            jupiter_swap_options: Some(jupiter_options),
            ..RelayerSolanaSwapConfig::default()
        };

        let policy = RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::User),
            swap_config: Some(swap_config),
            ..RelayerSolanaPolicy::default()
        };

        let relayer = Relayer::new(
            "valid-id".to_string(),
            "Valid Relayer".to_string(),
            "mainnet-beta".to_string(),
            false,
            RelayerNetworkType::Solana,
            Some(RelayerNetworkPolicy::Solana(policy)),
            "valid-signer".to_string(),
            None,
            None,
        );

        let result = relayer.validate();
        assert!(result.is_err());
        if let Err(RelayerValidationError::InvalidPolicy(msg)) = result {
            assert!(msg.contains("JupiterSwap options are only valid for JupiterSwap strategy"));
        } else {
            panic!("Expected InvalidPolicy error for Jupiter options with wrong strategy");
        }
    }

    #[test]
    fn test_relayer_validation_solana_jupiter_zero_max_lamports() {
        let jupiter_options = JupiterSwapOptions {
            priority_fee_max_lamports: Some(0), // Zero is invalid
            priority_level: Some("high".to_string()),
            dynamic_compute_unit_limit: Some(true),
        };

        let swap_config = RelayerSolanaSwapConfig {
            strategy: Some(SolanaSwapStrategy::JupiterSwap),
            jupiter_swap_options: Some(jupiter_options),
            ..RelayerSolanaSwapConfig::default()
        };

        let policy = RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::User),
            swap_config: Some(swap_config),
            ..RelayerSolanaPolicy::default()
        };

        let relayer = Relayer::new(
            "valid-id".to_string(),
            "Valid Relayer".to_string(),
            "mainnet-beta".to_string(),
            false,
            RelayerNetworkType::Solana,
            Some(RelayerNetworkPolicy::Solana(policy)),
            "valid-signer".to_string(),
            None,
            None,
        );

        let result = relayer.validate();
        assert!(result.is_err());
        if let Err(RelayerValidationError::InvalidPolicy(msg)) = result {
            assert!(msg.contains("Max lamports must be greater than 0"));
        } else {
            panic!("Expected InvalidPolicy error for zero max lamports");
        }
    }

    #[test]
    fn test_relayer_validation_solana_jupiter_empty_priority_level() {
        let jupiter_options = JupiterSwapOptions {
            priority_fee_max_lamports: Some(10000),
            priority_level: Some("".to_string()), // Empty priority level
            dynamic_compute_unit_limit: Some(true),
        };

        let swap_config = RelayerSolanaSwapConfig {
            strategy: Some(SolanaSwapStrategy::JupiterSwap),
            jupiter_swap_options: Some(jupiter_options),
            ..RelayerSolanaSwapConfig::default()
        };

        let policy = RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::User),
            swap_config: Some(swap_config),
            ..RelayerSolanaPolicy::default()
        };

        let relayer = Relayer::new(
            "valid-id".to_string(),
            "Valid Relayer".to_string(),
            "mainnet-beta".to_string(),
            false,
            RelayerNetworkType::Solana,
            Some(RelayerNetworkPolicy::Solana(policy)),
            "valid-signer".to_string(),
            None,
            None,
        );

        let result = relayer.validate();
        assert!(result.is_err());
        if let Err(RelayerValidationError::InvalidPolicy(msg)) = result {
            assert!(msg.contains("Priority level cannot be empty"));
        } else {
            panic!("Expected InvalidPolicy error for empty priority level");
        }
    }

    #[test]
    fn test_relayer_validation_solana_jupiter_invalid_priority_level() {
        let jupiter_options = JupiterSwapOptions {
            priority_fee_max_lamports: Some(10000),
            priority_level: Some("invalid".to_string()), // Invalid priority level
            dynamic_compute_unit_limit: Some(true),
        };

        let swap_config = RelayerSolanaSwapConfig {
            strategy: Some(SolanaSwapStrategy::JupiterSwap),
            jupiter_swap_options: Some(jupiter_options),
            ..RelayerSolanaSwapConfig::default()
        };

        let policy = RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::User),
            swap_config: Some(swap_config),
            ..RelayerSolanaPolicy::default()
        };

        let relayer = Relayer::new(
            "valid-id".to_string(),
            "Valid Relayer".to_string(),
            "mainnet-beta".to_string(),
            false,
            RelayerNetworkType::Solana,
            Some(RelayerNetworkPolicy::Solana(policy)),
            "valid-signer".to_string(),
            None,
            None,
        );

        let result = relayer.validate();
        assert!(result.is_err());
        if let Err(RelayerValidationError::InvalidPolicy(msg)) = result {
            assert!(msg.contains("Priority level must be one of: medium, high, veryHigh"));
        } else {
            panic!("Expected InvalidPolicy error for invalid priority level");
        }
    }

    #[test]
    fn test_relayer_validation_solana_jupiter_missing_priority_fee() {
        let jupiter_options = JupiterSwapOptions {
            priority_fee_max_lamports: None, // Missing
            priority_level: Some("high".to_string()),
            dynamic_compute_unit_limit: Some(true),
        };

        let swap_config = RelayerSolanaSwapConfig {
            strategy: Some(SolanaSwapStrategy::JupiterSwap),
            jupiter_swap_options: Some(jupiter_options),
            ..RelayerSolanaSwapConfig::default()
        };

        let policy = RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::User),
            swap_config: Some(swap_config),
            ..RelayerSolanaPolicy::default()
        };

        let relayer = Relayer::new(
            "valid-id".to_string(),
            "Valid Relayer".to_string(),
            "mainnet-beta".to_string(),
            false,
            RelayerNetworkType::Solana,
            Some(RelayerNetworkPolicy::Solana(policy)),
            "valid-signer".to_string(),
            None,
            None,
        );

        let result = relayer.validate();
        assert!(result.is_err());
        if let Err(RelayerValidationError::InvalidPolicy(msg)) = result {
            assert!(msg.contains("Priority Fee Max lamports must be set if priority level is set"));
        } else {
            panic!("Expected InvalidPolicy error for missing priority fee");
        }
    }

    #[test]
    fn test_relayer_validation_solana_jupiter_missing_priority_level() {
        let jupiter_options = JupiterSwapOptions {
            priority_fee_max_lamports: Some(10000),
            priority_level: None, // Missing
            dynamic_compute_unit_limit: Some(true),
        };

        let swap_config = RelayerSolanaSwapConfig {
            strategy: Some(SolanaSwapStrategy::JupiterSwap),
            jupiter_swap_options: Some(jupiter_options),
            ..RelayerSolanaSwapConfig::default()
        };

        let policy = RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::User),
            swap_config: Some(swap_config),
            ..RelayerSolanaPolicy::default()
        };

        let relayer = Relayer::new(
            "valid-id".to_string(),
            "Valid Relayer".to_string(),
            "mainnet-beta".to_string(),
            false,
            RelayerNetworkType::Solana,
            Some(RelayerNetworkPolicy::Solana(policy)),
            "valid-signer".to_string(),
            None,
            None,
        );

        let result = relayer.validate();
        assert!(result.is_err());
        if let Err(RelayerValidationError::InvalidPolicy(msg)) = result {
            assert!(msg.contains("Priority level must be set if priority fee max lamports is set"));
        } else {
            panic!("Expected InvalidPolicy error for missing priority level");
        }
    }

    // ===== Error Conversion Tests =====

    #[test]
    fn test_relayer_validation_error_to_api_error() {
        use crate::models::ApiError;

        // Test each variant
        let errors = vec![
            (RelayerValidationError::EmptyId, "ID cannot be empty"),
            (RelayerValidationError::InvalidIdFormat, "ID must contain only letters, numbers, dashes and underscores and must be at most 36 characters long"),
            (RelayerValidationError::IdTooLong, "ID must not exceed 36 characters"),
            (RelayerValidationError::EmptyName, "Name cannot be empty"),
            (RelayerValidationError::EmptyNetwork, "Network cannot be empty"),
            (RelayerValidationError::InvalidPolicy("test error".to_string()), "Invalid relayer policy: test error"),
            (RelayerValidationError::InvalidRpcUrl("http://invalid".to_string()), "Invalid RPC URL: http://invalid"),
            (RelayerValidationError::InvalidRpcWeight, "RPC URL weight must be in range 0-100"),
            (RelayerValidationError::InvalidField("test field error".to_string()), "test field error"),
        ];

        for (validation_error, expected_message) in errors {
            let api_error: ApiError = validation_error.into();
            if let ApiError::BadRequest(message) = api_error {
                assert_eq!(message, expected_message);
            } else {
                panic!("Expected BadRequest variant");
            }
        }
    }

    // ===== JSON Patch Tests (already existing) =====

    #[test]
    fn test_apply_json_patch_comprehensive() {
        // Create a sample relayer
        let relayer = Relayer {
            id: "test-relayer".to_string(),
            name: "Original Name".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Evm,
            policies: Some(RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
                min_balance: Some(1000000000000000000),
                gas_limit_estimation: Some(true),
                gas_price_cap: Some(50000000000),
                whitelist_receivers: None,
                eip1559_pricing: Some(false),
                private_transactions: None,
            })),
            signer_id: "test-signer".to_string(),
            notification_id: Some("old-notification".to_string()),
            custom_rpc_urls: None,
        };

        // Create a JSON patch
        let patch = json!({
            "name": "Updated Name via JSON Patch",
            "paused": true,
            "policies": {
                "min_balance": "2000000000000000000",
                "gas_price_cap": null,  // Remove this field
                "eip1559_pricing": true,  // Update this field
                "whitelist_receivers": ["0x123", "0x456"]  // Add this field
                // gas_limit_estimation not mentioned - should remain unchanged
            },
            "notification_id": null, // Remove notification
            "custom_rpc_urls": [{"url": "https://example.com", "weight": 100}]
        });

        // Apply the JSON patch - all logic now handled uniformly!
        let updated_relayer = relayer.apply_json_patch(&patch).unwrap();

        // Verify all updates were applied correctly
        assert_eq!(updated_relayer.name, "Updated Name via JSON Patch");
        assert!(updated_relayer.paused);
        assert_eq!(updated_relayer.notification_id, None); // Removed
        assert!(updated_relayer.custom_rpc_urls.is_some());

        // Verify policy merge patch worked correctly
        if let Some(RelayerNetworkPolicy::Evm(evm_policy)) = updated_relayer.policies {
            assert_eq!(evm_policy.min_balance, Some(2000000000000000000)); // Updated
            assert_eq!(evm_policy.gas_price_cap, None); // Removed (was null)
            assert_eq!(evm_policy.eip1559_pricing, Some(true)); // Updated
            assert_eq!(evm_policy.gas_limit_estimation, Some(true)); // Unchanged
            assert_eq!(
                evm_policy.whitelist_receivers,
                Some(vec!["0x123".to_string(), "0x456".to_string()])
            ); // Added
            assert_eq!(evm_policy.private_transactions, None); // Unchanged
        } else {
            panic!("Expected EVM policy");
        }
    }

    #[test]
    fn test_apply_json_patch_validation_failure() {
        let relayer = Relayer {
            id: "test-relayer".to_string(),
            name: "Original Name".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Evm,
            policies: None,
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        // Invalid patch - field that would make the result invalid
        let invalid_patch = json!({
            "name": ""  // Empty name should fail validation
        });

        // Should fail validation during final validation step
        let result = relayer.apply_json_patch(&invalid_patch);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Relayer name cannot be empty"));
    }

    #[test]
    fn test_apply_json_patch_invalid_result() {
        let relayer = Relayer {
            id: "test-relayer".to_string(),
            name: "Original Name".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Evm,
            policies: None,
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        // Patch that would create an invalid structure
        let invalid_patch = json!({
            "network_type": "invalid_type"  // Invalid enum value
        });

        // Should fail when converting back to domain object
        let result = relayer.apply_json_patch(&invalid_patch);
        assert!(result.is_err());
        // The error now occurs during the initial validation step
        let error_msg = result.unwrap_err().to_string();
        assert!(
            error_msg.contains("Invalid patch format")
                || error_msg.contains("Invalid result after patch")
        );
    }
}
