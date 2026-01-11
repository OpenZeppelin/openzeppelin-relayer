//! Configuration for RPC endpoints.
//!
//! This module provides configuration structures for RPC endpoints,
//! including URLs and weights for load balancing.

use crate::constants::DEFAULT_RPC_WEIGHT;
use eyre::eyre;
use serde::{
    de::Error as DeError, ser::SerializeStruct, Deserialize, Deserializer, Serialize, Serializer,
};
use std::hash::{Hash, Hasher};
use thiserror::Error;
use utoipa::ToSchema;

#[derive(Debug, Error, PartialEq)]
pub enum RpcConfigError {
    #[error("Invalid weight: {value}. Must be between 0 and 100.")]
    InvalidWeight { value: u8 },
}

/// Returns the default RPC weight for OpenAPI schema generation.
fn default_rpc_weight() -> u8 {
    DEFAULT_RPC_WEIGHT
}

/// Configuration for an RPC endpoint.
///
/// This struct contains only persistent configuration (URL and weight).
/// Health metadata (failures, pause state) is managed separately via `RpcHealthStore`.
#[derive(Clone, Debug, PartialEq, Eq, Default, ToSchema)]
#[schema(example = json!({"url": "https://rpc.example.com", "weight": 100}))]
pub struct RpcConfig {
    /// The RPC endpoint URL.
    pub url: String,
    /// The weight of this endpoint in the weighted round-robin selection.
    /// Defaults to [`DEFAULT_RPC_WEIGHT`]. Should be between 0 and 100.
    #[schema(default = default_rpc_weight, minimum = 0, maximum = 100)]
    pub weight: u8,
}

impl Hash for RpcConfig {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.url.hash(state);
        self.weight.hash(state);
    }
}

impl Serialize for RpcConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RpcConfig", 2)?;
        state.serialize_field("url", &self.url)?;
        state.serialize_field("weight", &self.weight)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for RpcConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RpcConfigHelper {
            url: String,
            weight: Option<u8>,
        }

        let helper = RpcConfigHelper::deserialize(deserializer)?;
        Ok(RpcConfig {
            url: helper.url,
            weight: helper.weight.unwrap_or(DEFAULT_RPC_WEIGHT),
        })
    }
}

impl RpcConfig {
    /// Creates a new RPC configuration with the given URL and default weight (DEFAULT_RPC_WEIGHT).
    ///
    /// # Arguments
    ///
    /// * `url` - A string slice that holds the URL of the RPC endpoint.
    pub fn new(url: String) -> Self {
        Self {
            url,
            weight: DEFAULT_RPC_WEIGHT,
        }
    }

    /// Creates a new RPC configuration with the given URL and weight.
    ///
    /// # Arguments
    ///
    /// * `url` - A string that holds the URL of the RPC endpoint.
    /// * `weight` - A u8 value representing the weight of the endpoint. Must be between 0 and 100 (inclusive).
    ///
    /// # Returns
    ///
    /// * `Ok(RpcConfig)` if the weight is valid.
    /// * `Err(RpcConfigError::InvalidWeight)` if the weight is greater than 100.
    pub fn with_weight(url: String, weight: u8) -> Result<Self, RpcConfigError> {
        if weight > 100 {
            return Err(RpcConfigError::InvalidWeight { value: weight });
        }
        Ok(Self { url, weight })
    }

    /// Gets the weight of this RPC endpoint.
    ///
    /// # Returns
    ///
    /// * `u8` - The weight of the RPC endpoint.
    pub fn get_weight(&self) -> u8 {
        self.weight
    }

    /// Validates that a URL has an HTTP or HTTPS scheme.
    /// Helper function, hence private.
    fn validate_url_scheme(url: &str) -> Result<(), eyre::Report> {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(eyre!(
                "Invalid URL scheme for {}: Only HTTP and HTTPS are supported",
                url
            ));
        }
        Ok(())
    }

    /// Validates all URLs in a slice of RpcConfig objects.
    ///
    /// # Arguments
    /// * `configs` - A slice of RpcConfig objects
    ///
    /// # Returns
    /// * `Result<()>` - Ok if all URLs have valid schemes, error on first invalid URL
    ///
    /// # Examples
    /// ```rust, ignore
    /// use crate::models::RpcConfig;
    ///
    /// let configs = vec![
    ///     RpcConfig::new("https://api.example.com".to_string()),
    ///     RpcConfig::new("http://localhost:8545".to_string()),
    /// ];
    /// assert!(RpcConfig::validate_list(&configs).is_ok());
    /// ```
    pub fn validate_list(configs: &[RpcConfig]) -> Result<(), eyre::Report> {
        for config in configs {
            // Call the helper function using Self to refer to the type for associated functions
            Self::validate_url_scheme(&config.url)?;
        }
        Ok(())
    }
}

