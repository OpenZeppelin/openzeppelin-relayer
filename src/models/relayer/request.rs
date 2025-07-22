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

use super::{
    Relayer, RelayerEvmPolicy, RelayerNetworkPolicy, RelayerNetworkType, RelayerSolanaPolicy,
    RelayerStellarPolicy, RpcConfig,
};
use crate::{models::error::ApiError, utils::generate_uuid};
use serde::{de, Deserialize, Deserializer, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

/// Request model for creating a new relayer
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateRelayerRequest {
    pub id: Option<String>,
    pub name: String,
    pub network: String,
    pub paused: bool,
    pub network_type: RelayerNetworkType,
    /// Policies without network_type tag - will be validated against the network_type field
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policies: Option<UpdateRelayerPolicyRequest>,
    pub signer_id: String,
    pub notification_id: Option<String>,
    pub custom_rpc_urls: Option<Vec<RpcConfig>>,
}

impl CreateRelayerRequest {
    /// Converts the policies field to domain RelayerNetworkPolicy using the network_type field
    pub fn to_domain_policies(&self) -> Result<Option<RelayerNetworkPolicy>, ApiError> {
        if let Some(policy_request) = &self.policies {
            Ok(Some(policy_request.to_domain_policy(self.network_type)?))
        } else {
            Ok(None)
        }
    }
}

/// Policy types for update requests - these don't require network_type tags
/// since they will be inferred from the existing relayer
#[derive(Debug, Clone, Serialize, PartialEq, ToSchema)]
#[serde(deny_unknown_fields)]
pub enum UpdateRelayerPolicyRequest {
    Evm(RelayerEvmPolicy),
    Solana(RelayerSolanaPolicy),
    Stellar(RelayerStellarPolicy),
}

impl<'de> Deserialize<'de> for UpdateRelayerPolicyRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value: Value = Value::deserialize(deserializer)?;

        if let Value::Object(obj) = &value {
            // Check for Solana-specific fields first (most distinctive)
            if obj.contains_key("fee_payment_strategy")
                || obj.contains_key("allowed_tokens")
                || obj.contains_key("allowed_programs")
                || obj.contains_key("max_signatures")
                || obj.contains_key("max_tx_data_size")
                || obj.contains_key("allowed_accounts")
                || obj.contains_key("disallowed_accounts")
                || obj.contains_key("max_allowed_fee_lamports")
                || obj.contains_key("swap_config")
                || obj.contains_key("fee_margin_percentage")
            {
                let policy: RelayerSolanaPolicy =
                    serde_json::from_value(value).map_err(de::Error::custom)?;
                return Ok(UpdateRelayerPolicyRequest::Solana(policy));
            }

            // Check for EVM-specific fields
            if obj.contains_key("gas_price_cap")
                || obj.contains_key("gas_limit_estimation")
                || obj.contains_key("whitelist_receivers")
                || obj.contains_key("eip1559_pricing")
                || obj.contains_key("private_transactions")
            {
                let policy: RelayerEvmPolicy =
                    serde_json::from_value(value).map_err(de::Error::custom)?;
                return Ok(UpdateRelayerPolicyRequest::Evm(policy));
            }

            // Check for Stellar-specific fields
            if obj.contains_key("max_fee") || obj.contains_key("timeout_seconds") {
                let policy: RelayerStellarPolicy =
                    serde_json::from_value(value).map_err(de::Error::custom)?;
                return Ok(UpdateRelayerPolicyRequest::Stellar(policy));
            }

            // If only min_balance is present, we can't determine the type automatically
            // Try each type and see which one deserializes successfully
            if let Ok(policy) = serde_json::from_value::<RelayerEvmPolicy>(value.clone()) {
                return Ok(UpdateRelayerPolicyRequest::Evm(policy));
            }
            if let Ok(policy) = serde_json::from_value::<RelayerSolanaPolicy>(value.clone()) {
                return Ok(UpdateRelayerPolicyRequest::Solana(policy));
            }
            if let Ok(policy) = serde_json::from_value::<RelayerStellarPolicy>(value) {
                return Ok(UpdateRelayerPolicyRequest::Stellar(policy));
            }
        }

        Err(de::Error::custom(
            "Unable to determine policy type from provided fields",
        ))
    }
}

