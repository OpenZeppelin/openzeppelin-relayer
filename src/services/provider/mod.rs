use std::num::ParseIntError;

use crate::config::ServerConfig;
use crate::models::{EvmNetwork, RpcConfig, SolanaNetwork, StellarNetwork};
use serde::Serialize;
use thiserror::Error;

use alloy::transports::RpcError;

pub mod evm;
pub use evm::*;

mod solana;
pub use solana::*;

mod stellar;
pub use stellar::*;

mod retry;
pub use retry::*;

pub mod rpc_selector;

#[derive(Error, Debug, Serialize)]
pub enum ProviderError {
    #[error("RPC client error: {0}")]
    SolanaRpcError(#[from] SolanaProviderError),
    #[error("Invalid address: {0}")]
    InvalidAddress(String),
    #[error("Network configuration error: {0}")]
    NetworkConfiguration(String),
    #[error("Request timeout")]
    Timeout,
    #[error("Rate limited (HTTP 429)")]
    RateLimited,
    #[error("Bad gateway (HTTP 502)")]
    BadGateway,
    #[error("Request error (HTTP {status_code}): {error}")]
    RequestError { error: String, status_code: u16 },
    #[error("JSON-RPC error (code {code}): {message}")]
    RpcErrorCode { code: i64, message: String },
    #[error("Transport error: {0}")]
    TransportError(String),
    #[error("Other provider error: {0}")]
    Other(String),
}

impl ProviderError {
    /// Determines if this error is transient (can retry) or permanent (should fail).
    pub fn is_transient(&self) -> bool {
        is_retriable_error(self)
    }
}

impl From<hex::FromHexError> for ProviderError {
    fn from(err: hex::FromHexError) -> Self {
        ProviderError::InvalidAddress(err.to_string())
    }
}

impl From<std::net::AddrParseError> for ProviderError {
    fn from(err: std::net::AddrParseError) -> Self {
        ProviderError::NetworkConfiguration(format!("Invalid network address: {err}"))
    }
}

impl From<ParseIntError> for ProviderError {
    fn from(err: ParseIntError) -> Self {
        ProviderError::Other(format!("Number parsing error: {err}"))
    }
}

/// Categorizes a reqwest error into an appropriate `ProviderError` variant.
///
/// This function analyzes the given reqwest error and maps it to a specific
/// `ProviderError` variant based on the error's properties:
/// - Timeout errors become `ProviderError::Timeout`
/// - HTTP 429 responses become `ProviderError::RateLimited`
/// - HTTP 502 responses become `ProviderError::BadGateway`
/// - All other errors become `ProviderError::Other` with the error message
///
/// # Arguments
///
/// * `err` - A reference to the reqwest error to categorize
///
/// # Returns
///
/// The appropriate `ProviderError` variant based on the error type
fn categorize_reqwest_error(err: &reqwest::Error) -> ProviderError {
    if err.is_timeout() {
        return ProviderError::Timeout;
    }

    if let Some(status) = err.status() {
        match status.as_u16() {
            429 => return ProviderError::RateLimited,
            502 => return ProviderError::BadGateway,
            _ => {
                return ProviderError::RequestError {
                    error: err.to_string(),
                    status_code: status.as_u16(),
                }
            }
        }
    }

    ProviderError::Other(err.to_string())
}

impl From<reqwest::Error> for ProviderError {
    fn from(err: reqwest::Error) -> Self {
        categorize_reqwest_error(&err)
    }
}

impl From<&reqwest::Error> for ProviderError {
    fn from(err: &reqwest::Error) -> Self {
        categorize_reqwest_error(err)
    }
}

impl From<eyre::Report> for ProviderError {
    fn from(err: eyre::Report) -> Self {
        // Downcast to known error types first
        if let Some(reqwest_err) = err.downcast_ref::<reqwest::Error>() {
            return ProviderError::from(reqwest_err);
        }

        // Default to Other for unknown error types
        ProviderError::Other(err.to_string())
    }
}

// Add conversion from String to ProviderError
impl From<String> for ProviderError {
    fn from(error: String) -> Self {
        ProviderError::Other(error)
    }
}

// Generic implementation for all RpcError types
impl<E> From<RpcError<E>> for ProviderError
where
    E: std::fmt::Display + std::any::Any + 'static,
{
    fn from(err: RpcError<E>) -> Self {
        match err {
            RpcError::Transport(transport_err) => {
                // First check if it's a reqwest::Error using downcasting
                if let Some(reqwest_err) =
                    (&transport_err as &dyn std::any::Any).downcast_ref::<reqwest::Error>()
                {
                    return categorize_reqwest_error(reqwest_err);
                }

                ProviderError::TransportError(transport_err.to_string())
            }
            RpcError::ErrorResp(json_rpc_err) => ProviderError::RpcErrorCode {
                code: json_rpc_err.code,
                message: json_rpc_err.message.to_string(),
            },
            _ => ProviderError::Other(format!("Other RPC error: {err}")),
        }
    }
}

// Implement From for RpcSelectorError
impl From<rpc_selector::RpcSelectorError> for ProviderError {
    fn from(err: rpc_selector::RpcSelectorError) -> Self {
        ProviderError::NetworkConfiguration(format!("RPC selector error: {err}"))
    }
}

pub trait NetworkConfiguration: Sized {
    type Provider;

