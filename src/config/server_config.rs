/// Configuration for the server, including network and rate limiting settings.
use std::{env, str::FromStr};
use strum::Display;

use crate::{
    constants::{
        DEFAULT_PROVIDER_FAILURE_EXPIRATION_SECS, DEFAULT_PROVIDER_FAILURE_THRESHOLD,
        DEFAULT_PROVIDER_PAUSE_DURATION_SECS, MINIMUM_SECRET_VALUE_LENGTH,
        STELLAR_FEE_FORWARDER_MAINNET, STELLAR_SOROSWAP_MAINNET_FACTORY,
        STELLAR_SOROSWAP_MAINNET_NATIVE_WRAPPER, STELLAR_SOROSWAP_MAINNET_ROUTER,
    },
    models::SecretString,
};

#[derive(Debug, Clone, PartialEq, Eq, Display)]
pub enum RepositoryStorageType {
    InMemory,
    Redis,
}

impl FromStr for RepositoryStorageType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "inmemory" | "in_memory" => Ok(Self::InMemory),
            "redis" => Ok(Self::Redis),
            _ => Err(format!("Invalid repository storage type: {s}")),
        }
    }
}

/// Returns `Some(s.to_string())` when `s` is non-empty, `None` otherwise.
fn non_empty_const(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// The host address the server will bind to.
    pub host: String,
    /// The port number the server will listen on.
    pub port: u16,
    /// The URL for the Redis primary instance (used for write operations).
    pub redis_url: String,
    /// Optional URL for Redis reader endpoint (used for read operations).
    /// When set, read operations use this endpoint while writes use `redis_url`.
    /// Useful for AWS ElastiCache with read replicas.
    pub redis_reader_url: Option<String>,
    /// The file path to the server's configuration file.
    pub config_file_path: String,
    /// The API key used for authentication.
    pub api_key: SecretString,
    /// The number of requests allowed per second.
    pub rate_limit_requests_per_second: u64,
    /// The maximum burst size for rate limiting.
    pub rate_limit_burst_size: u32,
    /// The port number for exposing metrics.
    pub metrics_port: u16,
    /// Enable Swagger UI.
    pub enable_swagger: bool,
    /// The number of seconds to wait for a Redis connection.
    pub redis_connection_timeout_ms: u64,
    /// The prefix for the Redis key.
    pub redis_key_prefix: String,
    /// Maximum number of connections in the Redis pool.
    pub redis_pool_max_size: usize,
    /// Maximum pool size for reader connections. Defaults to 1000.
    /// Useful for read-heavy workloads where more reader connections are beneficial.
    pub redis_reader_pool_max_size: usize,
    /// Timeout in milliseconds waiting to get a connection from the pool.
    pub redis_pool_timeout_ms: u64,
    /// The number of milliseconds to wait for an RPC response.
    pub rpc_timeout_ms: u64,
    /// Maximum number of retry attempts for provider operations.
    pub provider_max_retries: u8,
    /// Base delay between retry attempts (milliseconds).
    pub provider_retry_base_delay_ms: u64,
    /// Maximum delay between retry attempts (milliseconds).
    pub provider_retry_max_delay_ms: u64,
    /// Maximum number of failovers (switching to different providers).
    pub provider_max_failovers: u8,
    /// Number of consecutive failures before pausing a provider.
    pub provider_failure_threshold: u32,
    /// Duration in seconds to pause a provider after reaching failure threshold.
    pub provider_pause_duration_secs: u64,
    /// Duration in seconds after which failures are considered stale and reset.
    pub provider_failure_expiration_secs: u64,
    /// The type of repository storage to use.
    pub repository_storage_type: RepositoryStorageType,
    /// Flag to force config file processing.
    pub reset_storage_on_start: bool,
    /// The encryption key for the storage.
    pub storage_encryption_key: Option<SecretString>,
    /// Transaction expiration time in hours for transactions in final states.
    /// Supports fractional values (e.g., 0.1 = 6 minutes).
    pub transaction_expiration_hours: f64,
    /// Comma-separated list of allowed RPC hosts (domains or IPs). If non-empty, only these hosts are permitted.
    pub rpc_allowed_hosts: Vec<String>,
    /// If true, block private IP addresses (RFC 1918, loopback, link-local). Cloud metadata endpoints are always blocked.
    pub rpc_block_private_ips: bool,
    /// Maximum number of concurrent requests allowed for /api/v1/relayers/* endpoints.
    pub relayer_concurrency_limit: usize,
    /// Maximum number of concurrent TCP connections server-wide.
    pub max_connections: usize,
    /// TCP listen connection backlog size (pending connections queue).
    /// Higher values allow more connections to be queued during traffic bursts.
    pub connection_backlog: u32,
    /// Request handler timeout in seconds for API endpoints.
    pub request_timeout_seconds: u64,
    /// Stellar mainnet FeeForwarder contract address for gas abstraction.
    pub stellar_mainnet_fee_forwarder_address: Option<String>,
    /// Stellar testnet FeeForwarder contract address for gas abstraction.
    pub stellar_testnet_fee_forwarder_address: Option<String>,
    /// Stellar mainnet Soroswap router contract address.
    pub stellar_mainnet_soroswap_router_address: Option<String>,
    /// Stellar testnet Soroswap router contract address.
    pub stellar_testnet_soroswap_router_address: Option<String>,
    /// Stellar mainnet Soroswap factory contract address.
    pub stellar_mainnet_soroswap_factory_address: Option<String>,
    /// Stellar testnet Soroswap factory contract address.
    pub stellar_testnet_soroswap_factory_address: Option<String>,
    /// Stellar mainnet native XLM wrapper token address for Soroswap.
    pub stellar_mainnet_soroswap_native_wrapper_address: Option<String>,
    /// Stellar testnet native XLM wrapper token address for Soroswap.
    pub stellar_testnet_soroswap_native_wrapper_address: Option<String>,
}

impl ServerConfig {
    /// Creates a new `ServerConfig` instance from environment variables.
    ///
    /// # Panics
    ///
    /// This function will panic if the `REDIS_URL` or `API_KEY` environment
    /// variables are not set, as they are required for the server to function.
    ///
    /// # Defaults
    ///
    /// - `HOST` defaults to `"0.0.0.0"`.
    /// - `APP_PORT` defaults to `8080`.
    /// - `CONFIG_DIR` defaults to `"config/config.json"`.
    /// - `RATE_LIMIT_REQUESTS_PER_SECOND` defaults to `100`.
    /// - `RATE_LIMIT_BURST_SIZE` defaults to `300`.
    /// - `METRICS_PORT` defaults to `8081`.
    /// - `PROVIDER_MAX_RETRIES` defaults to `3`.
    /// - `PROVIDER_RETRY_BASE_DELAY_MS` defaults to `100`.
    /// - `PROVIDER_RETRY_MAX_DELAY_MS` defaults to `2000`.
    /// - `PROVIDER_MAX_FAILOVERS` defaults to `3`.
    /// - `PROVIDER_FAILURE_THRESHOLD` defaults to `3`.
    /// - `PROVIDER_PAUSE_DURATION_SECS` defaults to `60` (1 minute).
    /// - `PROVIDER_FAILURE_EXPIRATION_SECS` defaults to `60` (1 minute).
    /// - `REPOSITORY_STORAGE_TYPE` defaults to `"in_memory"`.
    /// - `TRANSACTION_EXPIRATION_HOURS` defaults to `4`.
    /// - `REQUEST_TIMEOUT_SECONDS` defaults to `30` (security measure for DoS protection).
    /// - `CONNECTION_BACKLOG` defaults to `511` (production-ready value for traffic bursts).
    pub fn from_env() -> Self {
        Self {
            host: Self::get_host(),
            port: Self::get_port(),
            redis_url: Self::get_redis_url(), // Uses panicking version as required
            redis_reader_url: Self::get_redis_reader_url_optional(),
            redis_reader_pool_max_size: Self::get_redis_reader_pool_max_size(),
            config_file_path: Self::get_config_file_path(),
            api_key: Self::get_api_key(), // Uses panicking version as required
            rate_limit_requests_per_second: Self::get_rate_limit_requests_per_second(),
            rate_limit_burst_size: Self::get_rate_limit_burst_size(),
            metrics_port: Self::get_metrics_port(),
            enable_swagger: Self::get_enable_swagger(),
            redis_connection_timeout_ms: Self::get_redis_connection_timeout_ms(),
            redis_key_prefix: Self::get_redis_key_prefix(),
            redis_pool_max_size: Self::get_redis_pool_max_size(),
            redis_pool_timeout_ms: Self::get_redis_pool_timeout_ms(),
            rpc_timeout_ms: Self::get_rpc_timeout_ms(),
            provider_max_retries: Self::get_provider_max_retries(),
            provider_retry_base_delay_ms: Self::get_provider_retry_base_delay_ms(),
            provider_retry_max_delay_ms: Self::get_provider_retry_max_delay_ms(),
            provider_max_failovers: Self::get_provider_max_failovers(),
            provider_failure_threshold: Self::get_provider_failure_threshold(),
            provider_pause_duration_secs: Self::get_provider_pause_duration_secs(),
            provider_failure_expiration_secs: Self::get_provider_failure_expiration_secs(),
            repository_storage_type: Self::get_repository_storage_type(),
            reset_storage_on_start: Self::get_reset_storage_on_start(),
            storage_encryption_key: Self::get_storage_encryption_key(),
            transaction_expiration_hours: Self::get_transaction_expiration_hours(),
            rpc_allowed_hosts: Self::get_rpc_allowed_hosts(),
            rpc_block_private_ips: Self::get_rpc_block_private_ips(),
            relayer_concurrency_limit: Self::get_relayer_concurrency_limit(),
            max_connections: Self::get_max_connections(),
            connection_backlog: Self::get_connection_backlog(),
            request_timeout_seconds: Self::get_request_timeout_seconds(),
            stellar_mainnet_fee_forwarder_address: Self::get_stellar_mainnet_fee_forwarder_address(
            ),
            stellar_testnet_fee_forwarder_address: Self::get_stellar_testnet_fee_forwarder_address(
            ),
            stellar_mainnet_soroswap_router_address:
                Self::get_stellar_mainnet_soroswap_router_address(),
            stellar_testnet_soroswap_router_address:
                Self::get_stellar_testnet_soroswap_router_address(),
            stellar_mainnet_soroswap_factory_address:
                Self::get_stellar_mainnet_soroswap_factory_address(),
            stellar_testnet_soroswap_factory_address:
                Self::get_stellar_testnet_soroswap_factory_address(),
            stellar_mainnet_soroswap_native_wrapper_address:
                Self::get_stellar_mainnet_soroswap_native_wrapper_address(),
            stellar_testnet_soroswap_native_wrapper_address:
                Self::get_stellar_testnet_soroswap_native_wrapper_address(),
        }
    }

