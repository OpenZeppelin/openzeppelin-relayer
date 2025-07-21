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

mod request;
pub use request::*;

mod response;
pub use response::*;

pub mod repository;
pub use repository::*;

mod rpc_config;
pub use rpc_config::*;

use crate::{
    config::ConfigFileNetworkType,
    constants::{
        DEFAULT_EVM_EIP1559_ENABLED, DEFAULT_EVM_GAS_LIMIT_ESTIMATION, DEFAULT_EVM_MIN_BALANCE,
        DEFAULT_EVM_PRIVATE_TRANSACTIONS, DEFAULT_SOLANA_MAX_TX_DATA_SIZE,
        DEFAULT_SOLANA_MIN_BALANCE, DEFAULT_STELLAR_MIN_BALANCE, ID_REGEX,
    },
    utils::deserialize_optional_u128,
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
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema, PartialEq)]
pub struct RelayerEvmPolicy {
    pub min_balance: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gas_limit_estimation: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(deserialize_with = "deserialize_optional_u128", default)]
    pub gas_price_cap: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whitelist_receivers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eip1559_pricing: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_transactions: Option<bool>,
}

impl Default for RelayerEvmPolicy {
    fn default() -> Self {
        Self {
            min_balance: Some(DEFAULT_EVM_MIN_BALANCE),
            gas_limit_estimation: Some(DEFAULT_EVM_GAS_LIMIT_ESTIMATION),
            gas_price_cap: Some(u128::MAX),
            whitelist_receivers: None,
            eip1559_pricing: Some(DEFAULT_EVM_EIP1559_ENABLED),
            private_transactions: Some(DEFAULT_EVM_PRIVATE_TRANSACTIONS),
        }
    }
}

/// Solana token swap configuration
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema, PartialEq, Default)]
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

/// Configuration for allowed token handling on Solana
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct AllowedToken {
    pub mint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decimals: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_allowed_fee: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swap_config: Option<AllowedTokenSwapConfig>,
}

impl AllowedToken {
    /// Create a new AllowedToken with required parameters
    pub fn new(
        mint: String,
        max_allowed_fee: Option<u64>,
        swap_config: Option<AllowedTokenSwapConfig>,
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
        swap_config: Option<AllowedTokenSwapConfig>,
    ) -> Self {
        Self::new(mint, max_allowed_fee, swap_config)
    }
}

/// Solana fee payment strategy
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RelayerSolanaFeePaymentStrategy {
    User,
    Relayer,
}

impl Default for RelayerSolanaFeePaymentStrategy {
    fn default() -> Self {
        Self::User
    }
}

/// Solana swap strategy
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RelayerSolanaSwapStrategy {
    JupiterSwap,
    JupiterUltra,
    Noop,
}

impl Default for RelayerSolanaSwapStrategy {
    fn default() -> Self {
        Self::Noop
    }
}

/// Jupiter swap options
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema, PartialEq)]
pub struct JupiterSwapOptions {
    /// Maximum priority fee (in lamports) for a transaction. Optional.
    pub priority_fee_max_lamports: Option<u64>,
    /// Priority. Optional.
    pub priority_level: Option<String>,
    pub dynamic_compute_unit_limit: Option<bool>,
}

/// Solana swap policy configuration
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema, PartialEq)]
pub struct RelayerSolanaSwapPolicy {
    /// DEX strategy to use for token swaps.
    pub strategy: Option<RelayerSolanaSwapStrategy>,
    /// Cron schedule for executing token swap logic to keep relayer funded. Optional.
    pub cron_schedule: Option<String>,
    /// Min sol balance to execute token swap logic to keep relayer funded. Optional.
    pub min_balance_threshold: Option<u64>,
    /// Swap options for JupiterSwap strategy. Optional.
    pub jupiter_swap_options: Option<JupiterSwapOptions>,
}

impl Default for RelayerSolanaSwapPolicy {
    fn default() -> Self {
        Self {
            strategy: Some(RelayerSolanaSwapStrategy::default()),
            cron_schedule: None,
            min_balance_threshold: None,
            jupiter_swap_options: None,
        }
    }
}

/// Solana-specific relayer policy configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
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
    pub allowed_tokens: Option<Vec<AllowedToken>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_payment_strategy: Option<RelayerSolanaFeePaymentStrategy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_margin_percentage: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_accounts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disallowed_accounts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_allowed_fee_lamports: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swap_config: Option<RelayerSolanaSwapPolicy>,
}

