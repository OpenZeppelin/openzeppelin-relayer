//! Response models for relayer API endpoints.
//!
//! This module provides response structures used by relayer API endpoints,
//! including:
//!
//! - **Response Models**: Structures returned by API endpoints
//! - **Status Models**: Relayer status and runtime information  
//! - **Conversions**: Mapping from domain and repository models to API responses
//! - **API Compatibility**: Maintaining backward compatibility with existing API contracts
//!
//! These models handle API-specific formatting and serialization while working
//! with the domain model for business logic.

use super::{
    Relayer, RelayerEvmPolicy, RelayerNetworkPolicy, RelayerNetworkType, RelayerRepoModel,
    RelayerSolanaPolicy, RelayerSolanaSwapConfig, RelayerStellarPolicy, RpcConfig,
    SolanaAllowedTokensPolicy, SolanaFeePaymentStrategy,
};
use crate::constants::{
    DEFAULT_EVM_GAS_LIMIT_ESTIMATION, DEFAULT_EVM_MIN_BALANCE, DEFAULT_SOLANA_MAX_TX_DATA_SIZE,
    DEFAULT_SOLANA_MIN_BALANCE, DEFAULT_STELLAR_MIN_BALANCE,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Response for delete pending transactions operation
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
pub struct DeletePendingTransactionsResponse {
    pub queued_for_cancellation_transaction_ids: Vec<String>,
    pub failed_to_queue_transaction_ids: Vec<String>,
    pub total_processed: u32,
}

/// Policy types for responses - these don't include network_type tags
/// since the network_type is already available at the top level of RelayerResponse
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(untagged)]
pub enum RelayerNetworkPolicyResponse {
    // Order matters for untagged enums - put most distinctive variants first
    // EVM has unique fields (gas_price_cap, whitelist_receivers, eip1559_pricing) so it should be tried first
    Evm(EvmPolicyResponse),
    // Stellar has unique fields (max_fee, timeout_seconds) so it should be tried next
    Stellar(StellarPolicyResponse),
    // Solana has many fields but some overlap with others, so it should be tried last
    Solana(SolanaPolicyResponse),
}

impl From<RelayerNetworkPolicy> for RelayerNetworkPolicyResponse {
    fn from(policy: RelayerNetworkPolicy) -> Self {
        match policy {
            RelayerNetworkPolicy::Evm(evm_policy) => {
                RelayerNetworkPolicyResponse::Evm(evm_policy.into())
            }
            RelayerNetworkPolicy::Solana(solana_policy) => {
                RelayerNetworkPolicyResponse::Solana(solana_policy.into())
            }
            RelayerNetworkPolicy::Stellar(stellar_policy) => {
                RelayerNetworkPolicyResponse::Stellar(stellar_policy.into())
            }
        }
    }
}

/// Relayer response model for API endpoints
#[derive(Debug, Serialize, Clone, PartialEq, ToSchema)]
pub struct RelayerResponse {
    pub id: String,
    pub name: String,
    pub network: String,
    pub network_type: RelayerNetworkType,
    pub paused: bool,
    /// Policies without redundant network_type tag - network type is available at top level
    /// Only included if user explicitly provided policies (not shown for empty/default policies)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub policies: Option<RelayerNetworkPolicyResponse>,
    pub signer_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub notification_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub custom_rpc_urls: Option<Vec<RpcConfig>>,
    // Runtime fields from repository model
    #[schema(nullable = false)]
    pub address: Option<String>,
    #[schema(nullable = false)]
    pub system_disabled: Option<bool>,
}

/// Relayer status with runtime information
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
#[serde(tag = "network_type")]
pub enum RelayerStatus {
    #[serde(rename = "evm")]
    Evm {
        balance: String,
        pending_transactions_count: u64,
        last_confirmed_transaction_timestamp: Option<String>,
        system_disabled: bool,
        paused: bool,
        nonce: String,
    },
    #[serde(rename = "stellar")]
    Stellar {
        balance: String,
        pending_transactions_count: u64,
        last_confirmed_transaction_timestamp: Option<String>,
        system_disabled: bool,
        paused: bool,
        sequence_number: String,
    },
    #[serde(rename = "solana")]
    Solana {
        balance: String,
        pending_transactions_count: u64,
        last_confirmed_transaction_timestamp: Option<String>,
        system_disabled: bool,
        paused: bool,
    },
}

