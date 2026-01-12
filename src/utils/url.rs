//! URL utility functions.
//!
//! This module provides utility functions for working with URLs,
//! including masking sensitive information from URLs.

/// Masks a URL by showing only the scheme and host, hiding the path and query parameters.
///
/// This is used to safely display RPC URLs in API responses and logs without exposing
/// sensitive API keys that are often embedded in the URL path or query string.
///
/// # Examples
/// - `https://eth-mainnet.g.alchemy.com/v2/abc123` → `https://eth-mainnet.g.alchemy.com/***`
/// - `https://mainnet.infura.io/v3/PROJECT_ID` → `https://mainnet.infura.io/***`
/// - `http://localhost:8545` → `http://localhost:8545` (no path to mask)
/// - `invalid-url` → `***` (fallback for unparseable URLs)
pub fn mask_url(url: &str) -> String {
    // Find the scheme separator "://"
    let Some(scheme_end) = url.find("://") else {
        // No valid scheme, mask entirely for safety
        return "***".to_string();
    };

    // Find where the host ends (first "/" after "://")
    let host_start = scheme_end + 3; // Skip "://"
    let rest = &url[host_start..];

    // Find the first "/" which marks the start of the path
    if let Some(path_start) = rest.find('/') {
        // Check if there's actually content in the path (not just "/")
        let path_and_beyond = &rest[path_start..];
        if path_and_beyond.len() > 1 || url.contains('?') {
            // There's a path or query to mask
            let host_end = host_start + path_start;
            format!("{}/***", &url[..host_end])
        } else {
            // Just a trailing "/" with no real path content
            url.to_string()
        }
    } else if url.contains('?') {
        // No path but has query parameters - mask those
        let query_start = url.find('?').unwrap();
        format!("{}?***", &url[..query_start])
    } else {
        // No path or query to mask, return original
        url.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_url_alchemy_with_api_key() {
        let url = "https://eth-mainnet.g.alchemy.com/v2/abc123xyz";
        let masked = mask_url(url);
        assert_eq!(masked, "https://eth-mainnet.g.alchemy.com/***");
    }

    #[test]
    fn test_mask_url_infura_with_project_id() {
        let url = "https://mainnet.infura.io/v3/my-project-id";
        let masked = mask_url(url);
        assert_eq!(masked, "https://mainnet.infura.io/***");
    }

    #[test]
    fn test_mask_url_quicknode_with_api_key() {
        let url = "https://my-node.quiknode.pro/secret-api-key/";
        let masked = mask_url(url);
        assert_eq!(masked, "https://my-node.quiknode.pro/***");
    }

    #[test]
    fn test_mask_url_localhost_no_path() {
        // No path to mask, should return original
        let url = "http://localhost:8545";
        let masked = mask_url(url);
        assert_eq!(masked, "http://localhost:8545");
    }

    #[test]
    fn test_mask_url_localhost_with_trailing_slash() {
        // Just a trailing slash with no real path content
        let url = "http://localhost:8545/";
        let masked = mask_url(url);
        assert_eq!(masked, "http://localhost:8545/");
    }

    #[test]
    fn test_mask_url_with_query_params() {
        let url = "https://rpc.example.com/v1?api_key=secret123&network=mainnet";
        let masked = mask_url(url);
        assert_eq!(masked, "https://rpc.example.com/***");
    }

    #[test]
    fn test_mask_url_query_params_no_path() {
        let url = "https://rpc.example.com?api_key=secret123";
        let masked = mask_url(url);
        assert_eq!(masked, "https://rpc.example.com?***");
    }

    #[test]
    fn test_mask_url_invalid_url_no_scheme() {
        // Invalid URL without scheme should be fully masked for safety
        let url = "invalid-url";
        let masked = mask_url(url);
        assert_eq!(masked, "***");
    }

    #[test]
    fn test_mask_url_empty_string() {
        let url = "";
        let masked = mask_url(url);
        assert_eq!(masked, "***");
    }

    #[test]
    fn test_mask_url_with_port_and_path() {
        let url = "https://rpc.example.com:8080/api/v1/secret";
        let masked = mask_url(url);
        assert_eq!(masked, "https://rpc.example.com:8080/***");
    }

    #[test]
    fn test_mask_url_ankr_with_api_key() {
        let url = "https://rpc.ankr.com/eth/my-api-key-here";
        let masked = mask_url(url);
        assert_eq!(masked, "https://rpc.ankr.com/***");
    }
}
