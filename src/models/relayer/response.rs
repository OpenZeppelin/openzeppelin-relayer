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
    AllowedToken, Relayer, RelayerEvmPolicy, RelayerNetworkPolicy, RelayerNetworkType,
    RelayerRepoModel, RelayerSolanaFeePaymentStrategy, RelayerSolanaPolicy,
    RelayerSolanaSwapPolicy, RelayerStellarPolicy, RpcConfig,
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

/// Relayer response model for API endpoints
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
pub struct RelayerResponse {
    pub id: String,
    pub name: String,
    pub network: String,
    pub network_type: RelayerNetworkType,
    pub paused: bool,
    pub policies: Option<RelayerNetworkPolicy>,
    pub signer_id: String,
    pub notification_id: Option<String>,
    pub custom_rpc_urls: Option<Vec<RpcConfig>>,
    // Runtime fields from repository model
    pub address: Option<String>,
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

impl From<Relayer> for RelayerResponse {
    fn from(relayer: Relayer) -> Self {
        Self {
            id: relayer.id,
            name: relayer.name,
            network: relayer.network,
            network_type: relayer.network_type,
            paused: relayer.paused,
            policies: relayer.policies,
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
        Self {
            id: model.id,
            name: model.name,
            network: model.network,
            network_type: model.network_type.into(),
            paused: model.paused,
            policies: Some(model.policies.into()),
            signer_id: model.signer_id,
            notification_id: model.notification_id,
            custom_rpc_urls: model.custom_rpc_urls,
            address: Some(model.address),
            system_disabled: Some(model.system_disabled),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::relayer::RelayerEvmPolicy;

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
        assert_eq!(response.policies, relayer.policies);
        assert_eq!(response.signer_id, relayer.signer_id);
        assert_eq!(response.notification_id, relayer.notification_id);
        assert_eq!(response.custom_rpc_urls, relayer.custom_rpc_urls);
        assert_eq!(response.address, None);
        assert_eq!(response.system_disabled, None);
    }

    #[test]
    fn test_response_serialization() {
        let response = RelayerResponse {
            id: "test-relayer".to_string(),
            name: "Test Relayer".to_string(),
            network: "mainnet".to_string(),
            network_type: RelayerNetworkType::Evm,
            paused: false,
            policies: Some(RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
                gas_price_cap: Some(100_000_000_000),
                whitelist_receivers: None,
                eip1559_pricing: Some(true),
                private_transactions: None,
                min_balance: None,
                gas_limit_estimation: None,
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
}

/// Network policy response models for OpenAPI documentation
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
pub struct NetworkPolicyResponse {
    #[serde(flatten)]
    pub policy: RelayerNetworkPolicy,
}

/// EVM policy response model for OpenAPI documentation  
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
pub struct EvmPolicyResponse {
    pub min_balance: Option<u128>,
    pub gas_limit_estimation: Option<bool>,
    pub gas_price_cap: Option<u128>,
    pub whitelist_receivers: Option<Vec<String>>,
    pub eip1559_pricing: Option<bool>,
    pub private_transactions: bool,
}

/// Solana policy response model for OpenAPI documentation
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
pub struct SolanaPolicyResponse {
    pub allowed_programs: Option<Vec<String>>,
    pub max_signatures: Option<u8>,
    pub max_tx_data_size: Option<u16>,
    pub min_balance: Option<u64>,
    pub allowed_tokens: Option<Vec<AllowedToken>>,
    pub fee_payment_strategy: Option<RelayerSolanaFeePaymentStrategy>,
    pub fee_margin_percentage: Option<f32>,
    pub allowed_accounts: Option<Vec<String>>,
    pub disallowed_accounts: Option<Vec<String>>,
    pub max_allowed_fee_lamports: Option<u64>,
    pub swap_config: Option<RelayerSolanaSwapPolicy>,
}

/// Stellar policy response model for OpenAPI documentation
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
pub struct StellarPolicyResponse {
    pub max_fee: Option<u32>,
    pub timeout_seconds: Option<u64>,
    pub min_balance: Option<u64>,
}

impl From<RelayerEvmPolicy> for EvmPolicyResponse {
    fn from(policy: RelayerEvmPolicy) -> Self {
        Self {
            min_balance: policy.min_balance,
            gas_limit_estimation: policy.gas_limit_estimation,
            gas_price_cap: policy.gas_price_cap,
            whitelist_receivers: policy.whitelist_receivers,
            eip1559_pricing: policy.eip1559_pricing,
            private_transactions: policy.private_transactions.unwrap_or(false),
        }
    }
}

impl From<RelayerSolanaPolicy> for SolanaPolicyResponse {
    fn from(policy: RelayerSolanaPolicy) -> Self {
        Self {
            allowed_programs: policy.allowed_programs,
            max_signatures: policy.max_signatures,
            max_tx_data_size: policy.max_tx_data_size,
            min_balance: policy.min_balance,
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
            min_balance: policy.min_balance,
            max_fee: policy.max_fee,
            timeout_seconds: policy.timeout_seconds,
        }
    }
}