    fn public_rpc_urls(&self) -> Vec<String>;

    fn new_provider(
        rpc_urls: Vec<RpcConfig>,
        timeout_seconds: u64,
    ) -> Result<Self::Provider, ProviderError>;
}

impl NetworkConfiguration for EvmNetwork {
    type Provider = EvmProvider;

    fn public_rpc_urls(&self) -> Vec<String> {
        (*self)
            .public_rpc_urls()
            .map(|urls| urls.iter().map(|url| url.to_string()).collect())
            .unwrap_or_default()
    }

    fn new_provider(
        rpc_urls: Vec<RpcConfig>,
        timeout_seconds: u64,
    ) -> Result<Self::Provider, ProviderError> {
        EvmProvider::new(rpc_urls, timeout_seconds)
    }
}

impl NetworkConfiguration for SolanaNetwork {
    type Provider = SolanaProvider;

    fn public_rpc_urls(&self) -> Vec<String> {
        (*self)
            .public_rpc_urls()
            .map(|urls| urls.to_vec())
            .unwrap_or_default()
    }

    fn new_provider(
        rpc_urls: Vec<RpcConfig>,
        timeout_seconds: u64,
    ) -> Result<Self::Provider, ProviderError> {
        SolanaProvider::new(rpc_urls, timeout_seconds)
    }
}

impl NetworkConfiguration for StellarNetwork {
    type Provider = StellarProvider;

    fn public_rpc_urls(&self) -> Vec<String> {
        (*self)
            .public_rpc_urls()
            .map(|urls| urls.to_vec())
            .unwrap_or_default()
    }

