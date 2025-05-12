//! URL validation utilities
//!
//! This module provides utility functions for validating URLs.

use eyre::{eyre, Result};

use crate::models::RpcConfig;

/// Validates that a URL has an HTTP or HTTPS scheme.
///
/// # Arguments
/// * `url` - The URL to validate
///
/// # Returns
/// * `Result<()>` - Ok if the URL has a valid scheme, error otherwise
///
/// # Examples
/// ```
/// use crate::utils::url::validate_http_url;
///
/// // Valid URLs
/// assert!(validate_http_url("http://example.com").is_ok());
/// assert!(validate_http_url("https://secure.example.com").is_ok());
///
/// // Invalid URLs
/// assert!(validate_http_url("ftp://example.com").is_err());
/// assert!(validate_http_url("invalid-url").is_err());
/// ```
pub fn validate_http_url(url: &str) -> Result<()> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(eyre!(
            "Invalid URL scheme for {}: Only HTTP and HTTPS are supported",
            url
        ));
    }
    Ok(())
}

/// Validates all URLs in a vector of RpcConfig objects.
///
/// This function is a utility for validating all URLs in a configuration
/// collection before initializing providers.
///
/// # Arguments
/// * `configs` - A slice of RpcConfig objects
///
/// # Returns
/// * `Result<()>` - Ok if all URLs have valid schemes, error on first invalid URL
///
/// # Examples
/// ```
/// use crate::utils::url::validate_configs_urls;
/// use crate::config::RpcConfig;
///
/// let configs = vec![
///     RpcConfig::new("https://api.example.com".to_string()),
///     RpcConfig::new("http://localhost:8545".to_string()),
/// ];
/// assert!(validate_configs_urls(&configs).is_ok());
/// ```
pub fn validate_configs_urls(configs: &[RpcConfig]) -> Result<()> {
    for config in configs {
        validate_http_url(&config.url)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_http_url_with_http() {
        let result = validate_http_url("http://example.com");
        assert!(result.is_ok(), "HTTP URL should be valid");
    }

    #[test]
    fn test_validate_http_url_with_https() {
        let result = validate_http_url("https://secure.example.com");
        assert!(result.is_ok(), "HTTPS URL should be valid");
    }

    #[test]
    fn test_validate_http_url_with_query_params() {
        let result = validate_http_url("https://example.com/api?param=value&other=123");
        assert!(result.is_ok(), "URL with query parameters should be valid");
    }

    #[test]
    fn test_validate_http_url_with_port() {
        let result = validate_http_url("http://localhost:8545");
        assert!(result.is_ok(), "URL with port should be valid");
    }

    #[test]
    fn test_validate_http_url_with_ftp() {
        let result = validate_http_url("ftp://example.com");
        assert!(result.is_err(), "FTP URL should be invalid");
    }

    #[test]
    fn test_validate_http_url_with_invalid_url() {
        let result = validate_http_url("invalid-url");
        assert!(result.is_err(), "Invalid URL format should be rejected");
    }

    #[test]
    fn test_validate_http_url_with_empty_string() {
        let result = validate_http_url("");
        assert!(result.is_err(), "Empty string should be rejected");
    }

    // Tests for validate_configs_urls function
    #[test]
    fn test_validate_configs_urls_with_empty_vec() {
        let configs: Vec<RpcConfig> = vec![];
        let result = validate_configs_urls(&configs);
        assert!(result.is_ok(), "Empty config vector should be valid");
    }

    #[test]
    fn test_validate_configs_urls_with_valid_urls() {
        let configs = vec![
            RpcConfig::new("https://api.example.com".to_string()),
            RpcConfig::new("http://localhost:8545".to_string()),
        ];
        let result = validate_configs_urls(&configs);
        assert!(result.is_ok(), "All URLs are valid, should return Ok");
    }

    #[test]
    fn test_validate_configs_urls_with_one_invalid_url() {
        let configs = vec![
            RpcConfig::new("https://api.example.com".to_string()),
            RpcConfig::new("ftp://invalid-scheme.com".to_string()),
            RpcConfig::new("http://another-valid.com".to_string()),
        ];
        let result = validate_configs_urls(&configs);
        assert!(result.is_err(), "Should fail on first invalid URL");
    }

    #[test]
    fn test_validate_configs_urls_with_all_invalid_urls() {
        let configs = vec![
            RpcConfig::new("ws://websocket.example.com".to_string()),
            RpcConfig::new("ftp://invalid-scheme.com".to_string()),
        ];
        let result = validate_configs_urls(&configs);
        assert!(result.is_err(), "Should fail with all invalid URLs");
    }
}
