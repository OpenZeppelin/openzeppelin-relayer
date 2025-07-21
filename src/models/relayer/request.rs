//! Request models for relayer API endpoints.
//!
//! This module provides request structures used by relayer CRUD API endpoints,
//! including:
//!
//! - **Create Requests**: New relayer creation
//! - **Update Requests**: Partial relayer updates
//! - **Validation**: Input validation and error handling
//! - **Conversions**: Mapping between API requests and domain models
//!
//! These models handle API-specific concerns like optional fields for updates
//! while delegating business logic validation to the domain model.

use super::{RelayerNetworkPolicy, RelayerNetworkType, RpcConfig};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Request model for creating a new relayer
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateRelayerRequest {
    pub id: String,
    pub name: String,
    pub network: String,
    pub paused: bool,
    pub network_type: RelayerNetworkType,
    pub policies: Option<RelayerNetworkPolicy>,
    pub signer_id: String,
    pub notification_id: Option<String>,
    pub custom_rpc_urls: Option<Vec<RpcConfig>>,
}

/// Request model for updating an existing relayer
/// All fields are optional to allow partial updates
/// Note: network and signer_id are not updateable after creation
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UpdateRelayerRequest {
    pub name: Option<String>,
    pub paused: Option<bool>,
    pub network_type: Option<RelayerNetworkType>,
    pub policies: Option<RelayerNetworkPolicy>,
    pub notification_id: Option<String>,
    pub custom_rpc_urls: Option<Vec<RpcConfig>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::relayer::{Relayer, RelayerEvmPolicy, RelayerNetworkType};

    #[test]
    fn test_valid_create_request() {
        let request = CreateRelayerRequest {
            id: "test-relayer".to_string(),
            name: "Test Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Evm,
            policies: Some(RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
                gas_price_cap: Some(100),
                whitelist_receivers: None,
                eip1559_pricing: Some(true),
                private_transactions: None,
                min_balance: None,
                gas_limit_estimation: None,
            })),
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        // Convert to domain model and validate there
        let domain_relayer = Relayer::new(
            request.id,
            request.name,
            request.network,
            request.paused,
            request.network_type,
            request.policies,
            request.signer_id,
            request.notification_id,
            request.custom_rpc_urls,
        );
        assert!(domain_relayer.validate().is_ok());
    }

    #[test]
    fn test_invalid_create_request_empty_id() {
        let request = CreateRelayerRequest {
            id: "".to_string(),
            name: "Test Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Evm,
            policies: None,
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        // Convert to domain model and validate there - should fail due to empty ID
        let domain_relayer = Relayer::new(
            request.id,
            request.name,
            request.network,
            request.paused,
            request.network_type,
            request.policies,
            request.signer_id,
            request.notification_id,
            request.custom_rpc_urls,
        );
        assert!(domain_relayer.validate().is_err());
    }

    #[test]
    fn test_valid_update_request() {
        let request = UpdateRelayerRequest {
            name: Some("Updated Name".to_string()),
            paused: Some(true),
            network_type: None,
            policies: None,
            notification_id: Some("new-notification".to_string()),
            custom_rpc_urls: None,
        };

        // Should serialize/deserialize without errors
        let serialized = serde_json::to_string(&request).unwrap();
        let _deserialized: UpdateRelayerRequest = serde_json::from_str(&serialized).unwrap();
    }

    #[test]
    fn test_update_request_all_none() {
        let request = UpdateRelayerRequest {
            name: None,
            paused: None,
            network_type: None,
            policies: None,
            notification_id: None,
            custom_rpc_urls: None,
        };

        // Should serialize/deserialize without errors - all fields are optional
        let serialized = serde_json::to_string(&request).unwrap();
        let _deserialized: UpdateRelayerRequest = serde_json::from_str(&serialized).unwrap();
    }
}