/// Convert RelayerNetworkPolicy to RelayerNetworkPolicyResponse based on network type
fn convert_policy_to_response(
    policy: RelayerNetworkPolicy,
    network_type: RelayerNetworkType,
) -> RelayerNetworkPolicyResponse {
    match (policy, network_type) {
        (RelayerNetworkPolicy::Evm(evm_policy), RelayerNetworkType::Evm) => {
            RelayerNetworkPolicyResponse::Evm(EvmPolicyResponse::from(evm_policy))
        }
        (RelayerNetworkPolicy::Solana(solana_policy), RelayerNetworkType::Solana) => {
            RelayerNetworkPolicyResponse::Solana(SolanaPolicyResponse::from(solana_policy))
        }
        (RelayerNetworkPolicy::Stellar(stellar_policy), RelayerNetworkType::Stellar) => {
            RelayerNetworkPolicyResponse::Stellar(StellarPolicyResponse::from(stellar_policy))
        }
        // Handle mismatched cases by falling back to the policy type
        (RelayerNetworkPolicy::Evm(evm_policy), _) => {
            RelayerNetworkPolicyResponse::Evm(EvmPolicyResponse::from(evm_policy))
        }
        (RelayerNetworkPolicy::Solana(solana_policy), _) => {
            RelayerNetworkPolicyResponse::Solana(SolanaPolicyResponse::from(solana_policy))
        }
        (RelayerNetworkPolicy::Stellar(stellar_policy), _) => {
            RelayerNetworkPolicyResponse::Stellar(StellarPolicyResponse::from(stellar_policy))
        }
    }
}

impl From<Relayer> for RelayerResponse {
    fn from(relayer: Relayer) -> Self {
        Self {
            id: relayer.id.clone(),
            name: relayer.name.clone(),
            network: relayer.network.clone(),
            network_type: relayer.network_type,
            paused: relayer.paused,
            policies: relayer
                .policies
                .map(|policy| convert_policy_to_response(policy, relayer.network_type)),
            signer_id: relayer.signer_id,
            notification_id: relayer.notification_id,
            custom_rpc_urls: relayer.custom_rpc_urls,
            address: None,
            system_disabled: None,
        }
    }
}

impl From<RelayerRepoModel> for RelayerResponse {
    fn from(model: RelayerRepoModel) -> Self {
        // Only include policies in response if they have actual user-provided values
        let policies = if is_empty_policy(&model.policies) {
            None // Don't return empty/default policies in API response
        } else {
            Some(convert_policy_to_response(
                model.policies.clone(),
                model.network_type,
            ))
        };

        Self {
            id: model.id,
            name: model.name,
            network: model.network,
            network_type: model.network_type,
            paused: model.paused,
            policies,
            signer_id: model.signer_id,
            notification_id: model.notification_id,
            custom_rpc_urls: model.custom_rpc_urls,
            address: Some(model.address),
            system_disabled: Some(model.system_disabled),
        }
    }
}

/// Custom Deserialize implementation for RelayerResponse that uses network_type to deserialize policies
impl<'de> serde::Deserialize<'de> for RelayerResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        use serde_json::Value;

        // First, deserialize to a generic Value to extract network_type
        let value: Value = Value::deserialize(deserializer)?;

        // Extract the network_type field
        let network_type: RelayerNetworkType = value
            .get("network_type")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .ok_or_else(|| D::Error::missing_field("network_type"))?;

        // Extract policies field if present
        let policies = if let Some(policies_value) = value.get("policies") {
            if policies_value.is_null() {
                None
            } else {
                // Deserialize policies based on network_type
                let policy_response = match network_type {
                    RelayerNetworkType::Evm => {
                        let evm_policy: EvmPolicyResponse =
                            serde_json::from_value(policies_value.clone())
                                .map_err(D::Error::custom)?;
                        RelayerNetworkPolicyResponse::Evm(evm_policy)
                    }
                    RelayerNetworkType::Solana => {
                        let solana_policy: SolanaPolicyResponse =
                            serde_json::from_value(policies_value.clone())
                                .map_err(D::Error::custom)?;
                        RelayerNetworkPolicyResponse::Solana(solana_policy)
                    }
                    RelayerNetworkType::Stellar => {
                        let stellar_policy: StellarPolicyResponse =
                            serde_json::from_value(policies_value.clone())
                                .map_err(D::Error::custom)?;
                        RelayerNetworkPolicyResponse::Stellar(stellar_policy)
                    }
                };
                Some(policy_response)
            }
        } else {
            None
        };

        // Deserialize all other fields normally
        Ok(RelayerResponse {
            id: value
                .get("id")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .ok_or_else(|| D::Error::missing_field("id"))?,
            name: value
                .get("name")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .ok_or_else(|| D::Error::missing_field("name"))?,
            network: value
                .get("network")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .ok_or_else(|| D::Error::missing_field("network"))?,
            network_type,
            paused: value
                .get("paused")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .ok_or_else(|| D::Error::missing_field("paused"))?,
            policies,
            signer_id: value
                .get("signer_id")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .ok_or_else(|| D::Error::missing_field("signer_id"))?,
            notification_id: value
                .get("notification_id")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or(None),
            custom_rpc_urls: value
                .get("custom_rpc_urls")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or(None),
            address: value
                .get("address")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or(None),
            system_disabled: value
                .get("system_disabled")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or(None),
        })
    }
}

