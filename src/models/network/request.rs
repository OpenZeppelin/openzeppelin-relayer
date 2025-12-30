//! API request models and validation for network endpoints.
//!
//! This module handles incoming HTTP requests for network operations, providing:
//!
//! - **Request Models**: Structures for updating network configurations via API
//! - **Input Validation**: Sanitization and validation of user-provided data
//!
//! Serves as the entry point for network data from external clients, ensuring
//! all input is properly validated before reaching the core business logic.

use crate::models::{ApiError, RpcConfig};
use serde::{de::Error as DeError, Deserialize, Deserializer, Serialize};
use serde_json;
use utoipa::ToSchema;

/// Schema-only type representing a flexible RPC URL entry.
/// Used for OpenAPI documentation to show that rpc_urls can accept
/// either strings or RpcConfig objects.
///
/// This is NOT used for actual deserialization - the custom deserializer
/// handles the conversion. This type exists solely for schema generation.
#[derive(Serialize, ToSchema)]
#[serde(untagged)]
#[schema(as = RpcUrlEntry)]
#[allow(dead_code)] // Only used for schema generation
pub enum RpcUrlEntry {
    /// Simple string format (e.g., "https://rpc.example.com")
    /// Defaults to weight 100.
    String(String),
    /// Extended object format with explicit weight
    Config(RpcConfig),
}

/// Custom deserializer for rpc_urls that supports:
/// - Simple format: array of strings (e.g., ["https://rpc.example.com"])
/// - Extended format: array of RpcConfig objects (e.g., [{"url": "https://rpc.example.com", "weight": 100}])
/// - Mixed format: array containing both strings and RpcConfig objects
///
/// When a string URL is provided, it defaults to weight 100.
fn deserialize_rpc_urls<'de, D>(deserializer: D) -> Result<Option<Vec<RpcConfig>>, D::Error>
where
    D: Deserializer<'de>,
{
    // First, deserialize as a generic Value to check what we have
    let value: Option<serde_json::Value> = Option::deserialize(deserializer)?;

    match value {
        None => Ok(None),
        Some(serde_json::Value::Array(arr)) => {
            let mut configs = Vec::new();
            for item in arr {
                match item {
                    serde_json::Value::String(url) => {
                        // Simple format: string -> convert to RpcConfig with default weight (100)
                        configs.push(RpcConfig::new(url));
                    }
                    serde_json::Value::Object(obj) => {
                        // Extended format: object -> deserialize as RpcConfig
                        let config: RpcConfig =
                            serde_json::from_value(serde_json::Value::Object(obj))
                                .map_err(DeError::custom)?;
                        configs.push(config);
                    }
                    _ => {
                        return Err(DeError::custom(
                            "rpc_urls must be an array of strings or RpcConfig objects",
                        ));
                    }
                }
            }
            Ok(Some(configs))
        }
        Some(_) => Err(DeError::custom(
            "rpc_urls must be an array of strings or RpcConfig objects",
        )),
    }
}

/// Request structure for updating a network configuration.
/// Currently supports updating RPC URLs only. Can be extended to support other fields.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateNetworkRequest {
    /// List of RPC endpoint configurations for connecting to the network.
    /// Supports multiple formats:
    /// - Array of strings: `["https://rpc.example.com"]` (defaults to weight 100)
    /// - Array of RpcConfig objects: `[{"url": "https://rpc.example.com", "weight": 100}]`
    /// - Mixed array: `["https://rpc1.com", {"url": "https://rpc2.com", "weight": 200}]`
    /// Must be non-empty and contain valid HTTP/HTTPS URLs if provided.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_rpc_urls"
    )]
    #[schema(
        nullable = false,
        example = json!([{"url": "https://rpc.example.com", "weight": 100}]),
        value_type = Vec<RpcUrlEntry>
    )]
    pub rpc_urls: Option<Vec<RpcConfig>>,
}