impl Default for RelayerSolanaPolicy {
    fn default() -> Self {
        Self {
            fee_payment_strategy: Some(RelayerSolanaFeePaymentStrategy::User),
            fee_margin_percentage: None,
            min_balance: Some(DEFAULT_SOLANA_MIN_BALANCE),
            allowed_tokens: None,
            allowed_programs: None,
            allowed_accounts: None,
            disallowed_accounts: None,
            max_signatures: None,
            max_tx_data_size: Some(DEFAULT_SOLANA_MAX_TX_DATA_SIZE),
            max_allowed_fee_lamports: None,
            swap_config: None,
        }
    }
}

impl RelayerSolanaPolicy {
    /// Get allowed tokens for this policy
    pub fn get_allowed_tokens(&self) -> Vec<AllowedToken> {
        self.allowed_tokens.clone().unwrap_or_default()
    }

    /// Get allowed token entry by mint address
    pub fn get_allowed_token_entry(&self, mint: &str) -> Option<AllowedToken> {
        self.allowed_tokens
            .clone()
            .unwrap_or_default()
            .into_iter()
            .find(|entry| entry.mint == mint)
    }

    /// Get swap configuration for this policy
    pub fn get_swap_config(&self) -> Option<RelayerSolanaSwapPolicy> {
        self.swap_config.clone()
    }

    /// Get allowed token decimals by mint address
    pub fn get_allowed_token_decimals(&self, mint: &str) -> Option<u8> {
        self.get_allowed_token_entry(mint)
            .and_then(|entry| entry.decimals)
    }
}
/// Stellar-specific relayer policy configuration
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema, PartialEq)]
pub struct RelayerStellarPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_balance: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_fee: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
}

impl Default for RelayerStellarPolicy {
    fn default() -> Self {
        Self {
            max_fee: None,
            timeout_seconds: None,
            min_balance: Some(DEFAULT_STELLAR_MIN_BALANCE),
        }
    }
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
        swap_config: &RelayerSolanaSwapPolicy,
        policy: &RelayerSolanaPolicy,
    ) -> Result<(), RelayerValidationError> {
        // Swap config only supported for user fee payment strategy
        if let Some(fee_payment_strategy) = &policy.fee_payment_strategy {
            if *fee_payment_strategy == RelayerSolanaFeePaymentStrategy::Relayer {
                return Err(RelayerValidationError::InvalidPolicy(
                    "Swap config only supported for user fee payment strategy".into(),
                ));
            }
        }

        // Validate strategy-specific restrictions
        if let Some(strategy) = &swap_config.strategy {
            match strategy {
                RelayerSolanaSwapStrategy::JupiterSwap
                | RelayerSolanaSwapStrategy::JupiterUltra => {
                    if self.network != "mainnet-beta" {
                        return Err(RelayerValidationError::InvalidPolicy(format!(
                            "{:?} strategy is only supported on mainnet-beta",
                            strategy
                        )));
                    }
                }
                RelayerSolanaSwapStrategy::Noop => {
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
            if swap_config.strategy != Some(RelayerSolanaSwapStrategy::JupiterSwap) {
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

    /// Applies an update request to create a new validated relayer
    ///
    /// This method provides a domain-first approach where the core model handles
    /// its own business rules and validation rather than having update logic
    /// scattered across request models.
    ///
    /// # Arguments
    /// * `request` - The update request containing partial data to apply
    ///
    /// # Returns
    /// * `Ok(Relayer)` - A new validated relayer with updates applied
    /// * `Err(RelayerValidationError)` - If the resulting relayer would be invalid
    pub fn apply_update(
        &self,
        request: &UpdateRelayerRequest,
    ) -> Result<Self, RelayerValidationError> {
        let mut updated = self.clone();

        // Apply updates from request
        if let Some(name) = &request.name {
            updated.name = name.clone();
        }

        if let Some(paused) = request.paused {
            updated.paused = paused;
        }

        if let Some(network_type) = &request.network_type {
            updated.network_type = network_type.clone();
        }

        if let Some(policies) = &request.policies {
            updated.policies = Some(policies.clone());
        }

        if let Some(notification_id) = &request.notification_id {
            updated.notification_id = if notification_id.is_empty() {
                None
            } else {
                Some(notification_id.clone())
            };
        }

        if let Some(custom_rpc_urls) = &request.custom_rpc_urls {
            updated.custom_rpc_urls = Some(custom_rpc_urls.clone());
        }

        // Validate the complete updated model
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
        })
    }
}