/// Check if a policy is "empty" (all fields are None) indicating it's a default
fn is_empty_policy(policy: &RelayerNetworkPolicy) -> bool {
    match policy {
        RelayerNetworkPolicy::Evm(evm_policy) => {
            evm_policy.min_balance.is_none()
                && evm_policy.gas_limit_estimation.is_none()
                && evm_policy.gas_price_cap.is_none()
                && evm_policy.whitelist_receivers.is_none()
                && evm_policy.eip1559_pricing.is_none()
                && evm_policy.private_transactions.is_none()
        }
        RelayerNetworkPolicy::Solana(solana_policy) => {
            solana_policy.allowed_programs.is_none()
                && solana_policy.max_signatures.is_none()
                && solana_policy.max_tx_data_size.is_none()
                && solana_policy.min_balance.is_none()
                && solana_policy.allowed_tokens.is_none()
                && solana_policy.fee_payment_strategy.is_none()
                && solana_policy.fee_margin_percentage.is_none()
                && solana_policy.allowed_accounts.is_none()
                && solana_policy.disallowed_accounts.is_none()
                && solana_policy.max_allowed_fee_lamports.is_none()
                && solana_policy.swap_config.is_none()
        }
        RelayerNetworkPolicy::Stellar(stellar_policy) => {
            stellar_policy.min_balance.is_none()
                && stellar_policy.max_fee.is_none()
                && stellar_policy.timeout_seconds.is_none()
        }
    }
}

/// Network policy response models for OpenAPI documentation
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
pub struct NetworkPolicyResponse {
    #[serde(flatten)]
    pub policy: RelayerNetworkPolicy,
}

/// Default function for EVM min balance
fn default_evm_min_balance() -> u128 {
    DEFAULT_EVM_MIN_BALANCE
}

fn default_evm_gas_limit_estimation() -> bool {
    DEFAULT_EVM_GAS_LIMIT_ESTIMATION
}

/// Default function for Solana min balance
fn default_solana_min_balance() -> u64 {
    DEFAULT_SOLANA_MIN_BALANCE
}

/// Default function for Stellar min balance
fn default_stellar_min_balance() -> u64 {
    DEFAULT_STELLAR_MIN_BALANCE
}

/// Default function for Solana max tx data size
fn default_solana_max_tx_data_size() -> u16 {
    DEFAULT_SOLANA_MAX_TX_DATA_SIZE
}
/// EVM policy response model for OpenAPI documentation  
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EvmPolicyResponse {
    #[serde(
        default = "default_evm_min_balance",
        serialize_with = "crate::utils::serialize_u128_as_number",
        deserialize_with = "crate::utils::deserialize_u128_as_number"
    )]
    #[schema(nullable = false)]
    pub min_balance: u128,
    #[serde(default = "default_evm_gas_limit_estimation")]
    #[schema(nullable = false)]
    pub gas_limit_estimation: bool,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::utils::serialize_optional_u128_as_number",
        deserialize_with = "crate::utils::deserialize_optional_u128_as_number",
        default
    )]
    #[schema(nullable = false)]
    pub gas_price_cap: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub whitelist_receivers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub eip1559_pricing: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub private_transactions: Option<bool>,
}

/// Solana policy response model for OpenAPI documentation
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SolanaPolicyResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub allowed_programs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub max_signatures: Option<u8>,
    #[schema(nullable = false)]
    #[serde(default = "default_solana_max_tx_data_size")]
    pub max_tx_data_size: u16,
    #[serde(default = "default_solana_min_balance")]
    #[schema(nullable = false)]
    pub min_balance: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub allowed_tokens: Option<Vec<SolanaAllowedTokensPolicy>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub fee_payment_strategy: Option<SolanaFeePaymentStrategy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub fee_margin_percentage: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub allowed_accounts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub disallowed_accounts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub max_allowed_fee_lamports: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub swap_config: Option<RelayerSolanaSwapConfig>,
}

/// Stellar policy response model for OpenAPI documentation
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StellarPolicyResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub max_fee: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub timeout_seconds: Option<u64>,
    #[serde(default = "default_stellar_min_balance")]
    #[schema(nullable = false)]
    pub min_balance: u64,
}

impl From<RelayerEvmPolicy> for EvmPolicyResponse {
    fn from(policy: RelayerEvmPolicy) -> Self {
        Self {
            min_balance: policy.min_balance.unwrap_or(DEFAULT_EVM_MIN_BALANCE),
            gas_limit_estimation: policy
                .gas_limit_estimation
                .unwrap_or(DEFAULT_EVM_GAS_LIMIT_ESTIMATION),
            gas_price_cap: policy.gas_price_cap,
            whitelist_receivers: policy.whitelist_receivers,
            eip1559_pricing: policy.eip1559_pricing,
            private_transactions: policy.private_transactions,
        }
    }
}