    // Individual getter methods for each configuration field

    /// Gets the host from environment variable or default
    pub fn get_host() -> String {
        env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string())
    }

    /// Gets the port from environment variable or default
    pub fn get_port() -> u16 {
        env::var("APP_PORT")
            .unwrap_or_else(|_| "8080".to_string())
            .parse()
            .unwrap_or(8080)
    }

    /// Gets the Redis URL from environment variable (panics if not set)
    pub fn get_redis_url() -> String {
        env::var("REDIS_URL").expect("REDIS_URL must be set")
    }

    /// Gets the Redis URL from environment variable or returns None if not set
    pub fn get_redis_url_optional() -> Option<String> {
        env::var("REDIS_URL").ok()
    }

    /// Gets the Redis reader URL from environment variable or returns None if not set.
    /// When set, read operations will use this endpoint while writes use REDIS_URL.
    /// Useful for AWS ElastiCache with read replicas.
    pub fn get_redis_reader_url_optional() -> Option<String> {
        env::var("REDIS_READER_URL").ok()
    }

    /// Gets the config file path from environment variables or default
    pub fn get_config_file_path() -> String {
        let conf_dir = if env::var("IN_DOCKER")
            .map(|val| val == "true")
            .unwrap_or(false)
        {
            "config/".to_string()
        } else {
            env::var("CONFIG_DIR").unwrap_or_else(|_| "./config".to_string())
        };

        let conf_dir = format!("{}/", conf_dir.trim_end_matches('/'));
        let config_file_name =
            env::var("CONFIG_FILE_NAME").unwrap_or_else(|_| "config.json".to_string());

        format!("{conf_dir}{config_file_name}")
    }

    /// Gets the queue backend from environment variable or default.
    ///
    /// Supported values: "redis", "sqs"
    /// Defaults to "redis" when not set.
    pub fn get_queue_backend() -> String {
        env::var("QUEUE_BACKEND").unwrap_or_else(|_| "redis".to_string())
    }

    /// Gets the AWS region from environment variable.
    ///
    /// Required when using SQS queue backend.
    ///
    /// # Errors
    ///
    /// Returns error if AWS_REGION is not set.
    pub fn get_aws_region() -> Result<String, String> {
        env::var("AWS_REGION")
            .map_err(|_| "AWS_REGION not set. Required for SQS backend.".to_string())
    }

    /// Gets the AWS account ID from environment variable.
    ///
    /// Required when using SQS queue backend and SQS_QUEUE_URL_PREFIX is not provided.
    ///
    /// # Errors
    ///
    /// Returns error if AWS_ACCOUNT_ID is not set.
    pub fn get_aws_account_id() -> Result<String, String> {
        env::var("AWS_ACCOUNT_ID").map_err(|_| {
            "AWS_ACCOUNT_ID not set. Required when SQS_QUEUE_URL_PREFIX is not provided."
                .to_string()
        })
    }

    /// Gets the API key from environment variable (panics if not set or too short)
    pub fn get_api_key() -> SecretString {
        let api_key = SecretString::new(&env::var("API_KEY").expect("API_KEY must be set"));

        if !api_key.has_minimum_length(MINIMUM_SECRET_VALUE_LENGTH) {
            panic!(
                "Security error: API_KEY must be at least {MINIMUM_SECRET_VALUE_LENGTH} characters long"
            );
        }

        api_key
    }

    /// Gets the API key from environment variable or returns None if not set or invalid
    pub fn get_api_key_optional() -> Option<SecretString> {
        env::var("API_KEY")
            .ok()
            .map(|key| SecretString::new(&key))
            .filter(|key| key.has_minimum_length(MINIMUM_SECRET_VALUE_LENGTH))
    }

    /// Gets the rate limit requests per second from environment variable or default
    pub fn get_rate_limit_requests_per_second() -> u64 {
        env::var("RATE_LIMIT_REQUESTS_PER_SECOND")
            .unwrap_or_else(|_| "100".to_string())
            .parse()
            .unwrap_or(100)
    }

    /// Gets the rate limit burst size from environment variable or default
    pub fn get_rate_limit_burst_size() -> u32 {
        env::var("RATE_LIMIT_BURST_SIZE")
            .unwrap_or_else(|_| "300".to_string())
            .parse()
            .unwrap_or(300)
    }

    /// Gets the metrics port from environment variable or default
    pub fn get_metrics_port() -> u16 {
        env::var("METRICS_PORT")
            .unwrap_or_else(|_| "8081".to_string())
            .parse()
            .unwrap_or(8081)
    }

    /// Gets the enable swagger setting from environment variable or default
    pub fn get_enable_swagger() -> bool {
        env::var("ENABLE_SWAGGER")
            .map(|v| v.to_lowercase() == "true")
            .unwrap_or(false)
    }

    /// Gets the Redis connection timeout from environment variable or default
    pub fn get_redis_connection_timeout_ms() -> u64 {
        env::var("REDIS_CONNECTION_TIMEOUT_MS")
            .unwrap_or_else(|_| "10000".to_string())
            .parse()
            .unwrap_or(10000)
    }

    /// Gets the Redis key prefix from environment variable or default
    pub fn get_redis_key_prefix() -> String {
        env::var("REDIS_KEY_PREFIX").unwrap_or_else(|_| "oz-relayer".to_string())
    }

    /// Gets the Redis pool max size from environment variable or default
    /// Returns default (500) if value is 0 or invalid
    pub fn get_redis_pool_max_size() -> usize {
        env::var("REDIS_POOL_MAX_SIZE")
            .unwrap_or_else(|_| "500".to_string())
            .parse()
            .ok()
            .filter(|&v| v > 0)
            .unwrap_or(500)
    }

    /// Gets the Redis reader pool max size from environment variable.
    /// Returns 1000 if not set or invalid.
    pub fn get_redis_reader_pool_max_size() -> usize {
        env::var("REDIS_READER_POOL_MAX_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&v| v > 0)
            .unwrap_or(1000)
    }

    /// Gets the Redis pool timeout from environment variable or default
    /// Returns default (10000) if value is 0 or invalid
    pub fn get_redis_pool_timeout_ms() -> u64 {
        env::var("REDIS_POOL_TIMEOUT_MS")
            .unwrap_or_else(|_| "10000".to_string())
            .parse()
            .ok()
            .filter(|&v| v > 0)
            .unwrap_or(10000)
    }

    /// Gets the RPC timeout from environment variable or default
    pub fn get_rpc_timeout_ms() -> u64 {
        env::var("RPC_TIMEOUT_MS")
            .unwrap_or_else(|_| "10000".to_string())
            .parse()
            .unwrap_or(10000)
    }

    /// Gets the provider max retries from environment variable or default
    pub fn get_provider_max_retries() -> u8 {
        env::var("PROVIDER_MAX_RETRIES")
            .unwrap_or_else(|_| "3".to_string())
            .parse()
            .unwrap_or(3)
    }

    /// Gets the provider retry base delay from environment variable or default
    pub fn get_provider_retry_base_delay_ms() -> u64 {
        env::var("PROVIDER_RETRY_BASE_DELAY_MS")
            .unwrap_or_else(|_| "100".to_string())
            .parse()
            .unwrap_or(100)
    }

    /// Gets the provider retry max delay from environment variable or default
    pub fn get_provider_retry_max_delay_ms() -> u64 {
        env::var("PROVIDER_RETRY_MAX_DELAY_MS")
            .unwrap_or_else(|_| "2000".to_string())
            .parse()
            .unwrap_or(2000)
    }

    /// Gets the provider max failovers from environment variable or default
    pub fn get_provider_max_failovers() -> u8 {
        env::var("PROVIDER_MAX_FAILOVERS")
            .unwrap_or_else(|_| "3".to_string())
            .parse()
            .unwrap_or(3)
    }

    /// Gets the provider failure threshold from environment variable or default
    pub fn get_provider_failure_threshold() -> u32 {
        env::var("PROVIDER_FAILURE_THRESHOLD")
            .or_else(|_| env::var("RPC_FAILURE_THRESHOLD")) // Support legacy env var
            .unwrap_or_else(|_| DEFAULT_PROVIDER_FAILURE_THRESHOLD.to_string())
            .parse()
            .unwrap_or(DEFAULT_PROVIDER_FAILURE_THRESHOLD)
    }

    /// Gets the provider pause duration in seconds from environment variable or default
    ///
    /// Defaults to 60 seconds (1 minute) for faster recovery while still providing
    /// a reasonable cooldown period for failed providers.
    pub fn get_provider_pause_duration_secs() -> u64 {
        env::var("PROVIDER_PAUSE_DURATION_SECS")
            .or_else(|_| env::var("RPC_PAUSE_DURATION_SECS")) // Support legacy env var
            .unwrap_or_else(|_| DEFAULT_PROVIDER_PAUSE_DURATION_SECS.to_string())
            .parse()
            .unwrap_or(DEFAULT_PROVIDER_PAUSE_DURATION_SECS)
    }

    /// Gets the provider failure expiration duration in seconds from environment variable or default
    ///
    /// Defaults to 60 seconds (1 minute). Failures older than this are considered stale
    /// and reset, allowing providers to naturally recover over time.
    pub fn get_provider_failure_expiration_secs() -> u64 {
        env::var("PROVIDER_FAILURE_EXPIRATION_SECS")
            .unwrap_or_else(|_| DEFAULT_PROVIDER_FAILURE_EXPIRATION_SECS.to_string())
            .parse()
            .unwrap_or(DEFAULT_PROVIDER_FAILURE_EXPIRATION_SECS)
    }

    /// Gets the repository storage type from environment variable or default
    pub fn get_repository_storage_type() -> RepositoryStorageType {
        env::var("REPOSITORY_STORAGE_TYPE")
            .unwrap_or_else(|_| "in_memory".to_string())
            .parse()
            .unwrap_or(RepositoryStorageType::InMemory)
    }

    /// Gets the reset storage on start setting from environment variable or default
    pub fn get_reset_storage_on_start() -> bool {
        env::var("RESET_STORAGE_ON_START")
            .map(|v| v.to_lowercase() == "true")
            .unwrap_or(false)
    }

    /// Gets the storage encryption key from environment variable or None
    pub fn get_storage_encryption_key() -> Option<SecretString> {
        env::var("STORAGE_ENCRYPTION_KEY")
            .map(|v| SecretString::new(&v))
            .ok()
    }

    /// Gets the transaction expiration hours from environment variable or default
    /// Supports fractional values (e.g., 0.1 = 6 minutes).
    pub fn get_transaction_expiration_hours() -> f64 {
        env::var("TRANSACTION_EXPIRATION_HOURS")
            .unwrap_or_else(|_| "4".to_string())
            .parse()
            .unwrap_or(4.0)
    }

    /// Gets the allowed RPC hosts from environment variable or default (empty list)
    pub fn get_rpc_allowed_hosts() -> Vec<String> {
        env::var("RPC_ALLOWED_HOSTS")
            .ok()
            .map(|s| {
                s.split(',')
                    .map(|host| host.trim().to_string())
                    .filter(|host| !host.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Gets the block private IPs setting from environment variable or default (false)
    pub fn get_rpc_block_private_ips() -> bool {
        env::var("RPC_BLOCK_PRIVATE_IPS")
            .map(|v| v.to_lowercase() == "true")
            .unwrap_or(false)
    }

    /// Gets the relayer concurrency limit from environment variable or default (100)
    pub fn get_relayer_concurrency_limit() -> usize {
        env::var("RELAYER_CONCURRENCY_LIMIT")
            .unwrap_or_else(|_| "100".to_string())
            .parse()
            .unwrap_or(100)
    }

    /// Gets the max connections from environment variable or default (256)
    pub fn get_max_connections() -> usize {
        env::var("MAX_CONNECTIONS")
            .unwrap_or_else(|_| "256".to_string())
            .parse()
            .unwrap_or(256)
    }

    /// Gets the connection backlog from environment variable or default (511)
    ///
    /// TCP listen backlog controls the size of the queue for pending connections.
    /// Higher values allow more connections to be queued during traffic bursts,
    /// preventing connection drops. Default of 511.
    pub fn get_connection_backlog() -> u32 {
        env::var("CONNECTION_BACKLOG")
            .unwrap_or_else(|_| "511".to_string())
            .parse()
            .unwrap_or(511)
    }

    /// Gets the request timeout in seconds from environment variable or default (30)
    ///
    /// This is a security measure to prevent resource exhaustion attacks (DoS).
    /// It limits how long a request handler can run, preventing slowloris-style
    /// attacks and ensuring resources are freed promptly.
    pub fn get_request_timeout_seconds() -> u64 {
        env::var("REQUEST_TIMEOUT_SECONDS")
            .unwrap_or_else(|_| "30".to_string())
            .parse()
            .unwrap_or(30)
    }

    /// Gets whether distributed mode is enabled from the `DISTRIBUTED_MODE` environment variable.
    ///
    /// When `true`, distributed locks are used to coordinate across multiple instances
    /// (e.g., preventing duplicate cron execution in multi-instance deployments).
    /// When `false` (default), locks are skipped — appropriate for single-instance deployments.
    ///
    /// Defaults to `false`.
    pub fn get_distributed_mode() -> bool {
        env::var("DISTRIBUTED_MODE")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false)
    }

    // =========================================================================
    // Stellar Contract Address Getters (raw env var reads)
    // =========================================================================

    pub fn get_stellar_mainnet_fee_forwarder_address() -> Option<String> {
        env::var("STELLAR_MAINNET_FEE_FORWARDER_ADDRESS").ok()
    }

    pub fn get_stellar_testnet_fee_forwarder_address() -> Option<String> {
        env::var("STELLAR_TESTNET_FEE_FORWARDER_ADDRESS").ok()
    }

    pub fn get_stellar_mainnet_soroswap_router_address() -> Option<String> {
        env::var("STELLAR_MAINNET_SOROSWAP_ROUTER_ADDRESS").ok()
    }

    pub fn get_stellar_testnet_soroswap_router_address() -> Option<String> {
        env::var("STELLAR_TESTNET_SOROSWAP_ROUTER_ADDRESS").ok()
    }

    pub fn get_stellar_mainnet_soroswap_factory_address() -> Option<String> {
        env::var("STELLAR_MAINNET_SOROSWAP_FACTORY_ADDRESS").ok()
    }

    pub fn get_stellar_testnet_soroswap_factory_address() -> Option<String> {
        env::var("STELLAR_TESTNET_SOROSWAP_FACTORY_ADDRESS").ok()
    }

    pub fn get_stellar_mainnet_soroswap_native_wrapper_address() -> Option<String> {
        env::var("STELLAR_MAINNET_SOROSWAP_NATIVE_WRAPPER_ADDRESS").ok()
    }

    pub fn get_stellar_testnet_soroswap_native_wrapper_address() -> Option<String> {
        env::var("STELLAR_TESTNET_SOROSWAP_NATIVE_WRAPPER_ADDRESS").ok()
    }

    // =========================================================================
    // Stellar Contract Address Resolvers
    // =========================================================================
    // For mainnet: env var override → hardcoded default from constants.
    // For testnet: env var only (no hardcoded defaults).

    /// Resolves the FeeForwarder contract address for the given network.
    pub fn resolve_stellar_fee_forwarder_address(is_testnet: bool) -> Option<String> {
        if is_testnet {
            Self::get_stellar_testnet_fee_forwarder_address()
        } else {
            Self::get_stellar_mainnet_fee_forwarder_address()
                .or_else(|| non_empty_const(STELLAR_FEE_FORWARDER_MAINNET))
        }
    }

    /// Resolves the Soroswap router contract address for the given network.
    pub fn resolve_stellar_soroswap_router_address(is_testnet: bool) -> Option<String> {
        if is_testnet {
            Self::get_stellar_testnet_soroswap_router_address()
        } else {
            Self::get_stellar_mainnet_soroswap_router_address()
                .or_else(|| Some(STELLAR_SOROSWAP_MAINNET_ROUTER.to_string()))
        }
    }

    /// Resolves the Soroswap factory contract address for the given network.
    pub fn resolve_stellar_soroswap_factory_address(is_testnet: bool) -> Option<String> {
        if is_testnet {
            Self::get_stellar_testnet_soroswap_factory_address()
        } else {
            Self::get_stellar_mainnet_soroswap_factory_address()
                .or_else(|| Some(STELLAR_SOROSWAP_MAINNET_FACTORY.to_string()))
        }
    }

    /// Resolves the Soroswap native wrapper token address for the given network.
    pub fn resolve_stellar_soroswap_native_wrapper_address(is_testnet: bool) -> Option<String> {
        if is_testnet {
            Self::get_stellar_testnet_soroswap_native_wrapper_address()
        } else {
            Self::get_stellar_mainnet_soroswap_native_wrapper_address()
                .or_else(|| Some(STELLAR_SOROSWAP_MAINNET_NATIVE_WRAPPER.to_string()))
        }
    }

    /// Get worker concurrency from environment variable or use default
    ///
    /// Environment variable format: `BACKGROUND_WORKER_{WORKER_NAME}_CONCURRENCY`
    /// Example: `BACKGROUND_WORKER_TRANSACTION_REQUEST_CONCURRENCY=20`
    pub fn get_worker_concurrency(worker_name: &str, default: usize) -> usize {
        let env_var = format!(
            "BACKGROUND_WORKER_{}_CONCURRENCY",
            worker_name.to_uppercase()
        );
        env::var(&env_var)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lazy_static::lazy_static;
    use std::env;
    use std::sync::Mutex;

    // Use a mutex to ensure tests don't run in parallel when modifying env vars
    lazy_static! {
        static ref ENV_MUTEX: Mutex<()> = Mutex::new(());
    }

    fn setup() {
        // Clear all environment variables first
        env::remove_var("HOST");
        env::remove_var("APP_PORT");
        env::remove_var("REDIS_URL");
        env::remove_var("CONFIG_DIR");
        env::remove_var("CONFIG_FILE_NAME");
        env::remove_var("CONFIG_FILE_PATH");
        env::remove_var("API_KEY");
        env::remove_var("RATE_LIMIT_REQUESTS_PER_SECOND");
        env::remove_var("RATE_LIMIT_BURST_SIZE");
        env::remove_var("METRICS_PORT");
        env::remove_var("REDIS_CONNECTION_TIMEOUT_MS");
        env::remove_var("RPC_TIMEOUT_MS");
        env::remove_var("PROVIDER_MAX_RETRIES");
        env::remove_var("PROVIDER_RETRY_BASE_DELAY_MS");
        env::remove_var("PROVIDER_RETRY_MAX_DELAY_MS");
        env::remove_var("PROVIDER_MAX_FAILOVERS");
        env::remove_var("REPOSITORY_STORAGE_TYPE");
        env::remove_var("RESET_STORAGE_ON_START");
        env::remove_var("TRANSACTION_EXPIRATION_HOURS");
        env::remove_var("REDIS_READER_URL");
        // Set required variables for most tests
        env::set_var("REDIS_URL", "redis://localhost:6379");
        env::set_var("API_KEY", "7EF1CB7C-5003-4696-B384-C72AF8C3E15D");
        env::set_var("REDIS_CONNECTION_TIMEOUT_MS", "5000");
    }

    #[test]
    fn test_default_values() {
        let _lock = match ENV_MUTEX.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        setup();

        let config = ServerConfig::from_env();

        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 8080);
        assert_eq!(config.redis_url, "redis://localhost:6379");
        assert_eq!(config.config_file_path, "./config/config.json");
        assert_eq!(
            config.api_key,
            SecretString::new("7EF1CB7C-5003-4696-B384-C72AF8C3E15D")
        );
        assert_eq!(config.rate_limit_requests_per_second, 100);
        assert_eq!(config.rate_limit_burst_size, 300);
        assert_eq!(config.metrics_port, 8081);
        assert_eq!(config.redis_connection_timeout_ms, 5000);
        assert_eq!(config.rpc_timeout_ms, 10000);
        assert_eq!(config.provider_max_retries, 3);
        assert_eq!(config.provider_retry_base_delay_ms, 100);
        assert_eq!(config.provider_retry_max_delay_ms, 2000);
        assert_eq!(config.provider_max_failovers, 3);
        assert_eq!(config.provider_failure_threshold, 3);
        assert_eq!(config.provider_pause_duration_secs, 60);
        assert_eq!(
            config.repository_storage_type,
            RepositoryStorageType::InMemory
        );
        assert!(!config.reset_storage_on_start);
        assert_eq!(config.transaction_expiration_hours, 4.0);
    }

    #[test]
    fn test_invalid_port_values() {
        let _lock = match ENV_MUTEX.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        setup();
        env::set_var("REDIS_URL", "redis://localhost:6379");
        env::set_var("API_KEY", "7EF1CB7C-5003-4696-B384-C72AF8C3E15D");
        env::set_var("APP_PORT", "not_a_number");
        env::set_var("METRICS_PORT", "also_not_a_number");
        env::set_var("RATE_LIMIT_REQUESTS_PER_SECOND", "invalid");
        env::set_var("RATE_LIMIT_BURST_SIZE", "invalid");
        env::set_var("REDIS_CONNECTION_TIMEOUT_MS", "invalid");
        env::set_var("RPC_TIMEOUT_MS", "invalid");
        env::set_var("PROVIDER_MAX_RETRIES", "invalid");
        env::set_var("PROVIDER_RETRY_BASE_DELAY_MS", "invalid");
        env::set_var("PROVIDER_RETRY_MAX_DELAY_MS", "invalid");
        env::set_var("PROVIDER_MAX_FAILOVERS", "invalid");
        env::set_var("REPOSITORY_STORAGE_TYPE", "invalid");
        env::set_var("RESET_STORAGE_ON_START", "invalid");
        env::set_var("TRANSACTION_EXPIRATION_HOURS", "invalid");
        let config = ServerConfig::from_env();

        // Should fall back to defaults when parsing fails
        assert_eq!(config.port, 8080);
        assert_eq!(config.metrics_port, 8081);
        assert_eq!(config.rate_limit_requests_per_second, 100);
        assert_eq!(config.rate_limit_burst_size, 300);
        assert_eq!(config.redis_connection_timeout_ms, 10000);
        assert_eq!(config.rpc_timeout_ms, 10000);
        assert_eq!(config.provider_max_retries, 3);
        assert_eq!(config.provider_retry_base_delay_ms, 100);
        assert_eq!(config.provider_retry_max_delay_ms, 2000);
        assert_eq!(config.provider_max_failovers, 3);
        assert_eq!(
            config.repository_storage_type,
            RepositoryStorageType::InMemory
        );
        assert!(!config.reset_storage_on_start);
        assert_eq!(config.transaction_expiration_hours, 4.0);
    }

    #[test]
    fn test_custom_values() {
        let _lock = match ENV_MUTEX.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        setup();

        env::set_var("HOST", "127.0.0.1");
        env::set_var("APP_PORT", "9090");
        env::set_var("REDIS_URL", "redis://custom:6379");
        env::set_var("CONFIG_DIR", "custom");
        env::set_var("CONFIG_FILE_NAME", "path.json");
        env::set_var("API_KEY", "7EF1CB7C-5003-4696-B384-C72AF8C3E15D");
        env::set_var("RATE_LIMIT_REQUESTS_PER_SECOND", "200");
        env::set_var("RATE_LIMIT_BURST_SIZE", "500");
        env::set_var("METRICS_PORT", "9091");
        env::set_var("REDIS_CONNECTION_TIMEOUT_MS", "10000");
        env::set_var("RPC_TIMEOUT_MS", "33333");
        env::set_var("PROVIDER_MAX_RETRIES", "5");
        env::set_var("PROVIDER_RETRY_BASE_DELAY_MS", "200");
        env::set_var("PROVIDER_RETRY_MAX_DELAY_MS", "3000");
        env::set_var("PROVIDER_MAX_FAILOVERS", "4");
        env::set_var("REPOSITORY_STORAGE_TYPE", "in_memory");
        env::set_var("RESET_STORAGE_ON_START", "true");
        env::set_var("TRANSACTION_EXPIRATION_HOURS", "6");
        let config = ServerConfig::from_env();

        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 9090);
        assert_eq!(config.redis_url, "redis://custom:6379");
        assert_eq!(config.config_file_path, "custom/path.json");
        assert_eq!(
            config.api_key,
            SecretString::new("7EF1CB7C-5003-4696-B384-C72AF8C3E15D")
        );
        assert_eq!(config.rate_limit_requests_per_second, 200);
        assert_eq!(config.rate_limit_burst_size, 500);
        assert_eq!(config.metrics_port, 9091);
        assert_eq!(config.redis_connection_timeout_ms, 10000);
        assert_eq!(config.rpc_timeout_ms, 33333);
        assert_eq!(config.provider_max_retries, 5);
        assert_eq!(config.provider_retry_base_delay_ms, 200);
        assert_eq!(config.provider_retry_max_delay_ms, 3000);
        assert_eq!(config.provider_max_failovers, 4);
        assert_eq!(
            config.repository_storage_type,
            RepositoryStorageType::InMemory
        );
        assert!(config.reset_storage_on_start);
        assert_eq!(config.transaction_expiration_hours, 6.0);
    }

    #[test]
    #[should_panic(expected = "Security error: API_KEY must be at least 32 characters long")]
    fn test_invalid_api_key_length() {
        let _lock = match ENV_MUTEX.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        setup();
        env::set_var("REDIS_URL", "redis://localhost:6379");
        env::set_var("API_KEY", "insufficient_length");
        env::set_var("APP_PORT", "8080");
        env::set_var("RATE_LIMIT_REQUESTS_PER_SECOND", "100");
        env::set_var("RATE_LIMIT_BURST_SIZE", "300");
        env::set_var("METRICS_PORT", "9091");
        env::set_var("TRANSACTION_EXPIRATION_HOURS", "4");

        let _ = ServerConfig::from_env();

        panic!("Test should have panicked before reaching here");
    }

    // Tests for individual getter methods
    #[test]
    fn test_individual_getters_with_defaults() {
        let _lock = match ENV_MUTEX.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        // Clear all environment variables to test defaults
        env::remove_var("HOST");
        env::remove_var("APP_PORT");
        env::remove_var("REDIS_URL");
        env::remove_var("CONFIG_DIR");
        env::remove_var("CONFIG_FILE_NAME");
        env::remove_var("API_KEY");
        env::remove_var("RATE_LIMIT_REQUESTS_PER_SECOND");
        env::remove_var("RATE_LIMIT_BURST_SIZE");
        env::remove_var("METRICS_PORT");
        env::remove_var("ENABLE_SWAGGER");
        env::remove_var("REDIS_CONNECTION_TIMEOUT_MS");
        env::remove_var("REDIS_KEY_PREFIX");
        env::remove_var("REDIS_READER_URL");
        env::remove_var("RPC_TIMEOUT_MS");
        env::remove_var("PROVIDER_MAX_RETRIES");
        env::remove_var("PROVIDER_RETRY_BASE_DELAY_MS");
        env::remove_var("PROVIDER_RETRY_MAX_DELAY_MS");
        env::remove_var("PROVIDER_MAX_FAILOVERS");
        env::remove_var("REPOSITORY_STORAGE_TYPE");
        env::remove_var("RESET_STORAGE_ON_START");
        env::remove_var("STORAGE_ENCRYPTION_KEY");
        env::remove_var("TRANSACTION_EXPIRATION_HOURS");
        env::remove_var("REDIS_POOL_MAX_SIZE");
        env::remove_var("REDIS_POOL_TIMEOUT_MS");

        // Test individual getters with defaults
        assert_eq!(ServerConfig::get_host(), "0.0.0.0");
        assert_eq!(ServerConfig::get_port(), 8080);
        assert_eq!(ServerConfig::get_redis_url_optional(), None);
        assert_eq!(ServerConfig::get_config_file_path(), "./config/config.json");
        assert_eq!(ServerConfig::get_api_key_optional(), None);
        assert_eq!(ServerConfig::get_rate_limit_requests_per_second(), 100);
        assert_eq!(ServerConfig::get_rate_limit_burst_size(), 300);
        assert_eq!(ServerConfig::get_metrics_port(), 8081);
        assert!(!ServerConfig::get_enable_swagger());
        assert_eq!(ServerConfig::get_redis_connection_timeout_ms(), 10000);
        assert_eq!(ServerConfig::get_redis_key_prefix(), "oz-relayer");
        assert_eq!(ServerConfig::get_rpc_timeout_ms(), 10000);
        assert_eq!(ServerConfig::get_provider_max_retries(), 3);
        assert_eq!(ServerConfig::get_provider_retry_base_delay_ms(), 100);
        assert_eq!(ServerConfig::get_provider_retry_max_delay_ms(), 2000);
        assert_eq!(ServerConfig::get_provider_max_failovers(), 3);
        assert_eq!(
            ServerConfig::get_repository_storage_type(),
            RepositoryStorageType::InMemory
        );
        assert!(!ServerConfig::get_reset_storage_on_start());
        assert!(ServerConfig::get_storage_encryption_key().is_none());
        assert_eq!(ServerConfig::get_transaction_expiration_hours(), 4.0);
        assert_eq!(ServerConfig::get_redis_pool_max_size(), 500);
        assert_eq!(ServerConfig::get_redis_pool_timeout_ms(), 10000);
    }

    #[test]
    fn test_individual_getters_with_custom_values() {
        let _lock = match ENV_MUTEX.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        // Set custom values
        env::set_var("HOST", "192.168.1.1");
        env::set_var("APP_PORT", "9999");
        env::set_var("REDIS_URL", "redis://custom:6379");
        env::set_var("CONFIG_DIR", "/custom/config");
        env::set_var("CONFIG_FILE_NAME", "custom.json");
        env::set_var("API_KEY", "7EF1CB7C-5003-4696-B384-C72AF8C3E15D");
        env::set_var("RATE_LIMIT_REQUESTS_PER_SECOND", "500");
        env::set_var("RATE_LIMIT_BURST_SIZE", "1000");
        env::set_var("METRICS_PORT", "9999");
        env::set_var("ENABLE_SWAGGER", "true");
        env::set_var("REDIS_CONNECTION_TIMEOUT_MS", "5000");
        env::set_var("REDIS_KEY_PREFIX", "custom-prefix");
        env::set_var("RPC_TIMEOUT_MS", "15000");
        env::set_var("PROVIDER_MAX_RETRIES", "5");
        env::set_var("PROVIDER_RETRY_BASE_DELAY_MS", "200");
        env::set_var("PROVIDER_RETRY_MAX_DELAY_MS", "5000");
        env::set_var("PROVIDER_MAX_FAILOVERS", "10");
        env::set_var("REPOSITORY_STORAGE_TYPE", "redis");
        env::set_var("RESET_STORAGE_ON_START", "true");
        env::set_var("STORAGE_ENCRYPTION_KEY", "my-encryption-key");
        env::set_var("TRANSACTION_EXPIRATION_HOURS", "12");
        env::set_var("REDIS_POOL_MAX_SIZE", "200");
        env::set_var("REDIS_POOL_TIMEOUT_MS", "20000");

        // Test individual getters with custom values
        assert_eq!(ServerConfig::get_host(), "192.168.1.1");
        assert_eq!(ServerConfig::get_port(), 9999);
        assert_eq!(
            ServerConfig::get_redis_url_optional(),
            Some("redis://custom:6379".to_string())
        );
        assert_eq!(
            ServerConfig::get_config_file_path(),
            "/custom/config/custom.json"
        );
        assert!(ServerConfig::get_api_key_optional().is_some());
        assert_eq!(ServerConfig::get_rate_limit_requests_per_second(), 500);
        assert_eq!(ServerConfig::get_rate_limit_burst_size(), 1000);
        assert_eq!(ServerConfig::get_metrics_port(), 9999);
        assert!(ServerConfig::get_enable_swagger());
        assert_eq!(ServerConfig::get_redis_connection_timeout_ms(), 5000);
        assert_eq!(ServerConfig::get_redis_key_prefix(), "custom-prefix");
        assert_eq!(ServerConfig::get_rpc_timeout_ms(), 15000);
        assert_eq!(ServerConfig::get_provider_max_retries(), 5);
        assert_eq!(ServerConfig::get_provider_retry_base_delay_ms(), 200);
        assert_eq!(ServerConfig::get_provider_retry_max_delay_ms(), 5000);
        assert_eq!(ServerConfig::get_provider_max_failovers(), 10);
        assert_eq!(
            ServerConfig::get_repository_storage_type(),
            RepositoryStorageType::Redis
        );
        assert!(ServerConfig::get_reset_storage_on_start());
        assert!(ServerConfig::get_storage_encryption_key().is_some());
        assert_eq!(ServerConfig::get_transaction_expiration_hours(), 12.0);
        assert_eq!(ServerConfig::get_redis_pool_max_size(), 200);
        assert_eq!(ServerConfig::get_redis_pool_timeout_ms(), 20000);
    }

    #[test]
    fn test_get_redis_pool_max_size() {
        let _lock = match ENV_MUTEX.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        // Test default value when env var is not set
        env::remove_var("REDIS_POOL_MAX_SIZE");
        assert_eq!(ServerConfig::get_redis_pool_max_size(), 500);

        // Test custom value
        env::set_var("REDIS_POOL_MAX_SIZE", "100");
        assert_eq!(ServerConfig::get_redis_pool_max_size(), 100);

        // Test invalid value returns default
        env::set_var("REDIS_POOL_MAX_SIZE", "not_a_number");
        assert_eq!(ServerConfig::get_redis_pool_max_size(), 500);

        // Test zero value returns default (invalid)
        env::set_var("REDIS_POOL_MAX_SIZE", "0");
        assert_eq!(ServerConfig::get_redis_pool_max_size(), 500);

        // Test large value
        env::set_var("REDIS_POOL_MAX_SIZE", "10000");
        assert_eq!(ServerConfig::get_redis_pool_max_size(), 10000);

        // Cleanup
        env::remove_var("REDIS_POOL_MAX_SIZE");
    }

    #[test]
    fn test_get_redis_pool_timeout_ms() {
        let _lock = match ENV_MUTEX.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        // Test default value when env var is not set
        env::remove_var("REDIS_POOL_TIMEOUT_MS");
        assert_eq!(ServerConfig::get_redis_pool_timeout_ms(), 10000);

        // Test custom value
        env::set_var("REDIS_POOL_TIMEOUT_MS", "15000");
        assert_eq!(ServerConfig::get_redis_pool_timeout_ms(), 15000);

        // Test invalid value returns default
        env::set_var("REDIS_POOL_TIMEOUT_MS", "not_a_number");
        assert_eq!(ServerConfig::get_redis_pool_timeout_ms(), 10000);

        // Test zero value returns default (invalid)
        env::set_var("REDIS_POOL_TIMEOUT_MS", "0");
        assert_eq!(ServerConfig::get_redis_pool_timeout_ms(), 10000);

        // Test large value
        env::set_var("REDIS_POOL_TIMEOUT_MS", "60000");
        assert_eq!(ServerConfig::get_redis_pool_timeout_ms(), 60000);

        // Cleanup
        env::remove_var("REDIS_POOL_TIMEOUT_MS");
    }

    #[test]
    fn test_fractional_transaction_expiration_hours() {
        let _lock = match ENV_MUTEX.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        setup();

        // Test fractional hours (0.1 hours = 6 minutes)
        env::set_var("TRANSACTION_EXPIRATION_HOURS", "0.1");
        assert_eq!(ServerConfig::get_transaction_expiration_hours(), 0.1);

        // Test another fractional value
        env::set_var("TRANSACTION_EXPIRATION_HOURS", "0.5");
        assert_eq!(ServerConfig::get_transaction_expiration_hours(), 0.5);

        // Test integer value still works
        env::set_var("TRANSACTION_EXPIRATION_HOURS", "24");
        assert_eq!(ServerConfig::get_transaction_expiration_hours(), 24.0);

        // Cleanup
        env::remove_var("TRANSACTION_EXPIRATION_HOURS");
    }

    #[test]
    #[should_panic(expected = "REDIS_URL must be set")]
    fn test_get_redis_url_panics_when_not_set() {
        let _lock = match ENV_MUTEX.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        env::remove_var("REDIS_URL");
        let _ = ServerConfig::get_redis_url();
    }

    #[test]
    #[should_panic(expected = "API_KEY must be set")]
    fn test_get_api_key_panics_when_not_set() {
        let _lock = match ENV_MUTEX.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        env::remove_var("API_KEY");
        let _ = ServerConfig::get_api_key();
    }

    #[test]
    fn test_optional_getters_return_none_safely() {
        let _lock = match ENV_MUTEX.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        env::remove_var("REDIS_URL");
        env::remove_var("API_KEY");
        env::remove_var("STORAGE_ENCRYPTION_KEY");

        assert!(ServerConfig::get_redis_url_optional().is_none());
        assert!(ServerConfig::get_api_key_optional().is_none());
        assert!(ServerConfig::get_storage_encryption_key().is_none());
    }

    #[test]
    fn test_refactored_from_env_equivalence() {
        let _lock = match ENV_MUTEX.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        setup();

        // Set custom values to test both default and custom paths
        env::set_var("HOST", "custom-host");
        env::set_var("APP_PORT", "7777");
        env::set_var("RATE_LIMIT_REQUESTS_PER_SECOND", "250");
        env::set_var("METRICS_PORT", "7778");
        env::set_var("ENABLE_SWAGGER", "true");
        env::set_var("PROVIDER_MAX_RETRIES", "7");
        env::set_var("TRANSACTION_EXPIRATION_HOURS", "8");

        let config = ServerConfig::from_env();

        // Verify the refactored from_env() produces the same results as individual getters
        assert_eq!(config.host, ServerConfig::get_host());
        assert_eq!(config.port, ServerConfig::get_port());
        assert_eq!(config.redis_url, ServerConfig::get_redis_url());
        assert_eq!(
            config.config_file_path,
            ServerConfig::get_config_file_path()
        );
        assert_eq!(config.api_key, ServerConfig::get_api_key());
        assert_eq!(
            config.rate_limit_requests_per_second,
            ServerConfig::get_rate_limit_requests_per_second()
        );
        assert_eq!(
            config.rate_limit_burst_size,
            ServerConfig::get_rate_limit_burst_size()
        );
        assert_eq!(config.metrics_port, ServerConfig::get_metrics_port());
        assert_eq!(config.enable_swagger, ServerConfig::get_enable_swagger());
        assert_eq!(
            config.redis_connection_timeout_ms,
            ServerConfig::get_redis_connection_timeout_ms()
        );
        assert_eq!(
            config.redis_key_prefix,
            ServerConfig::get_redis_key_prefix()
        );
        assert_eq!(config.rpc_timeout_ms, ServerConfig::get_rpc_timeout_ms());
        assert_eq!(
            config.provider_max_retries,
            ServerConfig::get_provider_max_retries()
        );
        assert_eq!(
            config.provider_retry_base_delay_ms,
            ServerConfig::get_provider_retry_base_delay_ms()
        );
        assert_eq!(
            config.provider_retry_max_delay_ms,
            ServerConfig::get_provider_retry_max_delay_ms()
        );
        assert_eq!(
            config.provider_max_failovers,
            ServerConfig::get_provider_max_failovers()
        );
        assert_eq!(
            config.repository_storage_type,
            ServerConfig::get_repository_storage_type()
        );
        assert_eq!(
            config.reset_storage_on_start,
            ServerConfig::get_reset_storage_on_start()
        );
        assert_eq!(
            config.storage_encryption_key,
            ServerConfig::get_storage_encryption_key()
        );
        assert_eq!(
            config.transaction_expiration_hours,
            ServerConfig::get_transaction_expiration_hours()
        );
    }

    mod get_worker_concurrency_tests {
        use super::*;
        use serial_test::serial;

        #[test]
        #[serial]
        fn test_returns_default_when_env_not_set() {
            let worker_name = "test_worker";
            let env_var = format!(
                "BACKGROUND_WORKER_{}_CONCURRENCY",
                worker_name.to_uppercase()
            );

            // Ensure env var is not set
            env::remove_var(&env_var);

            let default_value = 42;
            let result = ServerConfig::get_worker_concurrency(worker_name, default_value);

            assert_eq!(
                result, default_value,
                "Should return default value when env var is not set"
            );
        }

        #[test]
        #[serial]
        fn test_returns_env_value_when_set() {
            let worker_name = "status_checker";
            let env_var = format!(
                "BACKGROUND_WORKER_{}_CONCURRENCY",
                worker_name.to_uppercase()
            );

            // Set env var to a specific value
            env::set_var(&env_var, "100");

            let default_value = 10;
            let result = ServerConfig::get_worker_concurrency(worker_name, default_value);

            assert_eq!(result, 100, "Should return env var value when set");

            // Cleanup
            env::remove_var(&env_var);
        }

        #[test]
        #[serial]
        fn test_returns_default_when_env_invalid() {
            let worker_name = "invalid_worker";
            let env_var = format!(
                "BACKGROUND_WORKER_{}_CONCURRENCY",
                worker_name.to_uppercase()
            );

            // Set env var to invalid value
            env::set_var(&env_var, "not_a_number");

            let default_value = 25;
            let result = ServerConfig::get_worker_concurrency(worker_name, default_value);

            assert_eq!(
                result, default_value,
                "Should return default value when env var is invalid"
            );

            // Cleanup
            env::remove_var(&env_var);
        }

        #[test]
        #[serial]
        fn test_returns_default_when_env_empty() {
            let worker_name = "empty_worker";
            let env_var = format!(
                "BACKGROUND_WORKER_{}_CONCURRENCY",
                worker_name.to_uppercase()
            );

            // Set env var to empty string
            env::set_var(&env_var, "");

            let default_value = 15;
            let result = ServerConfig::get_worker_concurrency(worker_name, default_value);

            assert_eq!(
                result, default_value,
                "Should return default value when env var is empty"
            );

            // Cleanup
            env::remove_var(&env_var);
        }

        #[test]
        #[serial]
        fn test_returns_default_when_env_negative() {
            let worker_name = "negative_worker";
            let env_var = format!(
                "BACKGROUND_WORKER_{}_CONCURRENCY",
                worker_name.to_uppercase()
            );

            // Set env var to negative value
            env::set_var(&env_var, "-5");

            let default_value = 20;
            let result = ServerConfig::get_worker_concurrency(worker_name, default_value);

            assert_eq!(
                result, default_value,
                "Should return default value when env var is negative"
            );

            // Cleanup
            env::remove_var(&env_var);
        }

        #[test]
        #[serial]
        fn test_env_var_name_formatting() {
            // Test that worker names are properly uppercased
            let worker_names = vec![
                (
                    "transaction_sender",
                    "BACKGROUND_WORKER_TRANSACTION_SENDER_CONCURRENCY",
                ),
                (
                    "status_checker_evm",
                    "BACKGROUND_WORKER_STATUS_CHECKER_EVM_CONCURRENCY",
                ),
                (
                    "notification_sender",
                    "BACKGROUND_WORKER_NOTIFICATION_SENDER_CONCURRENCY",
                ),
            ];

            for (worker_name, expected_env_var) in worker_names {
                let actual_env_var = format!(
                    "BACKGROUND_WORKER_{}_CONCURRENCY",
                    worker_name.to_uppercase()
                );
                assert_eq!(
                    actual_env_var, expected_env_var,
                    "Env var name should be correctly formatted for worker: {worker_name}"
                );
            }
        }

        #[test]
        #[serial]
        fn test_zero_value() {
            let worker_name = "zero_worker";
            let env_var = format!(
                "BACKGROUND_WORKER_{}_CONCURRENCY",
                worker_name.to_uppercase()
            );

            // Set env var to zero
            env::set_var(&env_var, "0");

            let default_value = 30;
            let result = ServerConfig::get_worker_concurrency(worker_name, default_value);

            assert_eq!(result, 0, "Should accept zero as a valid value");

            // Cleanup
            env::remove_var(&env_var);
        }

        #[test]
        #[serial]
        fn test_large_value() {
            let worker_name = "large_worker";
            let env_var = format!(
                "BACKGROUND_WORKER_{}_CONCURRENCY",
                worker_name.to_uppercase()
            );

            // Set env var to a large value
            env::set_var(&env_var, "10000");

            let default_value = 50;
            let result = ServerConfig::get_worker_concurrency(worker_name, default_value);

            assert_eq!(result, 10000, "Should accept large values");

            // Cleanup
            env::remove_var(&env_var);
        }

        #[test]
        #[serial]
        fn test_whitespace_in_value() {
            let worker_name = "whitespace_worker";
            let env_var = format!(
                "BACKGROUND_WORKER_{}_CONCURRENCY",
                worker_name.to_uppercase()
            );

            // Set env var with leading/trailing whitespace
            env::set_var(&env_var, "  75  ");

            let default_value = 35;
            let result = ServerConfig::get_worker_concurrency(worker_name, default_value);

            // Note: String::parse::<usize>() does NOT trim whitespace, so this will fail to parse
            // and return the default value
            assert_eq!(
                result, default_value,
                "Should return default value when value has whitespace"
            );

            // Cleanup
            env::remove_var(&env_var);
        }

        #[test]
        #[serial]
        fn test_float_value_returns_default() {
            let worker_name = "float_worker";
            let env_var = format!(
                "BACKGROUND_WORKER_{}_CONCURRENCY",
                worker_name.to_uppercase()
            );

            // Set env var to float value
            env::set_var(&env_var, "12.5");

            let default_value = 40;
            let result = ServerConfig::get_worker_concurrency(worker_name, default_value);

            assert_eq!(
                result, default_value,
                "Should return default value for float input"
            );

            // Cleanup
            env::remove_var(&env_var);
        }
    }

    mod get_relayer_concurrency_limit_tests {
        use super::*;
        use serial_test::serial;

        #[test]
        #[serial]
        fn test_returns_default_when_env_not_set() {
            env::remove_var("RELAYER_CONCURRENCY_LIMIT");
            let result = ServerConfig::get_relayer_concurrency_limit();
            assert_eq!(result, 100, "Should return default value of 100");
        }

        #[test]
        #[serial]
        fn test_returns_env_value_when_set() {
            env::set_var("RELAYER_CONCURRENCY_LIMIT", "250");
            let result = ServerConfig::get_relayer_concurrency_limit();
            assert_eq!(result, 250, "Should return env var value");
            env::remove_var("RELAYER_CONCURRENCY_LIMIT");
        }

        #[test]
        #[serial]
        fn test_returns_default_when_env_invalid() {
            env::set_var("RELAYER_CONCURRENCY_LIMIT", "not_a_number");
            let result = ServerConfig::get_relayer_concurrency_limit();
            assert_eq!(result, 100, "Should return default value when invalid");
            env::remove_var("RELAYER_CONCURRENCY_LIMIT");
        }

        #[test]
        #[serial]
        fn test_returns_default_when_env_empty() {
            env::set_var("RELAYER_CONCURRENCY_LIMIT", "");
            let result = ServerConfig::get_relayer_concurrency_limit();
            assert_eq!(result, 100, "Should return default value when empty");
            env::remove_var("RELAYER_CONCURRENCY_LIMIT");
        }

        #[test]
        #[serial]
        fn test_zero_value() {
            env::set_var("RELAYER_CONCURRENCY_LIMIT", "0");
            let result = ServerConfig::get_relayer_concurrency_limit();
            assert_eq!(result, 0, "Should accept zero as valid value");
            env::remove_var("RELAYER_CONCURRENCY_LIMIT");
        }

        #[test]
        #[serial]
        fn test_large_value() {
            env::set_var("RELAYER_CONCURRENCY_LIMIT", "5000");
            let result = ServerConfig::get_relayer_concurrency_limit();
            assert_eq!(result, 5000, "Should accept large values");
            env::remove_var("RELAYER_CONCURRENCY_LIMIT");
        }

        #[test]
        #[serial]
        fn test_negative_value_returns_default() {
            env::set_var("RELAYER_CONCURRENCY_LIMIT", "-10");
            let result = ServerConfig::get_relayer_concurrency_limit();
            assert_eq!(result, 100, "Should return default for negative value");
            env::remove_var("RELAYER_CONCURRENCY_LIMIT");
        }

        #[test]
        #[serial]
        fn test_float_value_returns_default() {
            env::set_var("RELAYER_CONCURRENCY_LIMIT", "100.5");
            let result = ServerConfig::get_relayer_concurrency_limit();
            assert_eq!(result, 100, "Should return default for float value");
            env::remove_var("RELAYER_CONCURRENCY_LIMIT");
        }

        #[test]
        #[serial]
        fn test_whitespace_value_returns_default() {
            env::set_var("RELAYER_CONCURRENCY_LIMIT", "  150  ");
            let result = ServerConfig::get_relayer_concurrency_limit();
            assert_eq!(
                result, 100,
                "Should return default when value has whitespace"
            );
            env::remove_var("RELAYER_CONCURRENCY_LIMIT");
        }
    }

    mod get_max_connections_tests {
        use super::*;
        use serial_test::serial;

        #[test]
        #[serial]
        fn test_returns_default_when_env_not_set() {
            env::remove_var("MAX_CONNECTIONS");
            let result = ServerConfig::get_max_connections();
            assert_eq!(result, 256, "Should return default value of 256");
        }

        #[test]
        #[serial]
        fn test_returns_env_value_when_set() {
            env::set_var("MAX_CONNECTIONS", "512");
            let result = ServerConfig::get_max_connections();
            assert_eq!(result, 512, "Should return env var value");
            env::remove_var("MAX_CONNECTIONS");
        }

        #[test]
        #[serial]
        fn test_returns_default_when_env_invalid() {
            env::set_var("MAX_CONNECTIONS", "invalid");
            let result = ServerConfig::get_max_connections();
            assert_eq!(result, 256, "Should return default value when invalid");
            env::remove_var("MAX_CONNECTIONS");
        }

        #[test]
        #[serial]
        fn test_returns_default_when_env_empty() {
            env::set_var("MAX_CONNECTIONS", "");
            let result = ServerConfig::get_max_connections();
            assert_eq!(result, 256, "Should return default value when empty");
            env::remove_var("MAX_CONNECTIONS");
        }

        #[test]
        #[serial]
        fn test_zero_value() {
            env::set_var("MAX_CONNECTIONS", "0");
            let result = ServerConfig::get_max_connections();
            assert_eq!(result, 0, "Should accept zero as valid value");
            env::remove_var("MAX_CONNECTIONS");
        }

        #[test]
        #[serial]
        fn test_large_value() {
            env::set_var("MAX_CONNECTIONS", "10000");
            let result = ServerConfig::get_max_connections();
            assert_eq!(result, 10000, "Should accept large values");
            env::remove_var("MAX_CONNECTIONS");
        }

        #[test]
        #[serial]
        fn test_negative_value_returns_default() {
            env::set_var("MAX_CONNECTIONS", "-100");
            let result = ServerConfig::get_max_connections();
            assert_eq!(result, 256, "Should return default for negative value");
            env::remove_var("MAX_CONNECTIONS");
        }

        #[test]
        #[serial]
        fn test_float_value_returns_default() {
            env::set_var("MAX_CONNECTIONS", "256.5");
            let result = ServerConfig::get_max_connections();
            assert_eq!(result, 256, "Should return default for float value");
            env::remove_var("MAX_CONNECTIONS");
        }
    }

    mod get_connection_backlog_tests {
        use super::*;
        use serial_test::serial;

        #[test]
        #[serial]
        fn test_returns_default_when_env_not_set() {
            env::remove_var("CONNECTION_BACKLOG");
            let result = ServerConfig::get_connection_backlog();
            assert_eq!(result, 511, "Should return default value of 511");
        }

        #[test]
        #[serial]
        fn test_returns_env_value_when_set() {
            env::set_var("CONNECTION_BACKLOG", "1024");
            let result = ServerConfig::get_connection_backlog();
            assert_eq!(result, 1024, "Should return env var value");
            env::remove_var("CONNECTION_BACKLOG");
        }

        #[test]
        #[serial]
        fn test_returns_default_when_env_invalid() {
            env::set_var("CONNECTION_BACKLOG", "not_a_number");
            let result = ServerConfig::get_connection_backlog();
            assert_eq!(result, 511, "Should return default value when invalid");
            env::remove_var("CONNECTION_BACKLOG");
        }

        #[test]
        #[serial]
        fn test_returns_default_when_env_empty() {
            env::set_var("CONNECTION_BACKLOG", "");
            let result = ServerConfig::get_connection_backlog();
            assert_eq!(result, 511, "Should return default value when empty");
            env::remove_var("CONNECTION_BACKLOG");
        }

        #[test]
        #[serial]
        fn test_zero_value() {
            env::set_var("CONNECTION_BACKLOG", "0");
            let result = ServerConfig::get_connection_backlog();
            assert_eq!(result, 0, "Should accept zero as valid value");
            env::remove_var("CONNECTION_BACKLOG");
        }

        #[test]
        #[serial]
        fn test_large_value() {
            env::set_var("CONNECTION_BACKLOG", "65535");
            let result = ServerConfig::get_connection_backlog();
            assert_eq!(result, 65535, "Should accept large values");
            env::remove_var("CONNECTION_BACKLOG");
        }

        #[test]
        #[serial]
        fn test_negative_value_returns_default() {
            env::set_var("CONNECTION_BACKLOG", "-50");
            let result = ServerConfig::get_connection_backlog();
            assert_eq!(result, 511, "Should return default for negative value");
            env::remove_var("CONNECTION_BACKLOG");
        }

        #[test]
        #[serial]
        fn test_float_value_returns_default() {
            env::set_var("CONNECTION_BACKLOG", "511.5");
            let result = ServerConfig::get_connection_backlog();
            assert_eq!(result, 511, "Should return default for float value");
            env::remove_var("CONNECTION_BACKLOG");
        }

        #[test]
        #[serial]
        fn test_common_production_values() {
            // Test common production values
            let test_cases = vec![
                (128, "Small server"),
                (511, "Default"),
                (1024, "Medium server"),
                (2048, "Large server"),
            ];

            for (value, description) in test_cases {
                env::set_var("CONNECTION_BACKLOG", value.to_string());
                let result = ServerConfig::get_connection_backlog();
                assert_eq!(result, value, "Should accept {description}: {value}");
            }

            env::remove_var("CONNECTION_BACKLOG");
        }
    }

    mod get_request_timeout_seconds_tests {
        use super::*;
        use serial_test::serial;

        #[test]
        #[serial]
        fn test_returns_default_when_env_not_set() {
            env::remove_var("REQUEST_TIMEOUT_SECONDS");
            let result = ServerConfig::get_request_timeout_seconds();
            assert_eq!(result, 30, "Should return default value of 30");
        }

        #[test]
        #[serial]
        fn test_returns_env_value_when_set() {
            env::set_var("REQUEST_TIMEOUT_SECONDS", "60");
            let result = ServerConfig::get_request_timeout_seconds();
            assert_eq!(result, 60, "Should return env var value");
            env::remove_var("REQUEST_TIMEOUT_SECONDS");
        }

        #[test]
        #[serial]
        fn test_returns_default_when_env_invalid() {
            env::set_var("REQUEST_TIMEOUT_SECONDS", "invalid");
            let result = ServerConfig::get_request_timeout_seconds();
            assert_eq!(result, 30, "Should return default value when invalid");
            env::remove_var("REQUEST_TIMEOUT_SECONDS");
        }

        #[test]
        #[serial]
        fn test_returns_default_when_env_empty() {
            env::set_var("REQUEST_TIMEOUT_SECONDS", "");
            let result = ServerConfig::get_request_timeout_seconds();
            assert_eq!(result, 30, "Should return default value when empty");
            env::remove_var("REQUEST_TIMEOUT_SECONDS");
        }

        #[test]
        #[serial]
        fn test_zero_value() {
            env::set_var("REQUEST_TIMEOUT_SECONDS", "0");
            let result = ServerConfig::get_request_timeout_seconds();
            assert_eq!(result, 0, "Should accept zero as valid value");
            env::remove_var("REQUEST_TIMEOUT_SECONDS");
        }

        #[test]
        #[serial]
        fn test_large_value() {
            env::set_var("REQUEST_TIMEOUT_SECONDS", "300");
            let result = ServerConfig::get_request_timeout_seconds();
            assert_eq!(result, 300, "Should accept large values");
            env::remove_var("REQUEST_TIMEOUT_SECONDS");
        }

        #[test]
        #[serial]
        fn test_negative_value_returns_default() {
            env::set_var("REQUEST_TIMEOUT_SECONDS", "-10");
            let result = ServerConfig::get_request_timeout_seconds();
            assert_eq!(result, 30, "Should return default for negative value");
            env::remove_var("REQUEST_TIMEOUT_SECONDS");
        }

        #[test]
        #[serial]
        fn test_float_value_returns_default() {
            env::set_var("REQUEST_TIMEOUT_SECONDS", "30.5");
            let result = ServerConfig::get_request_timeout_seconds();
            assert_eq!(result, 30, "Should return default for float value");
            env::remove_var("REQUEST_TIMEOUT_SECONDS");
        }

        #[test]
        #[serial]
        fn test_common_timeout_values() {
            // Test common timeout values
            let test_cases = vec![
                (10, "Short timeout"),
                (30, "Default timeout"),
                (60, "Moderate timeout"),
                (120, "Long timeout"),
            ];

            for (value, description) in test_cases {
                env::set_var("REQUEST_TIMEOUT_SECONDS", value.to_string());
                let result = ServerConfig::get_request_timeout_seconds();
                assert_eq!(result, value, "Should accept {description}: {value}");
            }

            env::remove_var("REQUEST_TIMEOUT_SECONDS");
        }
    }

    mod get_redis_reader_url_tests {
        use super::*;
        use serial_test::serial;

        #[test]
        #[serial]
        fn test_returns_none_when_env_not_set() {
            env::remove_var("REDIS_READER_URL");
            let result = ServerConfig::get_redis_reader_url_optional();
            assert!(
                result.is_none(),
                "Should return None when env var is not set"
            );
        }

        #[test]
        #[serial]
        fn test_returns_value_when_set() {
            env::set_var("REDIS_READER_URL", "redis://reader:6379");
            let result = ServerConfig::get_redis_reader_url_optional();
            assert_eq!(
                result,
                Some("redis://reader:6379".to_string()),
                "Should return the env var value"
            );
            env::remove_var("REDIS_READER_URL");
        }

        #[test]
        #[serial]
        fn test_returns_empty_string_when_set_to_empty() {
            env::set_var("REDIS_READER_URL", "");
            let result = ServerConfig::get_redis_reader_url_optional();
            assert_eq!(
                result,
                Some("".to_string()),
                "Should return empty string when set to empty"
            );
            env::remove_var("REDIS_READER_URL");
        }

        #[test]
        #[serial]
        fn test_aws_elasticache_reader_url() {
            // Test with typical AWS ElastiCache reader endpoint format
            let reader_url = "redis://my-cluster-ro.xxx.cache.amazonaws.com:6379";
            env::set_var("REDIS_READER_URL", reader_url);
            let result = ServerConfig::get_redis_reader_url_optional();
            assert_eq!(
                result,
                Some(reader_url.to_string()),
                "Should accept AWS ElastiCache reader endpoint"
            );
            env::remove_var("REDIS_READER_URL");
        }

        #[test]
        #[serial]
        fn test_config_includes_redis_reader_url() {
            env::set_var("REDIS_URL", "redis://primary:6379");
            env::set_var("REDIS_READER_URL", "redis://reader:6379");
            env::set_var("API_KEY", "7EF1CB7C-5003-4696-B384-C72AF8C3E15D");

            let config = ServerConfig::from_env();

            assert_eq!(config.redis_url, "redis://primary:6379");
            assert_eq!(
                config.redis_reader_url,
                Some("redis://reader:6379".to_string())
            );

            env::remove_var("REDIS_URL");
            env::remove_var("REDIS_READER_URL");
            env::remove_var("API_KEY");
        }

        #[test]
        #[serial]
        fn test_config_without_redis_reader_url() {
            env::set_var("REDIS_URL", "redis://primary:6379");
            env::remove_var("REDIS_READER_URL");
            env::set_var("API_KEY", "7EF1CB7C-5003-4696-B384-C72AF8C3E15D");

            let config = ServerConfig::from_env();

            assert_eq!(config.redis_url, "redis://primary:6379");
            assert!(
                config.redis_reader_url.is_none(),
                "redis_reader_url should be None when not set"
            );

            env::remove_var("REDIS_URL");
            env::remove_var("API_KEY");
        }
    }

    mod get_redis_reader_pool_max_size_tests {
        use super::*;
        use serial_test::serial;

        #[test]
        #[serial]
        fn test_returns_default_when_env_not_set() {
            env::remove_var("REDIS_READER_POOL_MAX_SIZE");
            let result = ServerConfig::get_redis_reader_pool_max_size();
            assert_eq!(
                result, 1000,
                "Should return default 1000 when env var is not set"
            );
        }

        #[test]
        #[serial]
        fn test_returns_value_when_set() {
            env::set_var("REDIS_READER_POOL_MAX_SIZE", "2000");
            let result = ServerConfig::get_redis_reader_pool_max_size();
            assert_eq!(result, 2000, "Should return the parsed value");
            env::remove_var("REDIS_READER_POOL_MAX_SIZE");
        }

        #[test]
        #[serial]
        fn test_returns_default_when_invalid() {
            env::set_var("REDIS_READER_POOL_MAX_SIZE", "not_a_number");
            let result = ServerConfig::get_redis_reader_pool_max_size();
            assert_eq!(
                result, 1000,
                "Should return default 1000 for invalid values"
            );
            env::remove_var("REDIS_READER_POOL_MAX_SIZE");
        }

        #[test]
        #[serial]
        fn test_returns_default_when_zero() {
            env::set_var("REDIS_READER_POOL_MAX_SIZE", "0");
            let result = ServerConfig::get_redis_reader_pool_max_size();
            assert_eq!(result, 1000, "Should return default 1000 when value is 0");
            env::remove_var("REDIS_READER_POOL_MAX_SIZE");
        }

        #[test]
        #[serial]
        fn test_returns_default_when_negative() {
            env::set_var("REDIS_READER_POOL_MAX_SIZE", "-100");
            let result = ServerConfig::get_redis_reader_pool_max_size();
            assert_eq!(
                result, 1000,
                "Should return default 1000 for negative values"
            );
            env::remove_var("REDIS_READER_POOL_MAX_SIZE");
        }

        #[test]
        #[serial]
        fn test_config_includes_reader_pool_max_size() {
            env::set_var("REDIS_URL", "redis://primary:6379");
            env::set_var("API_KEY", "7EF1CB7C-5003-4696-B384-C72AF8C3E15D");
            env::set_var("REDIS_READER_POOL_MAX_SIZE", "750");

            let config = ServerConfig::from_env();

            assert_eq!(
                config.redis_reader_pool_max_size, 750,
                "Should include reader pool max size in config"
            );

            env::remove_var("REDIS_URL");
            env::remove_var("API_KEY");
            env::remove_var("REDIS_READER_POOL_MAX_SIZE");
        }

        #[test]
        #[serial]
        fn test_config_uses_default_when_not_set() {
            env::set_var("REDIS_URL", "redis://primary:6379");
            env::set_var("API_KEY", "7EF1CB7C-5003-4696-B384-C72AF8C3E15D");
            env::remove_var("REDIS_READER_POOL_MAX_SIZE");

            let config = ServerConfig::from_env();

            assert_eq!(
                config.redis_reader_pool_max_size, 1000,
                "Should use default 1000 when not set"
            );

            env::remove_var("REDIS_URL");
            env::remove_var("API_KEY");
        }
    }
}