    fn new_provider(
        rpc_urls: Vec<RpcConfig>,
        timeout_seconds: u64,
    ) -> Result<Self::Provider, ProviderError> {
        StellarProvider::new(rpc_urls, timeout_seconds)
    }
}

/// Creates a network-specific provider instance based on the provided configuration.
///
/// # Type Parameters
///
/// * `N`: The type of the network, which must implement the `NetworkConfiguration` trait.
///   This determines the specific provider type (`N::Provider`) and how to obtain
///   public RPC URLs.
///
/// # Arguments
///
/// * `network`: A reference to the network configuration object (`&N`).
/// * `custom_rpc_urls`: An `Option<Vec<RpcConfig>>`. If `Some` and not empty, these URLs
///   are used to configure the provider. If `None` or `Some` but empty, the function
///   falls back to using the public RPC URLs defined by the `network`'s
///   `NetworkConfiguration` implementation.
///
/// # Returns
///
/// * `Ok(N::Provider)`: An instance of the network-specific provider on success.
/// * `Err(ProviderError)`: An error if configuration fails, such as when no custom URLs
///   are provided and the network has no public RPC URLs defined
///   (`ProviderError::NetworkConfiguration`).
pub fn get_network_provider<N: NetworkConfiguration>(
    network: &N,
    custom_rpc_urls: Option<Vec<RpcConfig>>,
) -> Result<N::Provider, ProviderError> {
    let rpc_timeout_ms = ServerConfig::from_env().rpc_timeout_ms;
    let timeout_seconds = rpc_timeout_ms / 1000; // Convert ms to s

    let rpc_urls = match custom_rpc_urls {
        Some(configs) if !configs.is_empty() => configs,
        _ => {
            let urls = network.public_rpc_urls();
            if urls.is_empty() {
                return Err(ProviderError::NetworkConfiguration(
                    "No public RPC URLs available for this network".to_string(),
                ));
            }
            urls.into_iter().map(RpcConfig::new).collect()
        }
    };

    N::new_provider(rpc_urls, timeout_seconds)
}

/// Determines if an HTTP status code indicates the provider should be marked as failed.
///
/// This is a low-level function that can be reused across different error types.
///
/// Returns `true` for:
/// - 5xx Server Errors (500-599) - RPC node is having issues
/// - Specific 4xx Client Errors that indicate provider issues:
///   - 401 (Unauthorized) - auth required but not provided
///   - 403 (Forbidden) - node is blocking requests or auth issues
///   - 404 (Not Found) - endpoint doesn't exist or misconfigured
///   - 410 (Gone) - endpoint permanently removed
pub fn should_mark_provider_failed_by_status_code(status_code: u16) -> bool {
    match status_code {
        // 5xx Server Errors - RPC node is having issues
        500..=599 => true,

        // 4xx Client Errors that indicate we can't use this provider
        401 => true, // Unauthorized - auth required but not provided
        403 => true, // Forbidden - node is blocking requests or auth issues
        404 => true, // Not Found - endpoint doesn't exist or misconfigured
        410 => true, // Gone - endpoint permanently removed

        _ => false,
    }
}

pub fn should_mark_provider_failed(error: &ProviderError) -> bool {
    match error {
        ProviderError::RequestError { status_code, .. } => {
            should_mark_provider_failed_by_status_code(*status_code)
        }
        _ => false,
    }
}

// Errors that are retriable
pub fn is_retriable_error(error: &ProviderError) -> bool {
    match error {
        // HTTP-level errors that are retriable
        ProviderError::Timeout
        | ProviderError::RateLimited
        | ProviderError::BadGateway
        | ProviderError::TransportError(_) => true,

        ProviderError::RequestError { status_code, .. } => {
            match *status_code {
                // Non-retriable 5xx: persistent server-side issues
                501 | 505 => false, // Not Implemented, HTTP Version Not Supported

                // Retriable 5xx: temporary server-side issues
                500 | 502..=504 | 506..=599 => true,

                // Retriable 4xx: timeout or rate-limit related
                408 | 425 | 429 => true,

                // Non-retriable 4xx: client errors
                400..=499 => false,

                // Other status codes: not retriable
                _ => false,
            }
        }

        // JSON-RPC error codes (EIP-1474)
        ProviderError::RpcErrorCode { code, .. } => {
            match code {
                // -32002: Resource unavailable (temporary state)
                -32002 => true,
                // -32005: Limit exceeded / rate limited
                -32005 => true,
                // -32603: Internal error (may be temporary)
                -32603 => true,
                // -32000: Invalid input
                -32000 => false,
                // -32001: Resource not found
                -32001 => false,
                // -32003: Transaction rejected
                -32003 => false,
                // -32004: Method not supported
                -32004 => false,

                // Standard JSON-RPC 2.0 errors (not retriable)
                // -32700: Parse error
                // -32600: Invalid request
                // -32601: Method not found
                // -32602: Invalid params
                -32700..=-32600 => false,

                // All other error codes: not retriable by default
                _ => false,
            }
        }

        ProviderError::SolanaRpcError(err) => err.is_transient(),

        // Any other errors: check message for network-related issues
        _ => {
            let err_msg = format!("{error}");
            let msg_lower = err_msg.to_lowercase();
            msg_lower.contains("timeout")
                || msg_lower.contains("connection")
                || msg_lower.contains("reset")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lazy_static::lazy_static;
    use std::env;
    use std::sync::Mutex;
    use std::time::Duration;

    // Use a mutex to ensure tests don't run in parallel when modifying env vars
    lazy_static! {
        static ref ENV_MUTEX: Mutex<()> = Mutex::new(());
    }

    fn setup_test_env() {
        env::set_var("API_KEY", "7EF1CB7C-5003-4696-B384-C72AF8C3E15D"); // noboost
        env::set_var("REDIS_URL", "redis://localhost:6379");
        env::set_var("RPC_TIMEOUT_MS", "5000");
    }

    fn cleanup_test_env() {
        env::remove_var("API_KEY");
        env::remove_var("REDIS_URL");
        env::remove_var("RPC_TIMEOUT_MS");
    }

    fn create_test_evm_network() -> EvmNetwork {
        EvmNetwork {
            network: "test-evm".to_string(),
            rpc_urls: vec!["https://rpc.example.com".to_string()],
            explorer_urls: None,
            average_blocktime_ms: 12000,
            is_testnet: true,
            tags: vec![],
            chain_id: 1337,
            required_confirmations: 1,
            features: vec![],
            symbol: "ETH".to_string(),
            gas_price_cache: None,
        }
    }

    fn create_test_solana_network(network_str: &str) -> SolanaNetwork {
        SolanaNetwork {
            network: network_str.to_string(),
            rpc_urls: vec!["https://api.testnet.solana.com".to_string()],
            explorer_urls: None,
            average_blocktime_ms: 400,
            is_testnet: true,
            tags: vec![],
        }
    }

    fn create_test_stellar_network() -> StellarNetwork {
        StellarNetwork {
            network: "testnet".to_string(),
            rpc_urls: vec!["https://soroban-testnet.stellar.org".to_string()],
            explorer_urls: None,
            average_blocktime_ms: 5000,
            is_testnet: true,
            tags: vec![],
            passphrase: "Test SDF Network ; September 2015".to_string(),
            horizon_url: Some("https://horizon-testnet.stellar.org".to_string()),
        }
    }

    #[test]
    fn test_from_hex_error() {
        let hex_error = hex::FromHexError::OddLength;
        let provider_error: ProviderError = hex_error.into();
        assert!(matches!(provider_error, ProviderError::InvalidAddress(_)));
    }

    #[test]
    fn test_from_addr_parse_error() {
        let addr_error = "invalid:address"
            .parse::<std::net::SocketAddr>()
            .unwrap_err();
        let provider_error: ProviderError = addr_error.into();
        assert!(matches!(
            provider_error,
            ProviderError::NetworkConfiguration(_)
        ));
    }

    #[test]
    fn test_from_parse_int_error() {
        let parse_error = "not_a_number".parse::<u64>().unwrap_err();
        let provider_error: ProviderError = parse_error.into();
        assert!(matches!(provider_error, ProviderError::Other(_)));
    }

    #[actix_rt::test]
    async fn test_categorize_reqwest_error_timeout() {
        let client = reqwest::Client::new();
        let timeout_err = client
            .get("http://example.com")
            .timeout(Duration::from_nanos(1))
            .send()
            .await
            .unwrap_err();

        assert!(timeout_err.is_timeout());

        let provider_error = categorize_reqwest_error(&timeout_err);
        assert!(matches!(provider_error, ProviderError::Timeout));
    }

    #[actix_rt::test]
    async fn test_categorize_reqwest_error_rate_limited() {
        let mut mock_server = mockito::Server::new_async().await;

        let _mock = mock_server
            .mock("GET", mockito::Matcher::Any)
            .with_status(429)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let response = client
            .get(mock_server.url())
            .send()
            .await
            .expect("Failed to get response");

        let err = response
            .error_for_status()
            .expect_err("Expected error for status 429");

        assert!(err.status().is_some());
        assert_eq!(err.status().unwrap().as_u16(), 429);

        let provider_error = categorize_reqwest_error(&err);
        assert!(matches!(provider_error, ProviderError::RateLimited));
    }

    #[actix_rt::test]
    async fn test_categorize_reqwest_error_bad_gateway() {
        let mut mock_server = mockito::Server::new_async().await;

        let _mock = mock_server
            .mock("GET", mockito::Matcher::Any)
            .with_status(502)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let response = client
            .get(mock_server.url())
            .send()
            .await
            .expect("Failed to get response");

        let err = response
            .error_for_status()
            .expect_err("Expected error for status 502");

        assert!(err.status().is_some());
        assert_eq!(err.status().unwrap().as_u16(), 502);

        let provider_error = categorize_reqwest_error(&err);
        assert!(matches!(provider_error, ProviderError::BadGateway));
    }

    #[actix_rt::test]
    async fn test_categorize_reqwest_error_other() {
        let client = reqwest::Client::new();
        let err = client
            .get("http://non-existent-host-12345.local")
            .send()
            .await
            .unwrap_err();

        assert!(!err.is_timeout());
        assert!(err.status().is_none()); // No status code

        let provider_error = categorize_reqwest_error(&err);
        assert!(matches!(provider_error, ProviderError::Other(_)));
    }

    #[test]
    fn test_from_eyre_report_other_error() {
        let eyre_error: eyre::Report = eyre::eyre!("Generic error");
        let provider_error: ProviderError = eyre_error.into();
        assert!(matches!(provider_error, ProviderError::Other(_)));
    }

    #[test]
    fn test_get_evm_network_provider_valid_network() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = create_test_evm_network();
        let result = get_network_provider(&network, None);

        cleanup_test_env();
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_evm_network_provider_with_custom_urls() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = create_test_evm_network();
        let custom_urls = vec![
            RpcConfig {
                url: "https://custom-rpc1.example.com".to_string(),
                weight: 1,
            },
            RpcConfig {
                url: "https://custom-rpc2.example.com".to_string(),
                weight: 1,
            },
        ];
        let result = get_network_provider(&network, Some(custom_urls));

        cleanup_test_env();
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_evm_network_provider_with_empty_custom_urls() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = create_test_evm_network();
        let custom_urls: Vec<RpcConfig> = vec![];
        let result = get_network_provider(&network, Some(custom_urls));

        cleanup_test_env();
        assert!(result.is_ok()); // Should fall back to public URLs
    }

    #[test]
    fn test_get_solana_network_provider_valid_network_mainnet_beta() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = create_test_solana_network("mainnet-beta");
        let result = get_network_provider(&network, None);

        cleanup_test_env();
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_solana_network_provider_valid_network_testnet() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = create_test_solana_network("testnet");
        let result = get_network_provider(&network, None);

        cleanup_test_env();
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_solana_network_provider_with_custom_urls() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = create_test_solana_network("testnet");
        let custom_urls = vec![
            RpcConfig {
                url: "https://custom-rpc1.example.com".to_string(),
                weight: 1,
            },
            RpcConfig {
                url: "https://custom-rpc2.example.com".to_string(),
                weight: 1,
            },
        ];
        let result = get_network_provider(&network, Some(custom_urls));

        cleanup_test_env();
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_solana_network_provider_with_empty_custom_urls() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = create_test_solana_network("testnet");
        let custom_urls: Vec<RpcConfig> = vec![];
        let result = get_network_provider(&network, Some(custom_urls));

        cleanup_test_env();
        assert!(result.is_ok()); // Should fall back to public URLs
    }

    // Tests for Stellar Network Provider
    #[test]
    fn test_get_stellar_network_provider_valid_network_fallback_public() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = create_test_stellar_network();
        let result = get_network_provider(&network, None); // No custom URLs

        cleanup_test_env();
        assert!(result.is_ok()); // Should fall back to public URLs for testnet
                                 // StellarProvider::new will use the first public URL: https://soroban-testnet.stellar.org
    }