impl UpdateRelayerPolicyRequest {
    /// Converts to domain RelayerNetworkPolicy using the provided network type
    pub fn to_domain_policy(
        &self,
        network_type: RelayerNetworkType,
    ) -> Result<RelayerNetworkPolicy, ApiError> {
        match (self, network_type) {
            (UpdateRelayerPolicyRequest::Evm(policy), RelayerNetworkType::Evm) => {
                Ok(RelayerNetworkPolicy::Evm(policy.clone()))
            }
            (UpdateRelayerPolicyRequest::Solana(policy), RelayerNetworkType::Solana) => {
                Ok(RelayerNetworkPolicy::Solana(policy.clone()))
            }
            (UpdateRelayerPolicyRequest::Stellar(policy), RelayerNetworkType::Stellar) => {
                Ok(RelayerNetworkPolicy::Stellar(policy.clone()))
            }
            _ => Err(ApiError::BadRequest(
                "Policy type does not match relayer network type".to_string(),
            )),
        }
    }
}

/// Request model for updating an existing relayer
/// All fields are optional to allow partial updates
/// Note: network and signer_id are not updateable after creation
///
/// ## Merge Patch Semantics for Policies
/// The policies field uses JSON Merge Patch (RFC 7396) semantics:
/// - Field not provided: no change to existing value
/// - Field with null value: remove/clear the field  
/// - Field with value: update the field
/// - Empty object {}: no changes to any policy fields
///
/// ## Merge Patch Semantics for notification_id
/// The notification_id field also uses JSON Merge Patch semantics:
/// - Field not provided: no change to existing value
/// - Field with null value: remove notification (set to None)
/// - Field with string value: set to that notification ID
///
/// ## Example Usage
///
/// ```json
/// // Update request examples:
/// {
///   "notification_id": null,           // Remove notification
///   "policies": { "min_balance": null } // Remove min_balance policy
/// }
///
/// {
///   "notification_id": "notif-123",    // Set notification
///   "policies": { "min_balance": "2000000000000000000" } // Update min_balance
/// }
///
/// {
///   "name": "Updated Name"             // Only update name, leave others unchanged
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateRelayerRequest {
    pub name: Option<String>,
    pub paused: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policies: Option<UpdateRelayerPolicyRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notification_id: Option<String>,
    pub custom_rpc_urls: Option<Vec<RpcConfig>>,
}

impl TryFrom<CreateRelayerRequest> for Relayer {
    type Error = ApiError;

