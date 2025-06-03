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
    #[error("Other provider error: {0}")]
    Other(String),
}

impl From<hex::FromHexError> for ProviderError {
    fn from(err: hex::FromHexError) -> Self {
        ProviderError::InvalidAddress(err.to_string())
    }
}

impl From<std::net::AddrParseError> for ProviderError {
    fn from(err: std::net::AddrParseError) -> Self {
        ProviderError::NetworkConfiguration(format!("Invalid network address: {}", err))
    }
}

impl From<ParseIntError> for ProviderError {
    fn from(err: ParseIntError) -> Self {
        ProviderError::Other(format!("Number parsing error: {}", err))
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

                // Fallback for other transport error types
                ProviderError::Other(format!("Transport error: {}", transport_err))
            }
            RpcError::ErrorResp(json_rpc_err) => ProviderError::Other(format!(
                "JSON-RPC error ({}): {}",
                json_rpc_err.code, json_rpc_err.message
            )),
            _ => ProviderError::Other(format!("Other RPC error: {}", err)),
        }
    }
}

// Implement From for RpcSelectorError
impl From<super::rpc_selector::RpcSelectorError> for ProviderError {
    fn from(err: super::rpc_selector::RpcSelectorError) -> Self {
        ProviderError::NetworkConfiguration(format!("RPC selector error: {}", err))
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

#[cfg(test)]
mod tests {
    use super::*;
    use lazy_static::lazy_static;
    use std::env;
    use std::sync::Mutex;
    use std::time::Duration;
    use wiremock::matchers::any;
    use wiremock::{Mock, MockServer, ResponseTemplate};

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
        let mock_server = MockServer::start().await;

        Mock::given(any())
            .respond_with(ResponseTemplate::new(429))
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let response = client
            .get(mock_server.uri())
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
        let mock_server = MockServer::start().await;

        Mock::given(any())
            .respond_with(ResponseTemplate::new(502))
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let response = client
            .get(mock_server.uri())
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
}