impl From<RelayerSolanaPolicy> for SolanaPolicyResponse {
    fn from(policy: RelayerSolanaPolicy) -> Self {
        Self {
            allowed_programs: policy.allowed_programs,
            max_signatures: policy.max_signatures,
            max_tx_data_size: policy
                .max_tx_data_size
                .unwrap_or(DEFAULT_SOLANA_MAX_TX_DATA_SIZE),
            min_balance: policy.min_balance.unwrap_or(DEFAULT_SOLANA_MIN_BALANCE),
            allowed_tokens: policy.allowed_tokens,
            fee_payment_strategy: policy.fee_payment_strategy,
            fee_margin_percentage: policy.fee_margin_percentage,
            allowed_accounts: policy.allowed_accounts,
            disallowed_accounts: policy.disallowed_accounts,
            max_allowed_fee_lamports: policy.max_allowed_fee_lamports,
            swap_config: policy.swap_config,
        }
    }
}

impl From<RelayerStellarPolicy> for StellarPolicyResponse {
    fn from(policy: RelayerStellarPolicy) -> Self {
        Self {
            min_balance: policy.min_balance.unwrap_or(DEFAULT_STELLAR_MIN_BALANCE),
            max_fee: policy.max_fee,
            timeout_seconds: policy.timeout_seconds,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::relayer::{
        RelayerEvmPolicy, RelayerSolanaPolicy, RelayerSolanaSwapConfig, RelayerStellarPolicy,
        SolanaAllowedTokensPolicy, SolanaFeePaymentStrategy, SolanaSwapStrategy,
    };

    #[test]
    fn test_from_domain_relayer() {
        let relayer = Relayer::new(
            "test-relayer".to_string(),
            "Test Relayer".to_string(),
            "mainnet".to_string(),
            false,
            RelayerNetworkType::Evm,
            Some(RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
                gas_price_cap: Some(100_000_000_000),
                whitelist_receivers: None,
                eip1559_pricing: Some(true),
                private_transactions: None,
                min_balance: None,
                gas_limit_estimation: None,
            })),
            "test-signer".to_string(),
            None,
            None,
        );

        let response: RelayerResponse = relayer.clone().into();

        assert_eq!(response.id, relayer.id);
        assert_eq!(response.name, relayer.name);
        assert_eq!(response.network, relayer.network);
        assert_eq!(response.network_type, relayer.network_type);
        assert_eq!(response.paused, relayer.paused);
        assert_eq!(
            response.policies,
            Some(RelayerNetworkPolicyResponse::Evm(
                RelayerEvmPolicy {
                    gas_price_cap: Some(100_000_000_000),
                    whitelist_receivers: None,
                    eip1559_pricing: Some(true),
                    private_transactions: None,
                    min_balance: Some(DEFAULT_EVM_MIN_BALANCE),
                    gas_limit_estimation: Some(DEFAULT_EVM_GAS_LIMIT_ESTIMATION),
                }
                .into()
            ))
        );
        assert_eq!(response.signer_id, relayer.signer_id);
        assert_eq!(response.notification_id, relayer.notification_id);
        assert_eq!(response.custom_rpc_urls, relayer.custom_rpc_urls);
        assert_eq!(response.address, None);
        assert_eq!(response.system_disabled, None);
    }

