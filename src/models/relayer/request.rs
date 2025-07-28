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
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Request model for creating a new relayer
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateRelayerRequest {
    #[schema(nullable = false)]
    pub id: Option<String>,
    pub name: String,
    pub network: String,
    pub paused: bool,
    pub network_type: RelayerNetworkType,
    /// Policies - will be deserialized based on the network_type field
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub policies: Option<CreateRelayerPolicyRequest>,
    #[schema(nullable = false)]
    pub signer_id: String,
    #[schema(nullable = false)]
    pub notification_id: Option<String>,
    #[schema(nullable = false)]
    pub custom_rpc_urls: Option<Vec<RpcConfig>>,
}

/// Helper struct for deserializing CreateRelayerRequest with raw policies JSON
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRelayerRequestRaw {
    pub id: Option<String>,
    pub name: String,
    pub network: String,
    pub paused: bool,
    pub network_type: RelayerNetworkType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policies: Option<serde_json::Value>,
    pub signer_id: String,
    pub notification_id: Option<String>,
    pub custom_rpc_urls: Option<Vec<RpcConfig>>,
}

impl<'de> serde::Deserialize<'de> for CreateRelayerRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = CreateRelayerRequestRaw::deserialize(deserializer)?;

        // Convert policies based on network_type using the existing utility function
        let policies = if let Some(policies_value) = raw.policies {
            let domain_policy =
                deserialize_policy_for_network_type(&policies_value, raw.network_type)
                    .map_err(serde::de::Error::custom)?;

            // Convert from RelayerNetworkPolicy to CreateRelayerPolicyRequest
            let policy = match domain_policy {
                RelayerNetworkPolicy::Evm(evm_policy) => {
                    CreateRelayerPolicyRequest::Evm(evm_policy)
                }
                RelayerNetworkPolicy::Solana(solana_policy) => {
                    CreateRelayerPolicyRequest::Solana(solana_policy)
                }
                RelayerNetworkPolicy::Stellar(stellar_policy) => {
                    CreateRelayerPolicyRequest::Stellar(stellar_policy)
                }
            };
            Some(policy)
        } else {
            None
        };

        Ok(CreateRelayerRequest {
            id: raw.id,
            name: raw.name,
            network: raw.network,
            paused: raw.paused,
            network_type: raw.network_type,
            policies,
            signer_id: raw.signer_id,
            notification_id: raw.notification_id,
            custom_rpc_urls: raw.custom_rpc_urls,
        })
    }
}

/// Policy types for create requests - deserialized based on network_type from parent request
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(deny_unknown_fields)]
pub enum CreateRelayerPolicyRequest {
    Evm(RelayerEvmPolicy),
    Solana(RelayerSolanaPolicy),
    Stellar(RelayerStellarPolicy),
}

impl CreateRelayerPolicyRequest {
    /// Converts to domain RelayerNetworkPolicy using the provided network type
    pub fn to_domain_policy(
        &self,
        network_type: RelayerNetworkType,
    ) -> Result<RelayerNetworkPolicy, ApiError> {
        match (self, network_type) {
            (CreateRelayerPolicyRequest::Evm(policy), RelayerNetworkType::Evm) => {
                Ok(RelayerNetworkPolicy::Evm(policy.clone()))
            }
            (CreateRelayerPolicyRequest::Solana(policy), RelayerNetworkType::Solana) => {
                Ok(RelayerNetworkPolicy::Solana(policy.clone()))
            }
            (CreateRelayerPolicyRequest::Stellar(policy), RelayerNetworkType::Stellar) => {
                Ok(RelayerNetworkPolicy::Stellar(policy.clone()))
            }
            _ => Err(ApiError::BadRequest(
                "Policy type does not match relayer network type".to_string(),
            )),
        }
    }
}

