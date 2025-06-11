//! Configuration for RPC endpoints.
//!
//! This module provides configuration structures for RPC endpoints,
//! including URLs and weights for load balancing.

use crate::constants::DEFAULT_RPC_WEIGHT;
use eyre::{eyre, Result};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use utoipa::ToSchema;

#[derive(Debug, Error, PartialEq)]
pub enum RpcConfigError {
    #[error("Invalid weight: {value}. Must be between 0 and 100.")]
    InvalidWeight { value: u8 },
}

/// Returns the default RPC weight.
fn default_rpc_weight() -> u8 {
    DEFAULT_RPC_WEIGHT
}

/// Configuration for an RPC endpoint.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct RpcConfig {
    /// The RPC endpoint URL.
    pub url: String,
    /// The weight of this endpoint in the weighted round-robin selection.
    /// Defaults to DEFAULT_RPC_WEIGHT (100). Should be between 0 and 100.
    #[serde(default = "default_rpc_weight")]
    pub weight: u8,
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
    fn validate_url_scheme(url: &str) -> Result<()> {
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
    pub fn validate_list(configs: &[RpcConfig]) -> Result<()> {
        for config in configs {
            // Call the helper function using Self to refer to the type for associated functions
            Self::validate_url_scheme(&config.url)?;
        }
        Ok(())
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
        let config = RpcConfig { url, weight };

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
}