    #[test]
    fn test_from_domain_relayer_solana() {
        let relayer = Relayer::new(
            "test-solana-relayer".to_string(),
            "Test Solana Relayer".to_string(),
            "mainnet".to_string(),
            false,
            RelayerNetworkType::Solana,
            Some(RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
                allowed_programs: Some(vec!["11111111111111111111111111111111".to_string()]),
                max_signatures: Some(5),
                min_balance: Some(1000000),
                fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
                allowed_tokens: Some(vec![SolanaAllowedTokensPolicy::new(
                    "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                    Some(100000),
                    None,
                )]),
                max_tx_data_size: None,
                fee_margin_percentage: None,
                allowed_accounts: None,
                disallowed_accounts: None,
                max_allowed_fee_lamports: None,
                swap_config: None,
            })),
            "test-signer".to_string(),
            None,
            None,
        );

        let response: RelayerResponse = relayer.clone().into();

        assert_eq!(response.id, relayer.id);
        assert_eq!(response.network_type, RelayerNetworkType::Solana);
        assert!(response.policies.is_some());

        if let Some(RelayerNetworkPolicyResponse::Solana(solana_response)) = response.policies {
            assert_eq!(solana_response.min_balance, 1000000);
            assert_eq!(solana_response.max_signatures, Some(5));
        } else {
            panic!("Expected Solana policy response");
        }
    }

    #[test]
    fn test_from_domain_relayer_stellar() {
        let relayer = Relayer::new(
            "test-stellar-relayer".to_string(),
            "Test Stellar Relayer".to_string(),
            "mainnet".to_string(),
            false,
            RelayerNetworkType::Stellar,
            Some(RelayerNetworkPolicy::Stellar(RelayerStellarPolicy {
                min_balance: Some(20000000),
                max_fee: Some(100000),
                timeout_seconds: Some(30),
            })),
            "test-signer".to_string(),
            None,
            None,
        );

        let response: RelayerResponse = relayer.clone().into();

        assert_eq!(response.id, relayer.id);
        assert_eq!(response.network_type, RelayerNetworkType::Stellar);
        assert!(response.policies.is_some());

        if let Some(RelayerNetworkPolicyResponse::Stellar(stellar_response)) = response.policies {
            assert_eq!(stellar_response.min_balance, 20000000);
        } else {
            panic!("Expected Stellar policy response");
        }
    }

    #[test]
    fn test_response_serialization() {
        let response = RelayerResponse {
            id: "test-relayer".to_string(),
            name: "Test Relayer".to_string(),
            network: "mainnet".to_string(),
            network_type: RelayerNetworkType::Evm,
            paused: false,
            policies: Some(RelayerNetworkPolicyResponse::Evm(EvmPolicyResponse {
                gas_price_cap: Some(50000000000),
                whitelist_receivers: None,
                eip1559_pricing: Some(true),
                private_transactions: None,
                min_balance: DEFAULT_EVM_MIN_BALANCE,
                gas_limit_estimation: DEFAULT_EVM_GAS_LIMIT_ESTIMATION,
            })),
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
            address: Some("0x123...".to_string()),
            system_disabled: Some(false),
        };

        // Should serialize without errors
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.is_empty());

        // Should deserialize back to the same struct
        let deserialized: RelayerResponse = serde_json::from_str(&serialized).unwrap();
        assert_eq!(response.id, deserialized.id);
        assert_eq!(response.name, deserialized.name);
    }

    #[test]
    fn test_solana_response_serialization() {
        let response = RelayerResponse {
            id: "test-solana-relayer".to_string(),
            name: "Test Solana Relayer".to_string(),
            network: "mainnet".to_string(),
            network_type: RelayerNetworkType::Solana,
            paused: false,
            policies: Some(RelayerNetworkPolicyResponse::Solana(SolanaPolicyResponse {
                allowed_programs: Some(vec!["11111111111111111111111111111111".to_string()]),
                max_signatures: Some(5),
                max_tx_data_size: DEFAULT_SOLANA_MAX_TX_DATA_SIZE,
                min_balance: 1000000,
                allowed_tokens: Some(vec![SolanaAllowedTokensPolicy::new(
                    "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                    Some(100000),
                    None,
                )]),
                fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
                fee_margin_percentage: Some(5.0),
                allowed_accounts: None,
                disallowed_accounts: None,
                max_allowed_fee_lamports: Some(500000),
                swap_config: Some(RelayerSolanaSwapConfig {
                    strategy: Some(SolanaSwapStrategy::JupiterSwap),
                    cron_schedule: Some("0 0 * * *".to_string()),
                    min_balance_threshold: Some(500000),
                    jupiter_swap_options: None,
                }),
            })),
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
            address: Some("SolanaAddress123...".to_string()),
            system_disabled: Some(false),
        };

        // Should serialize without errors
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.is_empty());

        // Should deserialize back to the same struct
        let deserialized: RelayerResponse = serde_json::from_str(&serialized).unwrap();
        assert_eq!(response.id, deserialized.id);
        assert_eq!(response.network_type, RelayerNetworkType::Solana);
    }

    #[test]
    fn test_stellar_response_serialization() {
        let response = RelayerResponse {
            id: "test-stellar-relayer".to_string(),
            name: "Test Stellar Relayer".to_string(),
            network: "mainnet".to_string(),
            network_type: RelayerNetworkType::Stellar,
            paused: false,
            policies: Some(RelayerNetworkPolicyResponse::Stellar(
                StellarPolicyResponse {
                    max_fee: Some(5000),
                    timeout_seconds: None,
                    min_balance: 20000000,
                },
            )),
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
            address: Some("GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX".to_string()),
            system_disabled: Some(false),
        };

        // Should serialize without errors
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.is_empty());

        // Should deserialize back to the same struct
        let deserialized: RelayerResponse = serde_json::from_str(&serialized).unwrap();
        assert_eq!(response.id, deserialized.id);
        assert_eq!(response.network_type, RelayerNetworkType::Stellar);

        // Verify Stellar-specific fields
        if let Some(RelayerNetworkPolicyResponse::Stellar(stellar_policy)) = deserialized.policies {
            assert_eq!(stellar_policy.min_balance, 20000000);
            assert_eq!(stellar_policy.max_fee, Some(5000));
            assert_eq!(stellar_policy.timeout_seconds, None);
        } else {
            panic!("Expected Stellar policy in deserialized response");
        }
    }

    #[test]
    fn test_response_without_redundant_network_type() {
        let response = RelayerResponse {
            id: "test-relayer".to_string(),
            name: "Test Relayer".to_string(),
            network: "mainnet".to_string(),
            network_type: RelayerNetworkType::Evm,
            paused: false,
            policies: Some(RelayerNetworkPolicyResponse::Evm(EvmPolicyResponse {
                gas_price_cap: Some(100_000_000_000),
                whitelist_receivers: None,
                eip1559_pricing: Some(true),
                private_transactions: None,
                min_balance: DEFAULT_EVM_MIN_BALANCE,
                gas_limit_estimation: DEFAULT_EVM_GAS_LIMIT_ESTIMATION,
            })),
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
            address: Some("0x123...".to_string()),
            system_disabled: Some(false),
        };

        let serialized = serde_json::to_string_pretty(&response).unwrap();

        assert!(serialized.contains(r#""network_type": "evm""#));

        // Count occurrences - should only be 1 (at top level)
        let network_type_count = serialized.matches(r#""network_type""#).count();
        assert_eq!(
            network_type_count, 1,
            "Should only have one network_type field at top level, not in policies"
        );

        assert!(serialized.contains(r#""gas_price_cap": 100000000000"#));
        assert!(serialized.contains(r#""eip1559_pricing": true"#));
    }

    #[test]
    fn test_solana_response_without_redundant_network_type() {
        let response = RelayerResponse {
            id: "test-solana-relayer".to_string(),
            name: "Test Solana Relayer".to_string(),
            network: "mainnet".to_string(),
            network_type: RelayerNetworkType::Solana,
            paused: false,
            policies: Some(RelayerNetworkPolicyResponse::Solana(SolanaPolicyResponse {
                allowed_programs: Some(vec!["11111111111111111111111111111111".to_string()]),
                max_signatures: Some(5),
                max_tx_data_size: DEFAULT_SOLANA_MAX_TX_DATA_SIZE,
                min_balance: 1000000,
                allowed_tokens: None,
                fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
                fee_margin_percentage: None,
                allowed_accounts: None,
                disallowed_accounts: None,
                max_allowed_fee_lamports: None,
                swap_config: None,
            })),
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
            address: Some("SolanaAddress123...".to_string()),
            system_disabled: Some(false),
        };

        let serialized = serde_json::to_string_pretty(&response).unwrap();

        assert!(serialized.contains(r#""network_type": "solana""#));

        // Count occurrences - should only be 1 (at top level)
        let network_type_count = serialized.matches(r#""network_type""#).count();
        assert_eq!(
            network_type_count, 1,
            "Should only have one network_type field at top level, not in policies"
        );

        assert!(serialized.contains(r#""max_signatures": 5"#));
        assert!(serialized.contains(r#""fee_payment_strategy": "relayer""#));
    }

    #[test]
    fn test_stellar_response_without_redundant_network_type() {
        let response = RelayerResponse {
            id: "test-stellar-relayer".to_string(),
            name: "Test Stellar Relayer".to_string(),
            network: "mainnet".to_string(),
            network_type: RelayerNetworkType::Stellar,
            paused: false,
            policies: Some(RelayerNetworkPolicyResponse::Stellar(
                StellarPolicyResponse {
                    min_balance: 20000000,
                    max_fee: Some(100000),
                    timeout_seconds: Some(30),
                },
            )),
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
            address: Some("GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX".to_string()),
            system_disabled: Some(false),
        };

        let serialized = serde_json::to_string_pretty(&response).unwrap();

        assert!(serialized.contains(r#""network_type": "stellar""#));

        // Count occurrences - should only be 1 (at top level)
        let network_type_count = serialized.matches(r#""network_type""#).count();
        assert_eq!(
            network_type_count, 1,
            "Should only have one network_type field at top level, not in policies"
        );

        assert!(serialized.contains(r#""min_balance": 20000000"#));
        assert!(serialized.contains(r#""max_fee": 100000"#));
        assert!(serialized.contains(r#""timeout_seconds": 30"#));
    }

    #[test]
    fn test_empty_policies_not_returned_in_response() {
        // Create a repository model with empty policies (all None - user didn't set any)
        let repo_model = RelayerRepoModel {
            id: "test-relayer".to_string(),
            name: "Test Relayer".to_string(),
            network: "mainnet".to_string(),
            network_type: RelayerNetworkType::Evm,
            paused: false,
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()), // All None values
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
            address: "0x123...".to_string(),
            system_disabled: false,
        };

        // Convert to response
        let response = RelayerResponse::from(repo_model);

        // Empty policies should not be included in response
        assert_eq!(response.policies, None);

        // Verify serialization doesn't include policies field
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(
            !serialized.contains("policies"),
            "Empty policies should not appear in JSON response"
        );
    }

    #[test]
    fn test_empty_solana_policies_not_returned_in_response() {
        // Create a repository model with empty Solana policies (all None - user didn't set any)
        let repo_model = RelayerRepoModel {
            id: "test-solana-relayer".to_string(),
            name: "Test Solana Relayer".to_string(),
            network: "mainnet".to_string(),
            network_type: RelayerNetworkType::Solana,
            paused: false,
            policies: RelayerNetworkPolicy::Solana(RelayerSolanaPolicy::default()), // All None values
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
            address: "SolanaAddress123...".to_string(),
            system_disabled: false,
        };

        // Convert to response
        let response = RelayerResponse::from(repo_model);

        // Empty policies should not be included in response
        assert_eq!(response.policies, None);

        // Verify serialization doesn't include policies field
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(
            !serialized.contains("policies"),
            "Empty Solana policies should not appear in JSON response"
        );
    }

    #[test]
    fn test_empty_stellar_policies_not_returned_in_response() {
        // Create a repository model with empty Stellar policies (all None - user didn't set any)
        let repo_model = RelayerRepoModel {
            id: "test-stellar-relayer".to_string(),
            name: "Test Stellar Relayer".to_string(),
            network: "mainnet".to_string(),
            network_type: RelayerNetworkType::Stellar,
            paused: false,
            policies: RelayerNetworkPolicy::Stellar(RelayerStellarPolicy::default()), // All None values
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
            address: "GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX".to_string(),
            system_disabled: false,
        };

        // Convert to response
        let response = RelayerResponse::from(repo_model);

        // Empty policies should not be included in response
        assert_eq!(response.policies, None);

        // Verify serialization doesn't include policies field
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(
            !serialized.contains("policies"),
            "Empty Stellar policies should not appear in JSON response"
        );
    }

    #[test]
    fn test_user_provided_policies_returned_in_response() {
        // Create a repository model with user-provided policies
        let repo_model = RelayerRepoModel {
            id: "test-relayer".to_string(),
            name: "Test Relayer".to_string(),
            network: "mainnet".to_string(),
            network_type: RelayerNetworkType::Evm,
            paused: false,
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
                gas_price_cap: Some(100_000_000_000),
                eip1559_pricing: Some(true),
                min_balance: None, // Some fields can still be None
                gas_limit_estimation: None,
                whitelist_receivers: None,
                private_transactions: None,
            }),
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
            address: "0x123...".to_string(),
            system_disabled: false,
        };

        // Convert to response
        let response = RelayerResponse::from(repo_model);

        // User-provided policies should be included in response
        assert!(response.policies.is_some());

        // Verify serialization includes policies field
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(
            serialized.contains("policies"),
            "User-provided policies should appear in JSON response"
        );
        assert!(
            serialized.contains("gas_price_cap"),
            "User-provided policy values should appear in JSON response"
        );
    }

    #[test]
    fn test_user_provided_solana_policies_returned_in_response() {
        // Create a repository model with user-provided Solana policies
        let repo_model = RelayerRepoModel {
            id: "test-solana-relayer".to_string(),
            name: "Test Solana Relayer".to_string(),
            network: "mainnet".to_string(),
            network_type: RelayerNetworkType::Solana,
            paused: false,
            policies: RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
                max_signatures: Some(5),
                fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
                min_balance: Some(1000000),
                allowed_programs: None, // Some fields can still be None
                max_tx_data_size: None,
                allowed_tokens: None,
                fee_margin_percentage: None,
                allowed_accounts: None,
                disallowed_accounts: None,
                max_allowed_fee_lamports: None,
                swap_config: None,
            }),
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
            address: "SolanaAddress123...".to_string(),
            system_disabled: false,
        };

        // Convert to response
        let response = RelayerResponse::from(repo_model);

        // User-provided policies should be included in response
        assert!(response.policies.is_some());

        // Verify serialization includes policies field
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(
            serialized.contains("policies"),
            "User-provided Solana policies should appear in JSON response"
        );
        assert!(
            serialized.contains("max_signatures"),
            "User-provided Solana policy values should appear in JSON response"
        );
        assert!(
            serialized.contains("fee_payment_strategy"),
            "User-provided Solana policy values should appear in JSON response"
        );
    }

    #[test]
    fn test_user_provided_stellar_policies_returned_in_response() {
        // Create a repository model with user-provided Stellar policies
        let repo_model = RelayerRepoModel {
            id: "test-stellar-relayer".to_string(),
            name: "Test Stellar Relayer".to_string(),
            network: "mainnet".to_string(),
            network_type: RelayerNetworkType::Stellar,
            paused: false,
            policies: RelayerNetworkPolicy::Stellar(RelayerStellarPolicy {
                max_fee: Some(100000),
                timeout_seconds: Some(30),
                min_balance: None, // Some fields can still be None
            }),
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
            address: "GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX".to_string(),
            system_disabled: false,
        };

        // Convert to response
        let response = RelayerResponse::from(repo_model);

        // User-provided policies should be included in response
        assert!(response.policies.is_some());

        // Verify serialization includes policies field
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(
            serialized.contains("policies"),
            "User-provided Stellar policies should appear in JSON response"
        );
        assert!(
            serialized.contains("max_fee"),
            "User-provided Stellar policy values should appear in JSON response"
        );
        assert!(
            serialized.contains("timeout_seconds"),
            "User-provided Stellar policy values should appear in JSON response"
        );
    }

    #[test]
    fn test_relayer_status_serialization() {
        // Test EVM status
        let evm_status = RelayerStatus::Evm {
            balance: "1000000000000000000".to_string(),
            pending_transactions_count: 5,
            last_confirmed_transaction_timestamp: Some("2024-01-01T00:00:00Z".to_string()),
            system_disabled: false,
            paused: false,
            nonce: "42".to_string(),
        };

        let serialized = serde_json::to_string(&evm_status).unwrap();
        assert!(serialized.contains(r#""network_type":"evm""#));
        assert!(serialized.contains(r#""nonce":"42""#));
        assert!(serialized.contains(r#""balance":"1000000000000000000""#));

        // Test Solana status
        let solana_status = RelayerStatus::Solana {
            balance: "5000000000".to_string(),
            pending_transactions_count: 3,
            last_confirmed_transaction_timestamp: None,
            system_disabled: false,
            paused: true,
        };

        let serialized = serde_json::to_string(&solana_status).unwrap();
        assert!(serialized.contains(r#""network_type":"solana""#));
        assert!(serialized.contains(r#""balance":"5000000000""#));
        assert!(serialized.contains(r#""paused":true"#));

        // Test Stellar status
        let stellar_status = RelayerStatus::Stellar {
            balance: "1000000000".to_string(),
            pending_transactions_count: 2,
            last_confirmed_transaction_timestamp: Some("2024-01-01T12:00:00Z".to_string()),
            system_disabled: true,
            paused: false,
            sequence_number: "123456789".to_string(),
        };

        let serialized = serde_json::to_string(&stellar_status).unwrap();
        assert!(serialized.contains(r#""network_type":"stellar""#));
        assert!(serialized.contains(r#""sequence_number":"123456789""#));
        assert!(serialized.contains(r#""system_disabled":true"#));
    }

    #[test]
    fn test_relayer_status_deserialization() {
        // Test EVM status deserialization
        let evm_json = r#"{
            "network_type": "evm",
            "balance": "1000000000000000000",
            "pending_transactions_count": 5,
            "last_confirmed_transaction_timestamp": "2024-01-01T00:00:00Z",
            "system_disabled": false,
            "paused": false,
            "nonce": "42"
        }"#;

        let status: RelayerStatus = serde_json::from_str(evm_json).unwrap();
        if let RelayerStatus::Evm { nonce, balance, .. } = status {
            assert_eq!(nonce, "42");
            assert_eq!(balance, "1000000000000000000");
        } else {
            panic!("Expected EVM status");
        }

        // Test Solana status deserialization
        let solana_json = r#"{
            "network_type": "solana",
            "balance": "5000000000",
            "pending_transactions_count": 3,
            "last_confirmed_transaction_timestamp": null,
            "system_disabled": false,
            "paused": true
        }"#;

        let status: RelayerStatus = serde_json::from_str(solana_json).unwrap();
        if let RelayerStatus::Solana {
            balance, paused, ..
        } = status
        {
            assert_eq!(balance, "5000000000");
            assert!(paused);
        } else {
            panic!("Expected Solana status");
        }

        // Test Stellar status deserialization
        let stellar_json = r#"{
            "network_type": "stellar",
            "balance": "1000000000",
            "pending_transactions_count": 2,
            "last_confirmed_transaction_timestamp": "2024-01-01T12:00:00Z",
            "system_disabled": true,
            "paused": false,
            "sequence_number": "123456789"
        }"#;

        let status: RelayerStatus = serde_json::from_str(stellar_json).unwrap();
        if let RelayerStatus::Stellar {
            sequence_number,
            system_disabled,
            ..
        } = status
        {
            assert_eq!(sequence_number, "123456789");
            assert!(system_disabled);
        } else {
            panic!("Expected Stellar status");
        }
    }
}