/// Utility function to deserialize policy JSON for a specific network type
/// Used for update requests where we know the network type ahead of time
pub fn deserialize_policy_for_network_type(
    policies_value: &serde_json::Value,
    network_type: RelayerNetworkType,
) -> Result<RelayerNetworkPolicy, ApiError> {
    match network_type {
        RelayerNetworkType::Evm => {
            let evm_policy: RelayerEvmPolicy = serde_json::from_value(policies_value.clone())
                .map_err(|e| ApiError::BadRequest(format!("Invalid EVM policy: {}", e)))?;
            Ok(RelayerNetworkPolicy::Evm(evm_policy))
        }
        RelayerNetworkType::Solana => {
            let solana_policy: RelayerSolanaPolicy = serde_json::from_value(policies_value.clone())
                .map_err(|e| ApiError::BadRequest(format!("Invalid Solana policy: {}", e)))?;
            Ok(RelayerNetworkPolicy::Solana(solana_policy))
        }
        RelayerNetworkType::Stellar => {
            let stellar_policy: RelayerStellarPolicy =
                serde_json::from_value(policies_value.clone())
                    .map_err(|e| ApiError::BadRequest(format!("Invalid Stellar policy: {}", e)))?;
            Ok(RelayerNetworkPolicy::Stellar(stellar_policy))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateRelayerRequest {
    pub name: Option<String>,
    #[schema(nullable = false)]
    pub paused: Option<bool>,
    /// Raw policy JSON - will be validated against relayer's network type during application
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policies: Option<CreateRelayerPolicyRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notification_id: Option<String>,
    pub custom_rpc_urls: Option<Vec<RpcConfig>>,
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
pub struct UpdateRelayerRequestRaw {
    pub name: Option<String>,
    pub paused: Option<bool>,
    /// Raw policy JSON - will be validated against relayer's network type during application
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policies: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notification_id: Option<String>,
    pub custom_rpc_urls: Option<Vec<RpcConfig>>,
}

impl TryFrom<CreateRelayerRequest> for Relayer {
    type Error = ApiError;

    fn try_from(request: CreateRelayerRequest) -> Result<Self, Self::Error> {
        let id = request.id.clone().unwrap_or_else(generate_uuid);

        // Convert policies directly using the typed policy request
        let policies = if let Some(policy_request) = &request.policies {
            Some(policy_request.to_domain_policy(request.network_type)?)
        } else {
            None
        };

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
    use crate::models::relayer::{
        RelayerEvmPolicy, RelayerSolanaPolicy, RelayerStellarPolicy, SolanaFeePaymentStrategy,
    };

    #[test]
    fn test_valid_create_request() {
        let request = CreateRelayerRequest {
            id: Some("test-relayer".to_string()),
            name: "Test Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Evm,
            policies: Some(CreateRelayerPolicyRequest::Evm(RelayerEvmPolicy {
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
    fn test_valid_create_request_stellar() {
        let request = CreateRelayerRequest {
            id: Some("test-stellar-relayer".to_string()),
            name: "Test Stellar Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Stellar,
            policies: Some(CreateRelayerPolicyRequest::Stellar(RelayerStellarPolicy {
                min_balance: Some(20000000),
                max_fee: Some(100000),
                timeout_seconds: Some(30),
            })),
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        // Convert to domain model and validate there
        let domain_relayer = Relayer::try_from(request);
        assert!(domain_relayer.is_ok());

        // Verify the domain model has correct values
        let relayer = domain_relayer.unwrap();
        assert_eq!(relayer.network_type, RelayerNetworkType::Stellar);
        if let Some(RelayerNetworkPolicy::Stellar(stellar_policy)) = relayer.policies {
            assert_eq!(stellar_policy.min_balance, Some(20000000));
            assert_eq!(stellar_policy.max_fee, Some(100000));
            assert_eq!(stellar_policy.timeout_seconds, Some(30));
        } else {
            panic!("Expected Stellar policy");
        }
    }

    #[test]
    fn test_valid_create_request_solana() {
        let request = CreateRelayerRequest {
            id: Some("test-solana-relayer".to_string()),
            name: "Test Solana Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Solana,
            policies: Some(CreateRelayerPolicyRequest::Solana(RelayerSolanaPolicy {
                fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
                min_balance: Some(1000000),
                max_signatures: Some(5),
                allowed_tokens: None,
                allowed_programs: None,
                allowed_accounts: None,
                disallowed_accounts: None,
                max_tx_data_size: None,
                max_allowed_fee_lamports: None,
                swap_config: None,
                fee_margin_percentage: None,
            })),
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        // Convert to domain model and validate there
        let domain_relayer = Relayer::try_from(request);
        assert!(domain_relayer.is_ok());

        // Verify the domain model has correct values
        let relayer = domain_relayer.unwrap();
        assert_eq!(relayer.network_type, RelayerNetworkType::Solana);
        if let Some(RelayerNetworkPolicy::Solana(solana_policy)) = relayer.policies {
            assert_eq!(solana_policy.min_balance, Some(1000000));
            assert_eq!(solana_policy.max_signatures, Some(5));
            assert_eq!(
                solana_policy.fee_payment_strategy,
                Some(SolanaFeePaymentStrategy::Relayer)
            );
        } else {
            panic!("Expected Solana policy");
        }
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
            policies: Some(CreateRelayerPolicyRequest::Solana(RelayerSolanaPolicy {
                fee_payment_strategy: Some(
                    crate::models::relayer::SolanaFeePaymentStrategy::Relayer,
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
        if let Some(policy_request) = &request.policies {
            let policy = policy_request
                .to_domain_policy(request.network_type)
                .unwrap();
            if let RelayerNetworkPolicy::Solana(solana_policy) = policy {
                assert_eq!(solana_policy.min_balance, Some(1000000));
            } else {
                panic!("Expected Solana policy");
            }
        } else {
            panic!("Expected policies to be present");
        }

        // Test full conversion to domain relayer
        let domain_relayer = Relayer::try_from(request);
        assert!(domain_relayer.is_ok());
    }

    #[test]
    fn test_create_request_stellar_policy_conversion() {
        // Test that Stellar policies are correctly converted from request type to domain type
        let request = CreateRelayerRequest {
            id: Some("test-stellar-relayer".to_string()),
            name: "Test Stellar Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Stellar,
            policies: Some(CreateRelayerPolicyRequest::Stellar(RelayerStellarPolicy {
                min_balance: Some(50000000),
                max_fee: Some(150000),
                timeout_seconds: Some(60),
            })),
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        // Test policy conversion
        if let Some(policy_request) = &request.policies {
            let policy = policy_request
                .to_domain_policy(request.network_type)
                .unwrap();
            if let RelayerNetworkPolicy::Stellar(stellar_policy) = policy {
                assert_eq!(stellar_policy.min_balance, Some(50000000));
                assert_eq!(stellar_policy.max_fee, Some(150000));
                assert_eq!(stellar_policy.timeout_seconds, Some(60));
            } else {
                panic!("Expected Stellar policy");
            }
        } else {
            panic!("Expected policies to be present");
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
            policies: Some(CreateRelayerPolicyRequest::Solana(
                RelayerSolanaPolicy::default(),
            )), // But Solana policy
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        // Should fail during policy conversion - since the policy was auto-detected as Solana
        // but the network type is EVM, the conversion should fail
        if let Some(policy_request) = &request.policies {
            let result = policy_request.to_domain_policy(request.network_type);
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Policy type does not match relayer network type"));
        } else {
            panic!("Expected policies to be present");
        }
    }

    #[test]
    fn test_create_request_stellar_wrong_policy_type() {
        // Test that providing Stellar policy for EVM network type fails
        let request = CreateRelayerRequest {
            id: Some("test-relayer".to_string()),
            name: "Test Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Evm, // EVM network type
            policies: Some(CreateRelayerPolicyRequest::Stellar(
                RelayerStellarPolicy::default(),
            )), // But Stellar policy
            signer_id: "test-signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        // Should fail during policy conversion
        if let Some(policy_request) = &request.policies {
            let result = policy_request.to_domain_policy(request.network_type);
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Policy type does not match relayer network type"));
        } else {
            panic!("Expected policies to be present");
        }
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
    fn test_create_request_stellar_json_deserialization() {
        // Test that Stellar JSON deserializes correctly
        let json_input = r#"{
            "name": "Test Stellar Relayer",
            "network": "mainnet",
            "paused": false,
            "network_type": "stellar",
            "signer_id": "test-signer",
            "policies": {
                "min_balance": 25000000,
                "max_fee": 200000,
                "timeout_seconds": 45
            }
        }"#;

        let request: CreateRelayerRequest = serde_json::from_str(json_input).unwrap();
        assert_eq!(request.network_type, RelayerNetworkType::Stellar);
        assert!(request.policies.is_some());

        // Test that it converts to domain model correctly
        let domain_relayer = Relayer::try_from(request).unwrap();
        assert_eq!(domain_relayer.network_type, RelayerNetworkType::Stellar);

        if let Some(RelayerNetworkPolicy::Stellar(stellar_policy)) = domain_relayer.policies {
            assert_eq!(stellar_policy.min_balance, Some(25000000));
            assert_eq!(stellar_policy.max_fee, Some(200000));
            assert_eq!(stellar_policy.timeout_seconds, Some(45));
        } else {
            panic!("Expected Stellar policy");
        }
    }

    #[test]
    fn test_create_request_solana_json_deserialization() {
        // Test that Solana JSON deserializes correctly with complex policy
        let json_input = r#"{
            "name": "Test Solana Relayer",
            "network": "mainnet",
            "paused": false,
            "network_type": "solana",
            "signer_id": "test-signer",
            "policies": {
                "fee_payment_strategy": "relayer",
                "min_balance": 5000000,
                "max_signatures": 8,
                "max_tx_data_size": 1024,
                "fee_margin_percentage": 2.5
            }
        }"#;

        let request: CreateRelayerRequest = serde_json::from_str(json_input).unwrap();
        assert_eq!(request.network_type, RelayerNetworkType::Solana);
        assert!(request.policies.is_some());

        // Test that it converts to domain model correctly
        let domain_relayer = Relayer::try_from(request).unwrap();
        assert_eq!(domain_relayer.network_type, RelayerNetworkType::Solana);

        if let Some(RelayerNetworkPolicy::Solana(solana_policy)) = domain_relayer.policies {
            assert_eq!(solana_policy.min_balance, Some(5000000));
            assert_eq!(solana_policy.max_signatures, Some(8));
            assert_eq!(solana_policy.max_tx_data_size, Some(1024));
            assert_eq!(solana_policy.fee_margin_percentage, Some(2.5));
            assert_eq!(
                solana_policy.fee_payment_strategy,
                Some(SolanaFeePaymentStrategy::Relayer)
            );
        } else {
            panic!("Expected Solana policy");
        }
    }

    #[test]
    fn test_valid_update_request() {
        let request = UpdateRelayerRequestRaw {
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
        let request = UpdateRelayerRequestRaw {
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

        let request: UpdateRelayerRequestRaw = serde_json::from_str(json_input).unwrap();
        assert!(request.policies.is_some());

        // Validation happens during domain conversion based on network type
        // Test with the utility function
        if let Some(policies_json) = &request.policies {
            let network_policy =
                deserialize_policy_for_network_type(policies_json, RelayerNetworkType::Evm)
                    .unwrap();
            if let RelayerNetworkPolicy::Evm(evm_policy) = network_policy {
                assert_eq!(evm_policy.gas_price_cap, Some(100000000000));
                assert_eq!(evm_policy.eip1559_pricing, Some(true));
            } else {
                panic!("Expected EVM policy");
            }
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

        let request: UpdateRelayerRequestRaw = serde_json::from_str(json_input).unwrap();

        // Validation happens during domain conversion based on network type
        // Test with the utility function for Solana
        if let Some(policies_json) = &request.policies {
            let network_policy =
                deserialize_policy_for_network_type(policies_json, RelayerNetworkType::Solana)
                    .unwrap();
            if let RelayerNetworkPolicy::Solana(solana_policy) = network_policy {
                assert_eq!(solana_policy.min_balance, Some(1000000));
            } else {
                panic!("Expected Solana policy");
            }
        }
    }

    #[test]
    fn test_update_request_policy_deserialization_stellar() {
        // Test Stellar policy deserialization without network_type in user input
        let json_input = r#"{
            "policies": {
                "max_fee": 75000,
                "timeout_seconds": 120,
                "min_balance": 15000000
            }
        }"#;

        let request: UpdateRelayerRequestRaw = serde_json::from_str(json_input).unwrap();

        // Validation happens during domain conversion based on network type
        // Test with the utility function for Stellar
        if let Some(policies_json) = &request.policies {
            let network_policy =
                deserialize_policy_for_network_type(policies_json, RelayerNetworkType::Stellar)
                    .unwrap();
            if let RelayerNetworkPolicy::Stellar(stellar_policy) = network_policy {
                assert_eq!(stellar_policy.max_fee, Some(75000));
                assert_eq!(stellar_policy.timeout_seconds, Some(120));
                assert_eq!(stellar_policy.min_balance, Some(15000000));
            } else {
                panic!("Expected Stellar policy");
            }
        }
    }

    #[test]
    fn test_update_request_invalid_policy_format() {
        // Test that invalid policy format fails during validation with utility function
        let valid_json = r#"{
            "name": "Test",
            "policies": "invalid_not_an_object"
        }"#;

        let request: UpdateRelayerRequestRaw = serde_json::from_str(valid_json).unwrap();

        // Should fail when trying to validate the policy against a network type
        if let Some(policies_json) = &request.policies {
            let result =
                deserialize_policy_for_network_type(policies_json, RelayerNetworkType::Evm);
            assert!(result.is_err());
        }
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

        let request: UpdateRelayerRequestRaw = serde_json::from_str(json_input).unwrap();

        // Should correctly deserialize as raw JSON - validation happens during domain conversion
        assert!(request.policies.is_some());
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

        let request: UpdateRelayerRequestRaw = serde_json::from_str(json_input).unwrap();

        // Should correctly deserialize as raw JSON - validation happens during domain conversion
        assert!(request.policies.is_some());
    }

    #[test]
    fn test_update_request_stellar_policy_partial() {
        // Test Stellar policy with only some fields (partial update)
        let json_input = r#"{
            "policies": {
                "max_fee": 50000
            }
        }"#;

        let request: UpdateRelayerRequestRaw = serde_json::from_str(json_input).unwrap();

        // Should correctly deserialize as raw JSON
        assert!(request.policies.is_some());

        // Test domain conversion with utility function
        if let Some(policies_json) = &request.policies {
            let network_policy =
                deserialize_policy_for_network_type(policies_json, RelayerNetworkType::Stellar)
                    .unwrap();
            if let RelayerNetworkPolicy::Stellar(stellar_policy) = network_policy {
                assert_eq!(stellar_policy.max_fee, Some(50000));
                assert_eq!(stellar_policy.timeout_seconds, None);
                assert_eq!(stellar_policy.min_balance, None);
            } else {
                panic!("Expected Stellar policy");
            }
        }
    }

    #[test]
    fn test_notification_id_deserialization() {
        // Test valid notification_id deserialization
        let json_with_notification = r#"{
            "name": "Test Relayer",
            "notification_id": "notif-123"
        }"#;

        let request: UpdateRelayerRequestRaw =
            serde_json::from_str(json_with_notification).unwrap();
        assert_eq!(request.notification_id, Some("notif-123".to_string()));

        // Test without notification_id
        let json_without_notification = r#"{
            "name": "Test Relayer"
        }"#;

        let request: UpdateRelayerRequestRaw =
            serde_json::from_str(json_without_notification).unwrap();
        assert_eq!(request.notification_id, None);

        // Test invalid notification_id type should fail deserialization
        let invalid_json = r#"{
            "name": "Test Relayer",
            "notification_id": 123
        }"#;

        let result = serde_json::from_str::<UpdateRelayerRequestRaw>(invalid_json);
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

        let request: UpdateRelayerRequestRaw = serde_json::from_str(json_input).unwrap();

        // Verify all fields are correctly deserialized
        assert_eq!(request.name, Some("Updated Relayer".to_string()));
        assert_eq!(request.paused, Some(true));
        assert_eq!(
            request.notification_id,
            Some("new-notification-id".to_string())
        );
        assert!(request.policies.is_some());
        assert!(request.custom_rpc_urls.is_some());

        // Policies are now raw JSON - validation happens during domain conversion
        if let Some(policies_json) = &request.policies {
            // Just verify it's a JSON object with expected fields
            assert!(policies_json.get("min_balance").is_some());
            assert!(policies_json.get("gas_limit_estimation").is_some());
        } else {
            panic!("Expected policies");
        }
    }

    #[test]
    fn test_comprehensive_update_request_stellar() {
        // Test a comprehensive Stellar update request
        let json_input = r#"{
            "name": "Updated Stellar Relayer",
            "paused": false,
            "notification_id": "stellar-notification",
            "policies": {
                "min_balance": 30000000,
                "max_fee": 250000,
                "timeout_seconds": 90
            },
            "custom_rpc_urls": [
                {"url": "https://stellar-node.example.com", "weight": 100}
            ]
        }"#;

        let request: UpdateRelayerRequestRaw = serde_json::from_str(json_input).unwrap();

        // Verify all fields are correctly deserialized
        assert_eq!(request.name, Some("Updated Stellar Relayer".to_string()));
        assert_eq!(request.paused, Some(false));
        assert_eq!(
            request.notification_id,
            Some("stellar-notification".to_string())
        );
        assert!(request.policies.is_some());
        assert!(request.custom_rpc_urls.is_some());

        // Test domain conversion
        if let Some(policies_json) = &request.policies {
            let network_policy =
                deserialize_policy_for_network_type(policies_json, RelayerNetworkType::Stellar)
                    .unwrap();
            if let RelayerNetworkPolicy::Stellar(stellar_policy) = network_policy {
                assert_eq!(stellar_policy.min_balance, Some(30000000));
                assert_eq!(stellar_policy.max_fee, Some(250000));
                assert_eq!(stellar_policy.timeout_seconds, Some(90));
            } else {
                panic!("Expected Stellar policy");
            }
        }
    }

    #[test]
    fn test_create_request_network_type_based_policy_deserialization() {
        // Test that policies are correctly deserialized based on network_type
        // EVM network with EVM policy fields
        let evm_json = r#"{
            "name": "EVM Relayer",
            "network": "mainnet",
            "paused": false,
            "network_type": "evm",
            "signer_id": "test-signer",
            "policies": {
                "gas_price_cap": 50000000000,
                "eip1559_pricing": true,
                "min_balance": "1000000000000000000"
            }
        }"#;

        let evm_request: CreateRelayerRequest = serde_json::from_str(evm_json).unwrap();
        assert_eq!(evm_request.network_type, RelayerNetworkType::Evm);

        if let Some(CreateRelayerPolicyRequest::Evm(evm_policy)) = evm_request.policies {
            assert_eq!(evm_policy.gas_price_cap, Some(50000000000));
            assert_eq!(evm_policy.eip1559_pricing, Some(true));
            assert_eq!(evm_policy.min_balance, Some(1000000000000000000));
        } else {
            panic!("Expected EVM policy");
        }

        // Solana network with Solana policy fields
        let solana_json = r#"{
            "name": "Solana Relayer",
            "network": "mainnet",
            "paused": false,
            "network_type": "solana",
            "signer_id": "test-signer",
            "policies": {
                "fee_payment_strategy": "relayer",
                "min_balance": 5000000,
                "max_signatures": 10
            }
        }"#;

        let solana_request: CreateRelayerRequest = serde_json::from_str(solana_json).unwrap();
        assert_eq!(solana_request.network_type, RelayerNetworkType::Solana);

        if let Some(CreateRelayerPolicyRequest::Solana(solana_policy)) = solana_request.policies {
            assert_eq!(solana_policy.min_balance, Some(5000000));
            assert_eq!(solana_policy.max_signatures, Some(10));
        } else {
            panic!("Expected Solana policy");
        }

        // Stellar network with Stellar policy fields
        let stellar_json = r#"{
            "name": "Stellar Relayer",
            "network": "mainnet",
            "paused": false,
            "network_type": "stellar",
            "signer_id": "test-signer",
            "policies": {
                "min_balance": 40000000,
                "max_fee": 300000,
                "timeout_seconds": 180
            }
        }"#;

        let stellar_request: CreateRelayerRequest = serde_json::from_str(stellar_json).unwrap();
        assert_eq!(stellar_request.network_type, RelayerNetworkType::Stellar);

        if let Some(CreateRelayerPolicyRequest::Stellar(stellar_policy)) = stellar_request.policies
        {
            assert_eq!(stellar_policy.min_balance, Some(40000000));
            assert_eq!(stellar_policy.max_fee, Some(300000));
            assert_eq!(stellar_policy.timeout_seconds, Some(180));
        } else {
            panic!("Expected Stellar policy");
        }

        // Test that wrong policy fields for network type fails
        let invalid_json = r#"{
            "name": "Invalid Relayer",
            "network": "mainnet",
            "paused": false,
            "network_type": "evm",
            "signer_id": "test-signer",
            "policies": {
                "fee_payment_strategy": "relayer"
            }
        }"#;

        let result = serde_json::from_str::<CreateRelayerRequest>(invalid_json);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown field"));
    }

    #[test]
    fn test_create_request_invalid_stellar_policy_fields() {
        // Test that invalid Stellar policy fields fail during deserialization
        let invalid_json = r#"{
            "name": "Invalid Stellar Relayer",
            "network": "mainnet",
            "paused": false,
            "network_type": "stellar",
            "signer_id": "test-signer",
            "policies": {
                "gas_price_cap": 100000000000
            }
        }"#;

        let result = serde_json::from_str::<CreateRelayerRequest>(invalid_json);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown field"));
    }

    #[test]
    fn test_create_request_empty_policies() {
        // Test create request with empty policies for each network type
        let evm_json = r#"{
            "name": "EVM Relayer No Policies",
            "network": "mainnet",
            "paused": false,
            "network_type": "evm",
            "signer_id": "test-signer"
        }"#;

        let evm_request: CreateRelayerRequest = serde_json::from_str(evm_json).unwrap();
        assert_eq!(evm_request.network_type, RelayerNetworkType::Evm);
        assert!(evm_request.policies.is_none());

        let stellar_json = r#"{
            "name": "Stellar Relayer No Policies",
            "network": "mainnet",
            "paused": false,
            "network_type": "stellar",
            "signer_id": "test-signer"
        }"#;

        let stellar_request: CreateRelayerRequest = serde_json::from_str(stellar_json).unwrap();
        assert_eq!(stellar_request.network_type, RelayerNetworkType::Stellar);
        assert!(stellar_request.policies.is_none());

        let solana_json = r#"{
            "name": "Solana Relayer No Policies",
            "network": "mainnet",
            "paused": false,
            "network_type": "solana",
            "signer_id": "test-signer"
        }"#;

        let solana_request: CreateRelayerRequest = serde_json::from_str(solana_json).unwrap();
        assert_eq!(solana_request.network_type, RelayerNetworkType::Solana);
        assert!(solana_request.policies.is_none());
    }

    #[test]
    fn test_deserialize_policy_utility_function_all_networks() {
        // Test the utility function with all network types

        // EVM policy
        let evm_json = serde_json::json!({
            "gas_price_cap": "75000000000",
            "private_transactions": false,
            "min_balance": "2000000000000000000"
        });

        let evm_policy =
            deserialize_policy_for_network_type(&evm_json, RelayerNetworkType::Evm).unwrap();
        if let RelayerNetworkPolicy::Evm(policy) = evm_policy {
            assert_eq!(policy.gas_price_cap, Some(75000000000));
            assert_eq!(policy.private_transactions, Some(false));
            assert_eq!(policy.min_balance, Some(2000000000000000000));
        } else {
            panic!("Expected EVM policy");
        }

        // Solana policy
        let solana_json = serde_json::json!({
            "fee_payment_strategy": "user",
            "max_tx_data_size": 512,
            "fee_margin_percentage": 1.5
        });

        let solana_policy =
            deserialize_policy_for_network_type(&solana_json, RelayerNetworkType::Solana).unwrap();
        if let RelayerNetworkPolicy::Solana(policy) = solana_policy {
            assert_eq!(
                policy.fee_payment_strategy,
                Some(SolanaFeePaymentStrategy::User)
            );
            assert_eq!(policy.max_tx_data_size, Some(512));
            assert_eq!(policy.fee_margin_percentage, Some(1.5));
        } else {
            panic!("Expected Solana policy");
        }

        // Stellar policy
        let stellar_json = serde_json::json!({
            "max_fee": 125000,
            "timeout_seconds": 240
        });

        let stellar_policy =
            deserialize_policy_for_network_type(&stellar_json, RelayerNetworkType::Stellar)
                .unwrap();
        if let RelayerNetworkPolicy::Stellar(policy) = stellar_policy {
            assert_eq!(policy.max_fee, Some(125000));
            assert_eq!(policy.timeout_seconds, Some(240));
            assert_eq!(policy.min_balance, None);
        } else {
            panic!("Expected Stellar policy");
        }
    }
}