    #[test]
    fn test_get_stellar_network_provider_with_custom_urls() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = create_test_stellar_network();
        let custom_urls = vec![
            RpcConfig::new("https://custom-stellar-rpc1.example.com".to_string()),
            RpcConfig::with_weight("http://custom-stellar-rpc2.example.com".to_string(), 50)
                .unwrap(),
        ];
        let result = get_network_provider(&network, Some(custom_urls));

        cleanup_test_env();
        assert!(result.is_ok());
        // StellarProvider::new will pick custom-stellar-rpc1 (default weight 100) over custom-stellar-rpc2 (weight 50)
    }

    #[test]
    fn test_get_stellar_network_provider_with_empty_custom_urls_fallback() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = create_test_stellar_network();
        let custom_urls: Vec<RpcConfig> = vec![]; // Empty custom URLs
        let result = get_network_provider(&network, Some(custom_urls));

        cleanup_test_env();
        assert!(result.is_ok()); // Should fall back to public URLs for mainnet
                                 // StellarProvider::new will use the first public URL: https://horizon.stellar.org
    }

    #[test]
    fn test_get_stellar_network_provider_custom_urls_with_zero_weight() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = create_test_stellar_network();
        let custom_urls = vec![
            RpcConfig::with_weight("http://zero-weight-rpc.example.com".to_string(), 0).unwrap(),
            RpcConfig::new("http://active-rpc.example.com".to_string()), // Default weight 100
        ];
        let result = get_network_provider(&network, Some(custom_urls));
        cleanup_test_env();
        assert!(result.is_ok()); // active-rpc should be chosen
    }

    #[test]
    fn test_get_stellar_network_provider_all_custom_urls_zero_weight_fallback() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = create_test_stellar_network();
        let custom_urls = vec![
            RpcConfig::with_weight("http://zero1.example.com".to_string(), 0).unwrap(),
            RpcConfig::with_weight("http://zero2.example.com".to_string(), 0).unwrap(),
        ];
        // Since StellarProvider::new filters out zero-weight URLs, and if the list becomes empty,
        // get_network_provider does NOT re-trigger fallback to public. Instead, StellarProvider::new itself will error.
        // The current get_network_provider logic passes the custom_urls to N::new_provider if Some and not empty.
        // If custom_urls becomes effectively empty *inside* N::new_provider (like StellarProvider::new after filtering weights),
        // then N::new_provider is responsible for erroring or handling.
        let result = get_network_provider(&network, Some(custom_urls));
        cleanup_test_env();
        assert!(result.is_err());
        match result.unwrap_err() {
            ProviderError::NetworkConfiguration(msg) => {
                assert!(msg.contains("No active RPC configurations provided"));
            }
            _ => panic!("Unexpected error type"),
        }
    }

    #[test]
    fn test_provider_error_rpc_error_code_variant() {
        let error = ProviderError::RpcErrorCode {
            code: -32000,
            message: "insufficient funds".to_string(),
        };
        let error_string = format!("{}", error);
        assert!(error_string.contains("-32000"));
        assert!(error_string.contains("insufficient funds"));
    }

    #[test]
    fn test_get_stellar_network_provider_invalid_custom_url_scheme() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();
        let network = create_test_stellar_network();
        let custom_urls = vec![RpcConfig::new("ftp://custom-ftp.example.com".to_string())];
        let result = get_network_provider(&network, Some(custom_urls));
        cleanup_test_env();
        assert!(result.is_err());
        match result.unwrap_err() {
            ProviderError::NetworkConfiguration(msg) => {
                // This error comes from RpcConfig::validate_list inside StellarProvider::new
                assert!(msg.contains("Invalid URL scheme"));
            }
            _ => panic!("Unexpected error type"),
        }
    }

    #[test]
    fn test_should_mark_provider_failed_server_errors() {
        // 5xx errors should mark provider as failed
        for status_code in 500..=599 {
            let error = ProviderError::RequestError {
                error: format!("Server error {}", status_code),
                status_code,
            };
            assert!(
                should_mark_provider_failed(&error),
                "Status code {} should mark provider as failed",
                status_code
            );
        }
    }

    #[test]
    fn test_should_mark_provider_failed_auth_errors() {
        // Authentication/authorization errors should mark provider as failed
        let auth_errors = [401, 403];
        for &status_code in &auth_errors {
            let error = ProviderError::RequestError {
                error: format!("Auth error {}", status_code),
                status_code,
            };
            assert!(
                should_mark_provider_failed(&error),
                "Status code {} should mark provider as failed",
                status_code
            );
        }
    }

    #[test]
    fn test_should_mark_provider_failed_not_found_errors() {
        // 404 and 410 should mark provider as failed (endpoint issues)
        let not_found_errors = [404, 410];
        for &status_code in &not_found_errors {
            let error = ProviderError::RequestError {
                error: format!("Not found error {}", status_code),
                status_code,
            };
            assert!(
                should_mark_provider_failed(&error),
                "Status code {} should mark provider as failed",
                status_code
            );
        }
    }

    #[test]
    fn test_should_mark_provider_failed_client_errors_not_failed() {
        // These 4xx errors should NOT mark provider as failed (client-side issues)
        let client_errors = [400, 405, 413, 414, 415, 422, 429];
        for &status_code in &client_errors {
            let error = ProviderError::RequestError {
                error: format!("Client error {}", status_code),
                status_code,
            };
            assert!(
                !should_mark_provider_failed(&error),
                "Status code {} should NOT mark provider as failed",
                status_code
            );
        }
    }

    #[test]
    fn test_should_mark_provider_failed_other_error_types() {
        // Test non-RequestError types - these should NOT mark provider as failed
        let errors = [
            ProviderError::Timeout,
            ProviderError::RateLimited,
            ProviderError::BadGateway,
            ProviderError::InvalidAddress("test".to_string()),
            ProviderError::NetworkConfiguration("test".to_string()),
            ProviderError::Other("test".to_string()),
        ];

        for error in errors {
            assert!(
                !should_mark_provider_failed(&error),
                "Error type {:?} should NOT mark provider as failed",
                error
            );
        }
    }

    #[test]
    fn test_should_mark_provider_failed_edge_cases() {
        // Test some edge case status codes
        let edge_cases = [
            (200, false), // Success - shouldn't happen in error context but test anyway
            (300, false), // Redirection
            (418, false), // I'm a teapot - should not mark as failed
            (451, false), // Unavailable for legal reasons - client issue
            (499, false), // Client closed request - client issue
        ];

        for (status_code, should_fail) in edge_cases {
            let error = ProviderError::RequestError {
                error: format!("Edge case error {}", status_code),
                status_code,
            };
            assert_eq!(
                should_mark_provider_failed(&error),
                should_fail,
                "Status code {} should {} mark provider as failed",
                status_code,
                if should_fail { "" } else { "NOT" }
            );
        }
    }

    #[test]
    fn test_is_retriable_error_retriable_types() {
        // These error types should be retriable
        let retriable_errors = [
            ProviderError::Timeout,
            ProviderError::RateLimited,
            ProviderError::BadGateway,
            ProviderError::TransportError("test".to_string()),
        ];

        for error in retriable_errors {
            assert!(
                is_retriable_error(&error),
                "Error type {:?} should be retriable",
                error
            );
        }
    }

    #[test]
    fn test_is_retriable_error_non_retriable_types() {
        // These error types should NOT be retriable
        let non_retriable_errors = [
            ProviderError::InvalidAddress("test".to_string()),
            ProviderError::NetworkConfiguration("test".to_string()),
            ProviderError::RequestError {
                error: "Some error".to_string(),
                status_code: 400,
            },
        ];

        for error in non_retriable_errors {
            assert!(
                !is_retriable_error(&error),
                "Error type {:?} should NOT be retriable",
                error
            );
        }
    }

    #[test]
    fn test_is_retriable_error_message_based_detection() {
        // Test errors that should be retriable based on message content
        let retriable_messages = [
            "Connection timeout occurred",
            "Network connection reset",
            "Connection refused",
            "TIMEOUT error happened",
            "Connection was reset by peer",
        ];

        for message in retriable_messages {
            let error = ProviderError::Other(message.to_string());
            assert!(
                is_retriable_error(&error),
                "Error with message '{}' should be retriable",
                message
            );
        }
    }

    #[test]
    fn test_is_retriable_error_message_based_non_retriable() {
        // Test errors that should NOT be retriable based on message content
        let non_retriable_messages = [
            "Invalid address format",
            "Bad request parameters",
            "Authentication failed",
            "Method not found",
            "Some other error",
        ];

        for message in non_retriable_messages {
            let error = ProviderError::Other(message.to_string());
            assert!(
                !is_retriable_error(&error),
                "Error with message '{}' should NOT be retriable",
                message
            );
        }
    }

    #[test]
    fn test_is_retriable_error_case_insensitive() {
        // Test that message-based detection is case insensitive
        let case_variations = [
            "TIMEOUT",
            "Timeout",
            "timeout",
            "CONNECTION",
            "Connection",
            "connection",
            "RESET",
            "Reset",
            "reset",
        ];

        for message in case_variations {
            let error = ProviderError::Other(message.to_string());
            assert!(
                is_retriable_error(&error),
                "Error with message '{}' should be retriable (case insensitive)",
                message
            );
        }
    }

    #[test]
    fn test_is_retriable_error_request_error_retriable_5xx() {
        // Test retriable 5xx status codes
        let retriable_5xx = vec![
            (500, "Internal Server Error"),
            (502, "Bad Gateway"),
            (503, "Service Unavailable"),
            (504, "Gateway Timeout"),
            (506, "Variant Also Negotiates"),
            (507, "Insufficient Storage"),
            (508, "Loop Detected"),
            (510, "Not Extended"),
            (511, "Network Authentication Required"),
            (599, "Network Connect Timeout Error"),
        ];

        for (status_code, description) in retriable_5xx {
            let error = ProviderError::RequestError {
                error: description.to_string(),
                status_code,
            };
            assert!(
                is_retriable_error(&error),
                "Status code {} ({}) should be retriable",
                status_code,
                description
            );
        }
    }

    #[test]
    fn test_is_retriable_error_request_error_non_retriable_5xx() {
        // Test non-retriable 5xx status codes (persistent server issues)
        let non_retriable_5xx = vec![
            (501, "Not Implemented"),
            (505, "HTTP Version Not Supported"),
        ];

        for (status_code, description) in non_retriable_5xx {
            let error = ProviderError::RequestError {
                error: description.to_string(),
                status_code,
            };
            assert!(
                !is_retriable_error(&error),
                "Status code {} ({}) should NOT be retriable",
                status_code,
                description
            );
        }
    }

    #[test]
    fn test_is_retriable_error_request_error_retriable_4xx() {
        // Test retriable 4xx status codes (timeout/rate-limit related)
        let retriable_4xx = vec![
            (408, "Request Timeout"),
            (425, "Too Early"),
            (429, "Too Many Requests"),
        ];

        for (status_code, description) in retriable_4xx {
            let error = ProviderError::RequestError {
                error: description.to_string(),
                status_code,
            };
            assert!(
                is_retriable_error(&error),
                "Status code {} ({}) should be retriable",
                status_code,
                description
            );
        }
    }

    #[test]
    fn test_is_retriable_error_request_error_non_retriable_4xx() {
        // Test non-retriable 4xx status codes (client errors)
        let non_retriable_4xx = vec![
            (400, "Bad Request"),
            (401, "Unauthorized"),
            (403, "Forbidden"),
            (404, "Not Found"),
            (405, "Method Not Allowed"),
            (406, "Not Acceptable"),
            (407, "Proxy Authentication Required"),
            (409, "Conflict"),
            (410, "Gone"),
            (411, "Length Required"),
            (412, "Precondition Failed"),
            (413, "Payload Too Large"),
            (414, "URI Too Long"),
            (415, "Unsupported Media Type"),
            (416, "Range Not Satisfiable"),
            (417, "Expectation Failed"),
            (418, "I'm a teapot"),
            (421, "Misdirected Request"),
            (422, "Unprocessable Entity"),
            (423, "Locked"),
            (424, "Failed Dependency"),
            (426, "Upgrade Required"),
            (428, "Precondition Required"),
            (431, "Request Header Fields Too Large"),
            (451, "Unavailable For Legal Reasons"),
            (499, "Client Closed Request"),
        ];

        for (status_code, description) in non_retriable_4xx {
            let error = ProviderError::RequestError {
                error: description.to_string(),
                status_code,
            };
            assert!(
                !is_retriable_error(&error),
                "Status code {} ({}) should NOT be retriable",
                status_code,
                description
            );
        }
    }

    #[test]
    fn test_is_retriable_error_request_error_other_status_codes() {
        // Test other status codes (1xx, 2xx, 3xx) - should not be retriable
        let other_status_codes = vec![
            (100, "Continue"),
            (101, "Switching Protocols"),
            (200, "OK"),
            (201, "Created"),
            (204, "No Content"),
            (300, "Multiple Choices"),
            (301, "Moved Permanently"),
            (302, "Found"),
            (304, "Not Modified"),
            (600, "Custom status"),
            (999, "Unknown status"),
        ];

        for (status_code, description) in other_status_codes {
            let error = ProviderError::RequestError {
                error: description.to_string(),
                status_code,
            };
            assert!(
                !is_retriable_error(&error),
                "Status code {} ({}) should NOT be retriable",
                status_code,
                description
            );
        }
    }

    #[test]
    fn test_is_retriable_error_request_error_boundary_cases() {
        // Test boundary cases for our ranges
        let test_cases = vec![
            // Just before retriable 4xx range
            (407, false, "Proxy Authentication Required"),
            (408, true, "Request Timeout - first retriable 4xx"),
            (409, false, "Conflict"),
            // Around 425
            (424, false, "Failed Dependency"),
            (425, true, "Too Early"),
            (426, false, "Upgrade Required"),
            // Around 429
            (428, false, "Precondition Required"),
            (429, true, "Too Many Requests"),
            (430, false, "Would be non-retriable if it existed"),
            // 5xx boundaries
            (499, false, "Last 4xx"),
            (500, true, "First 5xx - retriable"),
            (501, false, "Not Implemented - exception"),
            (502, true, "Bad Gateway - retriable"),
            (505, false, "HTTP Version Not Supported - exception"),
            (506, true, "First after 505 exception"),
            (599, true, "Last defined 5xx"),
        ];

        for (status_code, should_be_retriable, description) in test_cases {
            let error = ProviderError::RequestError {
                error: description.to_string(),
                status_code,
            };
            assert_eq!(
                is_retriable_error(&error),
                should_be_retriable,
                "Status code {} ({}) should{} be retriable",
                status_code,
                description,
                if should_be_retriable { "" } else { " NOT" }
            );
        }
    }
}