impl UpdateNetworkRequest {
    /// Validates the request data.
    ///
    /// # Returns
    /// - `Ok(())` if the request is valid
    /// - `Err(ApiError)` if validation fails
    pub fn validate(&self) -> Result<(), ApiError> {
        // Validate RPC URLs if provided
        if let Some(ref rpc_urls) = self.rpc_urls {
            // Check that rpc_urls is not empty
            if rpc_urls.is_empty() {
                return Err(ApiError::BadRequest(
                    "rpc_urls must contain at least one RPC endpoint".to_string(),
                ));
            }

            // Validate all RPC URLs
            RpcConfig::validate_list(rpc_urls).map_err(|e| {
                ApiError::BadRequest(format!("Invalid RPC URL configuration: {}", e))
            })?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_update_network_request_validation_empty_rpc_urls() {
        let request = UpdateNetworkRequest {
            rpc_urls: Some(vec![]),
        };
        assert!(request.validate().is_err());
    }

    #[test]
    fn test_update_network_request_validation_valid() {
        let request = UpdateNetworkRequest {
            rpc_urls: Some(vec![RpcConfig::new("https://rpc.example.com".to_string())]),
        };
        assert!(request.validate().is_ok());
    }

    #[test]
    fn test_update_network_request_validation_invalid_url() {
        let request = UpdateNetworkRequest {
            rpc_urls: Some(vec![RpcConfig::new("ftp://invalid.com".to_string())]),
        };
        assert!(request.validate().is_err());
    }

    #[test]
    fn test_update_network_request_validation_none_rpc_urls() {
        let request = UpdateNetworkRequest { rpc_urls: None };
        assert!(request.validate().is_ok());
    }

    #[test]
    fn test_deserialize_rpc_urls_simple_format() {
        let json = r#"{"rpc_urls": ["https://rpc1.com", "https://rpc2.com"]}"#;
        let request: UpdateNetworkRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.rpc_urls.as_ref().unwrap().len(), 2);
        assert_eq!(
            request.rpc_urls.as_ref().unwrap()[0].url,
            "https://rpc1.com"
        );
        assert_eq!(request.rpc_urls.as_ref().unwrap()[0].weight, 100u8); // Default weight
        assert_eq!(
            request.rpc_urls.as_ref().unwrap()[1].url,
            "https://rpc2.com"
        );
        assert_eq!(request.rpc_urls.as_ref().unwrap()[1].weight, 100u8); // Default weight
    }

    #[test]
    fn test_deserialize_rpc_urls_extended_format() {
        let json = r#"{"rpc_urls": [{"url": "https://rpc1.com", "weight": 50}, {"url": "https://rpc2.com", "weight": 75}]}"#;
        let request: UpdateNetworkRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.rpc_urls.as_ref().unwrap().len(), 2);
        assert_eq!(
            request.rpc_urls.as_ref().unwrap()[0].url,
            "https://rpc1.com"
        );
        assert_eq!(request.rpc_urls.as_ref().unwrap()[0].weight, 50u8);
        assert_eq!(
            request.rpc_urls.as_ref().unwrap()[1].url,
            "https://rpc2.com"
        );
        assert_eq!(request.rpc_urls.as_ref().unwrap()[1].weight, 75u8);
    }

    #[test]
    fn test_deserialize_rpc_urls_mixed_format() {
        let json =
            r#"{"rpc_urls": ["https://rpc1.com", {"url": "https://rpc2.com", "weight": 50}]}"#;
        let request: UpdateNetworkRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.rpc_urls.as_ref().unwrap().len(), 2);
        assert_eq!(
            request.rpc_urls.as_ref().unwrap()[0].url,
            "https://rpc1.com"
        );
        assert_eq!(request.rpc_urls.as_ref().unwrap()[0].weight, 100u8); // Default weight for string
        assert_eq!(
            request.rpc_urls.as_ref().unwrap()[1].url,
            "https://rpc2.com"
        );
        assert_eq!(request.rpc_urls.as_ref().unwrap()[1].weight, 50u8); // Explicit weight from object
    }

    #[test]
    fn test_deserialize_rpc_urls_none() {
        let json = r#"{}"#;
        let request: UpdateNetworkRequest = serde_json::from_str(json).unwrap();
        assert!(request.rpc_urls.is_none());
    }

    #[test]
    fn test_deserialize_rpc_urls_invalid_format() {
        let json = r#"{"rpc_urls": [123, 456]}"#;
        let result: Result<UpdateNetworkRequest, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}
