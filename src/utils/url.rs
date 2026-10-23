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
/// - `https://user:pass@rpc.example.com/v1` → `https://***@rpc.example.com/***` (userinfo is always hidden)
/// - `https://rpc.example.com#token` → `https://rpc.example.com#***`
/// - `invalid-url` → `***` (fallback for unparsable URLs)
pub fn mask_url(url: &str) -> String {
    // Split only what the URL parser accepts, right after the scheme it found. Otherwise the
    // host boundary below may not be the real one: in `https://user:pa/ss@host/rpc` the "/"
    // would end the authority early and leave part of the password in the output.
    let Ok(parsed) = url::Url::parse(url) else {
        return "***".to_string();
    };
    let scheme_end = parsed.scheme().len();
    let host_start = scheme_end + 3; // Skip "://"
    if url.get(scheme_end..host_start) != Some("://") {
        return "***".to_string();
    }

    // The authority ends at the first "/", "?", "#" or "\" (the parser reads "\" as "/" in
    // http(s) URLs)
    let authority_end = url[host_start..]
        .find(['/', '?', '#', '\\'])
        .map_or(url.len(), |i| host_start + i);

    // Userinfo ("user:password@") carries credentials, so it is never shown
    let authority = &url[host_start..authority_end];
    let host = match authority.rfind('@') {
        Some(at) => format!("***@{}", &authority[at + 1..]),
        None => authority.to_string(),
    };

    // Everything after the authority is masked, except a bare trailing "/"
    let tail = &url[authority_end..];
    let masked_tail = if tail.is_empty() || tail == "/" {
        tail
    } else if tail.starts_with('?') {
        "?***"
    } else if tail.starts_with('#') {
        "#***"
    } else {
        "/***"
    };

    format!("{}{}{}", &url[..host_start], host, masked_tail)
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

    #[test]
    fn test_mask_url_hides_userinfo_with_path() {
        let url = "https://user:SECRET@rpc.example.com/rpc";
        let masked = mask_url(url);
        assert_eq!(masked, "https://***@rpc.example.com/***");
    }

    #[test]
    fn test_mask_url_hides_userinfo_without_path() {
        assert_eq!(
            mask_url("https://user:SECRET@rpc.example.com"),
            "https://***@rpc.example.com"
        );
        assert_eq!(
            mask_url("https://user:SECRET@rpc.example.com:8545/"),
            "https://***@rpc.example.com:8545/"
        );
        assert_eq!(
            mask_url("https://user:SECRET@rpc.example.com?key=abc"),
            "https://***@rpc.example.com?***"
        );
    }

    #[test]
    fn test_mask_url_at_sign_in_path_is_not_userinfo() {
        let url = "https://rpc.example.com/v1/user@example.org";
        let masked = mask_url(url);
        assert_eq!(masked, "https://rpc.example.com/***");
    }

    #[test]
    fn test_mask_url_unparsable_falls_back_to_stars() {
        // "/" in the password ends the authority for the parser too, so this is not a valid URL
        assert_eq!(mask_url("https://user:pa/ss@rpc.example.com/rpc"), "***");
        // "://" later in the string is not the scheme separator
        assert_eq!(mask_url("https:/user:SECRET@rpc.example.com/a://b"), "***");
    }

    #[test]
    fn test_mask_url_masks_fragment() {
        assert_eq!(
            mask_url("https://rpc.example.com#SECRET"),
            "https://rpc.example.com#***"
        );
        assert_eq!(
            mask_url("https://user:SECRET@rpc.example.com#SECRET"),
            "https://***@rpc.example.com#***"
        );
    }

    #[test]
    fn test_mask_url_masks_from_first_delimiter_after_host() {
        // A "/" inside the query is not the start of the path
        assert_eq!(
            mask_url("https://rpc.example.com?apikey=SECRET/abc"),
            "https://rpc.example.com?***"
        );
        // The parser reads "\" as "/" in http(s) URLs
        assert_eq!(
            mask_url("https://rpc.example.com\\v2\\SECRET"),
            "https://rpc.example.com/***"
        );
    }
}