/// Custom deserializer for `Option<Vec<RpcConfig>>` that supports multiple input formats.
///
/// This function is designed to be used with `#[serde(deserialize_with = "...")]` and supports:
///
/// - **Simple format**: Array of strings, e.g., `["https://rpc1.com", "https://rpc2.com"]`
///   Each string is converted to an `RpcConfig` with default weight (100).
///
/// - **Extended format**: Array of objects, e.g., `[{"url": "https://rpc.com", "weight": 50}]`
///   Each object is deserialized directly as an `RpcConfig`.
///
/// - **Mixed format**: Array containing both strings and objects
///   e.g., `["https://rpc1.com", {"url": "https://rpc2.com", "weight": 50}]`
///
/// # Example Usage
///
/// ```rust,ignore
/// use serde::Deserialize;
/// use crate::models::RpcConfig;
///
/// #[derive(Deserialize)]
/// struct MyConfig {
///     #[serde(default, deserialize_with = "crate::models::deserialize_rpc_urls")]
///     rpc_urls: Option<Vec<RpcConfig>>,
/// }
/// ```
pub fn deserialize_rpc_urls<'de, D>(deserializer: D) -> Result<Option<Vec<RpcConfig>>, D::Error>
where
    D: Deserializer<'de>,
{
    // First, deserialize as a generic Value to check what we have
    let value: Option<serde_json::Value> = Option::deserialize(deserializer)?;

    match value {
        None => Ok(None),
        Some(serde_json::Value::Array(arr)) => {
            let mut configs = Vec::with_capacity(arr.len());
            for item in arr {
                match item {
                    serde_json::Value::String(url) => {
                        // Simple format: string -> convert to RpcConfig with default weight
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::DEFAULT_RPC_WEIGHT;

    #[test]
    fn test_new_creates_config_with_default_weight() {
        let url = "https://example.com".to_string();
        let config = RpcConfig::new(url.clone());

        assert_eq!(config.url, url);
        assert_eq!(config.weight, DEFAULT_RPC_WEIGHT);
    }

    #[test]
    fn test_with_weight_creates_config_with_custom_weight() {
        let url = "https://example.com".to_string();
        let weight: u8 = 5;
        let result = RpcConfig::with_weight(url.clone(), weight);
        assert!(result.is_ok());

        let config = result.unwrap();
        assert_eq!(config.url, url);
        assert_eq!(config.weight, weight);
    }

    #[test]
    fn test_get_weight_returns_weight_value() {
        let url = "https://example.com".to_string();
        let weight: u8 = 10;
        let config = RpcConfig {
            url,
            weight,
            ..Default::default()
        };

        assert_eq!(config.get_weight(), weight);
    }

    #[test]
    fn test_equality_of_configs() {
        let url = "https://example.com".to_string();
        let config1 = RpcConfig::new(url.clone());
        let config2 = RpcConfig::new(url.clone()); // Same as config1
        let config3 = RpcConfig::with_weight(url.clone(), 5u8).unwrap(); // Different weight
        let config4 =
            RpcConfig::with_weight("https://different.com".to_string(), DEFAULT_RPC_WEIGHT)
                .unwrap(); // Different URL

        assert_eq!(config1, config2);
        assert_ne!(config1, config3);
        assert_ne!(config1, config4);
    }

    // Tests for URL validation
    #[test]
    fn test_validate_url_scheme_with_http() {
        let result = RpcConfig::validate_url_scheme("http://example.com");
        assert!(result.is_ok(), "HTTP URL should be valid");
    }

    #[test]
    fn test_validate_url_scheme_with_https() {
        let result = RpcConfig::validate_url_scheme("https://secure.example.com");
        assert!(result.is_ok(), "HTTPS URL should be valid");
    }

    #[test]
    fn test_validate_url_scheme_with_query_params() {
        let result =
            RpcConfig::validate_url_scheme("https://example.com/api?param=value&other=123");
        assert!(result.is_ok(), "URL with query parameters should be valid");
    }

    #[test]
    fn test_validate_url_scheme_with_port() {
        let result = RpcConfig::validate_url_scheme("http://localhost:8545");
        assert!(result.is_ok(), "URL with port should be valid");
    }

    #[test]
    fn test_validate_url_scheme_with_ftp() {
        let result = RpcConfig::validate_url_scheme("ftp://example.com");
        assert!(result.is_err(), "FTP URL should be invalid");
    }

    #[test]
    fn test_validate_url_scheme_with_invalid_url() {
        let result = RpcConfig::validate_url_scheme("invalid-url");
        assert!(result.is_err(), "Invalid URL format should be rejected");
    }

    #[test]
    fn test_validate_url_scheme_with_empty_string() {
        let result = RpcConfig::validate_url_scheme("");
        assert!(result.is_err(), "Empty string should be rejected");
    }

    // Tests for validate_list function
    #[test]
    fn test_validate_list_with_empty_vec() {
        let configs: Vec<RpcConfig> = vec![];
        let result = RpcConfig::validate_list(&configs);
        assert!(result.is_ok(), "Empty config vector should be valid");
    }

    #[test]
    fn test_validate_list_with_valid_urls() {
        let configs = vec![
            RpcConfig::new("https://api.example.com".to_string()),
            RpcConfig::new("http://localhost:8545".to_string()),
        ];
        let result = RpcConfig::validate_list(&configs);
        assert!(result.is_ok(), "All URLs are valid, should return Ok");
    }

    #[test]
    fn test_validate_list_with_one_invalid_url() {
        let configs = vec![
            RpcConfig::new("https://api.example.com".to_string()),
            RpcConfig::new("ftp://invalid-scheme.com".to_string()),
            RpcConfig::new("http://another-valid.com".to_string()),
        ];
        let result = RpcConfig::validate_list(&configs);
        assert!(result.is_err(), "Should fail on first invalid URL");
    }

    #[test]
    fn test_validate_list_with_all_invalid_urls() {
        let configs = vec![
            RpcConfig::new("ws://websocket.example.com".to_string()),
            RpcConfig::new("ftp://invalid-scheme.com".to_string()),
        ];
        let result = RpcConfig::validate_list(&configs);
        assert!(result.is_err(), "Should fail with all invalid URLs");
    }

    // =========================================================================
    // Tests for deserialize_rpc_urls function
    // =========================================================================

    /// Helper struct to test the deserialize_rpc_urls function via serde
    #[derive(Deserialize, Debug)]
    struct TestRpcUrlsContainer {
        #[serde(default, deserialize_with = "super::deserialize_rpc_urls")]
        rpc_urls: Option<Vec<RpcConfig>>,
    }

    #[test]
    fn test_deserialize_rpc_urls_simple_format_single_url() {
        let json = r#"{"rpc_urls": ["https://rpc.example.com"]}"#;
        let result: TestRpcUrlsContainer = serde_json::from_str(json).unwrap();

        let urls = result.rpc_urls.unwrap();
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].url, "https://rpc.example.com");
        assert_eq!(urls[0].weight, DEFAULT_RPC_WEIGHT);
    }

    #[test]
    fn test_deserialize_rpc_urls_simple_format_multiple_urls() {
        let json = r#"{"rpc_urls": ["https://rpc1.com", "https://rpc2.com", "https://rpc3.com"]}"#;
        let result: TestRpcUrlsContainer = serde_json::from_str(json).unwrap();

        let urls = result.rpc_urls.unwrap();
        assert_eq!(urls.len(), 3);
        assert_eq!(urls[0].url, "https://rpc1.com");
        assert_eq!(urls[1].url, "https://rpc2.com");
        assert_eq!(urls[2].url, "https://rpc3.com");
        // All should have default weight
        for url in &urls {
            assert_eq!(url.weight, DEFAULT_RPC_WEIGHT);
        }
    }

    #[test]
    fn test_deserialize_rpc_urls_extended_format_single_config() {
        let json = r#"{"rpc_urls": [{"url": "https://rpc.example.com", "weight": 50}]}"#;
        let result: TestRpcUrlsContainer = serde_json::from_str(json).unwrap();

        let urls = result.rpc_urls.unwrap();
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].url, "https://rpc.example.com");
        assert_eq!(urls[0].weight, 50);
    }

    #[test]
    fn test_deserialize_rpc_urls_extended_format_multiple_configs() {
        let json = r#"{"rpc_urls": [
            {"url": "https://primary.com", "weight": 80},
            {"url": "https://secondary.com", "weight": 15},
            {"url": "https://fallback.com", "weight": 5}
        ]}"#;
        let result: TestRpcUrlsContainer = serde_json::from_str(json).unwrap();

        let urls = result.rpc_urls.unwrap();
        assert_eq!(urls.len(), 3);
        assert_eq!(urls[0].url, "https://primary.com");
        assert_eq!(urls[0].weight, 80);
        assert_eq!(urls[1].url, "https://secondary.com");
        assert_eq!(urls[1].weight, 15);
        assert_eq!(urls[2].url, "https://fallback.com");
        assert_eq!(urls[2].weight, 5);
    }

    #[test]
    fn test_deserialize_rpc_urls_extended_format_without_weight() {
        // When weight is omitted in extended format, it should default
        let json = r#"{"rpc_urls": [{"url": "https://rpc.example.com"}]}"#;
        let result: TestRpcUrlsContainer = serde_json::from_str(json).unwrap();

        let urls = result.rpc_urls.unwrap();
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].url, "https://rpc.example.com");
        assert_eq!(urls[0].weight, DEFAULT_RPC_WEIGHT);
    }

    #[test]
    fn test_deserialize_rpc_urls_mixed_format() {
        let json = r#"{"rpc_urls": [
            "https://simple.com",
            {"url": "https://weighted.com", "weight": 75},
            "https://another-simple.com"
        ]}"#;
        let result: TestRpcUrlsContainer = serde_json::from_str(json).unwrap();

        let urls = result.rpc_urls.unwrap();
        assert_eq!(urls.len(), 3);

        // First: simple string format
        assert_eq!(urls[0].url, "https://simple.com");
        assert_eq!(urls[0].weight, DEFAULT_RPC_WEIGHT);

        // Second: extended object format
        assert_eq!(urls[1].url, "https://weighted.com");
        assert_eq!(urls[1].weight, 75);

        // Third: simple string format
        assert_eq!(urls[2].url, "https://another-simple.com");
        assert_eq!(urls[2].weight, DEFAULT_RPC_WEIGHT);
    }

    #[test]
    fn test_deserialize_rpc_urls_none_when_field_missing() {
        let json = r#"{}"#;
        let result: TestRpcUrlsContainer = serde_json::from_str(json).unwrap();

        assert!(result.rpc_urls.is_none());
    }

    #[test]
    fn test_deserialize_rpc_urls_none_when_null() {
        let json = r#"{"rpc_urls": null}"#;
        let result: TestRpcUrlsContainer = serde_json::from_str(json).unwrap();

        assert!(result.rpc_urls.is_none());
    }

    #[test]
    fn test_deserialize_rpc_urls_empty_array() {
        let json = r#"{"rpc_urls": []}"#;
        let result: TestRpcUrlsContainer = serde_json::from_str(json).unwrap();

        let urls = result.rpc_urls.unwrap();
        assert!(urls.is_empty());
    }

    #[test]
    fn test_deserialize_rpc_urls_weight_zero() {
        let json = r#"{"rpc_urls": [{"url": "https://disabled.com", "weight": 0}]}"#;
        let result: TestRpcUrlsContainer = serde_json::from_str(json).unwrap();

        let urls = result.rpc_urls.unwrap();
        assert_eq!(urls[0].weight, 0);
    }

    #[test]
    fn test_deserialize_rpc_urls_weight_max() {
        let json = r#"{"rpc_urls": [{"url": "https://max.com", "weight": 100}]}"#;
        let result: TestRpcUrlsContainer = serde_json::from_str(json).unwrap();

        let urls = result.rpc_urls.unwrap();
        assert_eq!(urls[0].weight, 100);
    }

    #[test]
    fn test_deserialize_rpc_urls_invalid_not_array() {
        let json = r#"{"rpc_urls": "https://not-an-array.com"}"#;
        let result: Result<TestRpcUrlsContainer, _> = serde_json::from_str(json);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("rpc_urls must be an array"),
            "Error should mention array requirement: {}",
            err
        );
    }

    #[test]
    fn test_deserialize_rpc_urls_invalid_number_in_array() {
        let json = r#"{"rpc_urls": [123, 456]}"#;
        let result: Result<TestRpcUrlsContainer, _> = serde_json::from_str(json);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("rpc_urls must be an array of strings or RpcConfig objects"),
            "Error should mention valid types: {}",
            err
        );
    }

    #[test]
    fn test_deserialize_rpc_urls_invalid_boolean_in_array() {
        let json = r#"{"rpc_urls": [true, false]}"#;
        let result: Result<TestRpcUrlsContainer, _> = serde_json::from_str(json);

        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_rpc_urls_invalid_nested_array() {
        let json = r#"{"rpc_urls": [["nested", "array"]]}"#;
        let result: Result<TestRpcUrlsContainer, _> = serde_json::from_str(json);

        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_rpc_urls_invalid_object_in_array() {
        let json = r#"{"rpc_urls": {"not": "an_array"}}"#;
        let result: Result<TestRpcUrlsContainer, _> = serde_json::from_str(json);

        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_rpc_urls_invalid_object_missing_url() {
        // Object format requires 'url' field
        let json = r#"{"rpc_urls": [{"weight": 50}]}"#;
        let result: Result<TestRpcUrlsContainer, _> = serde_json::from_str(json);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("url") || err.contains("missing field"),
            "Error should mention missing url field: {}",
            err
        );
    }

    #[test]
    fn test_deserialize_rpc_urls_mixed_valid_and_invalid() {
        // One valid string followed by an invalid number
        let json = r#"{"rpc_urls": ["https://valid.com", 12345]}"#;
        let result: Result<TestRpcUrlsContainer, _> = serde_json::from_str(json);

        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_rpc_urls_preserves_url_with_special_chars() {
        let json = r#"{"rpc_urls": ["https://rpc.example.com/v1?api_key=abc123&network=mainnet"]}"#;
        let result: TestRpcUrlsContainer = serde_json::from_str(json).unwrap();

        let urls = result.rpc_urls.unwrap();
        assert_eq!(
            urls[0].url,
            "https://rpc.example.com/v1?api_key=abc123&network=mainnet"
        );
    }

    #[test]
    fn test_deserialize_rpc_urls_preserves_url_with_port() {
        let json = r#"{"rpc_urls": ["http://localhost:8545"]}"#;
        let result: TestRpcUrlsContainer = serde_json::from_str(json).unwrap();

        let urls = result.rpc_urls.unwrap();
        assert_eq!(urls[0].url, "http://localhost:8545");
    }

    #[test]
    fn test_deserialize_rpc_urls_unicode_url() {
        let json = r#"{"rpc_urls": ["https://测试.example.com"]}"#;
        let result: TestRpcUrlsContainer = serde_json::from_str(json).unwrap();

        let urls = result.rpc_urls.unwrap();
        assert_eq!(urls[0].url, "https://测试.example.com");
    }

    #[test]
    fn test_deserialize_rpc_urls_empty_string_url() {
        // Empty string is technically valid JSON, deserialization should succeed
        // (validation happens at a different layer)
        let json = r#"{"rpc_urls": [""]}"#;
        let result: TestRpcUrlsContainer = serde_json::from_str(json).unwrap();

        let urls = result.rpc_urls.unwrap();
        assert_eq!(urls[0].url, "");
    }

    #[test]
    fn test_deserialize_rpc_urls_whitespace_url() {
        let json = r#"{"rpc_urls": ["  https://rpc.example.com  "]}"#;
        let result: TestRpcUrlsContainer = serde_json::from_str(json).unwrap();

        let urls = result.rpc_urls.unwrap();
        // Whitespace is preserved (trimming is a validation concern)
        assert_eq!(urls[0].url, "  https://rpc.example.com  ");
    }
}