    fn try_from(request: CreateRelayerRequest) -> Result<Self, Self::Error> {
        let id = request.id.clone().unwrap_or_else(|| generate_uuid());

        // Convert policies using the network_type from the request
        let policies = request.to_domain_policies()?;

        // Create domain relayer
        let relayer = Relayer::new(
            id,
            request.name,
            request.network,
            request.paused,
            request.network_type,
            policies,
            request.signer_id,
            request.notification_id,
            request.custom_rpc_urls,
        );

        // Validate using domain model validation logic
        relayer.validate().map_err(ApiError::from)?;

        Ok(relayer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::relayer::{RelayerEvmPolicy, RelayerSolanaPolicy};

    #[test]
    fn test_valid_create_request() {
        let request = CreateRelayerRequest {
            id: Some("test-relayer".to_string()),
            name: "Test Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Evm,
            policies: Some(UpdateRelayerPolicyRequest::Evm(RelayerEvmPolicy {
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
        let domain_relayer = Relayer::try_from(request);
        assert!(domain_relayer.is_ok());
    }

    #[test]
    fn test_invalid_create_request_empty_id() {
        let request = CreateRelayerRequest {
            id: Some("".to_string()),
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
        let domain_relayer = Relayer::try_from(request);
        assert!(domain_relayer.is_err());
    }

    #[test]
    fn test_create_request_policy_conversion() {
        // Test that policies are correctly converted from request type to domain type
        let request = CreateRelayerRequest {
            id: Some("test-relayer".to_string()),
            name: "Test Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Solana,
            policies: Some(UpdateRelayerPolicyRequest::Solana(RelayerSolanaPolicy {
                fee_payment_strategy: Some(
                    crate::models::relayer::RelayerSolanaFeePaymentStrategy::Relayer,
                ),
                min_balance: Some(1000000),
                allowed_tokens: None,
                allowed_programs: None,
                allowed_accounts: None,
                disallowed_accounts: None,
                max_signatures: None,
                max_tx_data_size: None,
                max_allowed_fee_lamports: None,
                swap_config: None,
                fee_margin_percentage: None,
            })),
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        // Test policy conversion
        let policies = request.to_domain_policies().unwrap();
        assert!(policies.is_some());

        if let Some(RelayerNetworkPolicy::Solana(solana_policy)) = policies {
            assert_eq!(solana_policy.min_balance, Some(1000000));
        } else {
            panic!("Expected Solana policy");
        }

        // Test full conversion to domain relayer
        let domain_relayer = Relayer::try_from(request);
        assert!(domain_relayer.is_ok());
    }

    #[test]
    fn test_create_request_wrong_policy_type() {
        // Test that providing wrong policy type for network type fails
        let request = CreateRelayerRequest {
            id: Some("test-relayer".to_string()),
            name: "Test Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Evm, // EVM network type
            policies: Some(UpdateRelayerPolicyRequest::Solana(
                RelayerSolanaPolicy::default(),
            )), // But Solana policy
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        // Should fail during policy conversion
        let result = request.to_domain_policies();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Policy type does not match relayer network type"));
    }

    #[test]
    fn test_create_request_json_deserialization() {
        // Test that JSON without network_type in policies deserializes correctly
        let json_input = r#"{
            "name": "Test Relayer",
            "network": "mainnet",
            "paused": false,
            "network_type": "evm",
            "signer_id": "test-signer",
            "policies": {
                "gas_price_cap": 100000000000,
                "eip1559_pricing": true,
                "min_balance": 1000000000000000000
            }
        }"#;

        let request: CreateRelayerRequest = serde_json::from_str(json_input).unwrap();
        assert_eq!(request.network_type, RelayerNetworkType::Evm);
        assert!(request.policies.is_some());

        // Test that it converts to domain model correctly
        let domain_relayer = Relayer::try_from(request).unwrap();
        assert_eq!(domain_relayer.network_type, RelayerNetworkType::Evm);

        if let Some(RelayerNetworkPolicy::Evm(evm_policy)) = domain_relayer.policies {
            assert_eq!(evm_policy.gas_price_cap, Some(100000000000));
            assert_eq!(evm_policy.eip1559_pricing, Some(true));
        } else {
            panic!("Expected EVM policy");
        }
    }

    #[test]
    fn test_valid_update_request() {
        let request = UpdateRelayerRequest {
            name: Some("Updated Name".to_string()),
            paused: Some(true),
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
            policies: None,
            notification_id: None,
            custom_rpc_urls: None,
        };

        // Should serialize/deserialize without errors - all fields are optional
        let serialized = serde_json::to_string(&request).unwrap();
        let _deserialized: UpdateRelayerRequest = serde_json::from_str(&serialized).unwrap();
    }

    #[test]
    fn test_update_request_policy_deserialization() {
        // Test EVM policy deserialization without network_type in user input
        let json_input = r#"{
            "name": "Updated Relayer",
            "policies": {
                "gas_price_cap": 100000000000,
                "eip1559_pricing": true
            }
        }"#;

        let request: UpdateRelayerRequest = serde_json::from_str(json_input).unwrap();
        assert!(request.policies.is_some());

        // Validation now happens automatically during deserialization
        if let Some(UpdateRelayerPolicyRequest::Evm(evm_policy)) = request.policies {
            assert_eq!(evm_policy.gas_price_cap, Some(100000000000));
            assert_eq!(evm_policy.eip1559_pricing, Some(true));
        } else {
            panic!("Expected EVM policy");
        }
    }

    #[test]
    fn test_update_request_policy_deserialization_solana() {
        // Test Solana policy deserialization without network_type in user input
        let json_input = r#"{
            "policies": {
                "fee_payment_strategy": "relayer",
                "min_balance": 1000000
            }
        }"#;

        let request: UpdateRelayerRequest = serde_json::from_str(json_input).unwrap();

        // Validation now happens automatically during deserialization
        if let Some(UpdateRelayerPolicyRequest::Solana(solana_policy)) = request.policies {
            assert_eq!(solana_policy.min_balance, Some(1000000));
        } else {
            panic!("Expected Solana policy");
        }
    }

    #[test]
    fn test_update_request_invalid_policy_format() {
        // Test that invalid policy format fails during JSON deserialization
        let invalid_json = r#"{
            "name": "Test",
            "policies": "invalid_not_an_object"
        }"#;

        // Should fail during deserialization since policies should be objects with valid fields
        let result = serde_json::from_str::<UpdateRelayerRequest>(invalid_json);
        assert!(result.is_err());
    }

    #[test]
    fn test_update_request_wrong_network_type() {
        // Test that EVM policy deserializes correctly as EVM type
        let json_input = r#"{
            "policies": {
                "gas_price_cap": 100000000000,
                "eip1559_pricing": true
            }
        }"#;

        let request: UpdateRelayerRequest = serde_json::from_str(json_input).unwrap();

        // Should correctly deserialize as EVM policy based on field detection
        if let Some(UpdateRelayerPolicyRequest::Evm(evm_policy)) = request.policies {
            assert_eq!(evm_policy.gas_price_cap, Some(100000000000));
            assert_eq!(evm_policy.eip1559_pricing, Some(true));
        } else {
            panic!("Expected EVM policy to be auto-detected");
        }
    }

    #[test]
    fn test_update_request_stellar_policy() {
        // Test Stellar policy deserialization
        let json_input = r#"{
            "policies": {
                "max_fee": 10000,
                "timeout_seconds": 300,
                "min_balance": 5000000
            }
        }"#;

        let request: UpdateRelayerRequest = serde_json::from_str(json_input).unwrap();

        // Should correctly deserialize as Stellar policy
        if let Some(UpdateRelayerPolicyRequest::Stellar(stellar_policy)) = request.policies {
            assert_eq!(stellar_policy.max_fee, Some(10000));
            assert_eq!(stellar_policy.timeout_seconds, Some(300));
            assert_eq!(stellar_policy.min_balance, Some(5000000));
        } else {
            panic!("Expected Stellar policy");
        }
    }

    #[test]
    fn test_notification_id_deserialization() {
        // Test valid notification_id deserialization
        let json_with_notification = r#"{
            "name": "Test Relayer",
            "notification_id": "notif-123"
        }"#;

        let request: UpdateRelayerRequest = serde_json::from_str(json_with_notification).unwrap();
        assert_eq!(request.notification_id, Some("notif-123".to_string()));

        // Test without notification_id
        let json_without_notification = r#"{
            "name": "Test Relayer"
        }"#;

        let request: UpdateRelayerRequest =
            serde_json::from_str(json_without_notification).unwrap();
        assert_eq!(request.notification_id, None);

        // Test invalid notification_id type should fail deserialization
        let invalid_json = r#"{
            "name": "Test Relayer",
            "notification_id": 123
        }"#;

        let result = serde_json::from_str::<UpdateRelayerRequest>(invalid_json);
        assert!(result.is_err());
    }

    #[test]
    fn test_comprehensive_update_request() {
        // Test a comprehensive update request with multiple fields
        let json_input = r#"{
            "name": "Updated Relayer",
            "paused": true,
            "notification_id": "new-notification-id",
            "policies": {
                "min_balance": "5000000000000000000",
                "gas_limit_estimation": false
            },
            "custom_rpc_urls": [
                {"url": "https://example.com", "weight": 100}
            ]
        }"#;

        let request: UpdateRelayerRequest = serde_json::from_str(json_input).unwrap();

        // Verify all fields are correctly deserialized
        assert_eq!(request.name, Some("Updated Relayer".to_string()));
        assert_eq!(request.paused, Some(true));
        assert_eq!(
            request.notification_id,
            Some("new-notification-id".to_string())
        );
        assert!(request.policies.is_some());
        assert!(request.custom_rpc_urls.is_some());

        if let Some(UpdateRelayerPolicyRequest::Evm(evm_policy)) = request.policies {
            assert_eq!(evm_policy.min_balance, Some(5000000000000000000));
            assert_eq!(evm_policy.gas_limit_estimation, Some(false));
        } else {
            panic!("Expected EVM policy");
        }
    }
}
