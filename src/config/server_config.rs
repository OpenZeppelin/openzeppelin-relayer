/// Configuration for the server, including network and rate limiting settings.
use std::{env, str::FromStr};
use strum::Display;

use crate::{constants::MINIMUM_SECRET_VALUE_LENGTH, models::SecretString};

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
            _ => Err(format!("Invalid repository storage type: {}", s)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// The host address the server will bind to.
    pub host: String,
    /// The port number the server will listen on.
    pub port: u16,
    /// The URL for the Redis instance.
    pub redis_url: String,
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
    /// The type of repository storage to use.
    pub repository_storage_type: RepositoryStorageType,
    /// Flag to force config file processing.
    pub reset_storage_on_start: bool,
    /// The encryption key for the storage.
    pub storage_encryption_key: Option<SecretString>,
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
    /// - `REPOSITORY_STORAGE_TYPE` defaults to `"in_memory"`.
    pub fn from_env() -> Self {
        let conf_dir = if env::var("IN_DOCKER")
            .map(|val| val == "true")
            .unwrap_or(false)
        {
            "config/".to_string()
        } else {
            env::var("CONFIG_DIR").unwrap_or_else(|_| "./config".to_string())
        };

        let conf_dir = format!("{}/", conf_dir.trim_end_matches('/'));

        // Get config filename (default: config.json), applies to both local and Docker
        let config_file_name =
            env::var("CONFIG_FILE_NAME").unwrap_or_else(|_| "config.json".to_string());

        // Construct full path
        let config_file_path = format!("{}{}", conf_dir, config_file_name);

        let api_key = SecretString::new(&env::var("API_KEY").expect("API_KEY must be set"));

        if !api_key.has_minimum_length(MINIMUM_SECRET_VALUE_LENGTH) {
            panic!(
                "Security error: API_KEY must be at least {} characters long",
                MINIMUM_SECRET_VALUE_LENGTH
            );
        }

        Self {
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            port: env::var("APP_PORT")
                .unwrap_or_else(|_| "8080".to_string())
                .parse()
                .unwrap_or(8080),
            redis_url: env::var("REDIS_URL").expect("REDIS_URL must be set"),
            config_file_path,
            api_key,
            rate_limit_requests_per_second: env::var("RATE_LIMIT_REQUESTS_PER_SECOND")
                .unwrap_or_else(|_| "100".to_string())
                .parse()
                .unwrap_or(100),
            rate_limit_burst_size: env::var("RATE_LIMIT_BURST_SIZE")
                .unwrap_or_else(|_| "300".to_string())
                .parse()
                .unwrap_or(300),
            metrics_port: env::var("METRICS_PORT")
                .unwrap_or_else(|_| "8081".to_string())
                .parse()
                .unwrap_or(8081),
            enable_swagger: env::var("ENABLE_SWAGGER")
                .map(|v| v.to_lowercase() == "true")
                .unwrap_or(false),
            redis_connection_timeout_ms: env::var("REDIS_CONNECTION_TIMEOUT_MS")
                .unwrap_or_else(|_| "10000".to_string())
                .parse()
                .unwrap_or(10000),
            redis_key_prefix: env::var("REDIS_KEY_PREFIX")
                .unwrap_or_else(|_| "oz-relayer".to_string()),
            rpc_timeout_ms: env::var("RPC_TIMEOUT_MS")
                .unwrap_or_else(|_| "10000".to_string())
                .parse()
                .unwrap_or(10000),
            provider_max_retries: env::var("PROVIDER_MAX_RETRIES")
                .unwrap_or_else(|_| "3".to_string())
                .parse()
                .unwrap_or(3),
            provider_retry_base_delay_ms: env::var("PROVIDER_RETRY_BASE_DELAY_MS")
                .unwrap_or_else(|_| "100".to_string())
                .parse()
                .unwrap_or(100),
            provider_retry_max_delay_ms: env::var("PROVIDER_RETRY_MAX_DELAY_MS")
                .unwrap_or_else(|_| "2000".to_string())
                .parse()
                .unwrap_or(2000),
            provider_max_failovers: env::var("PROVIDER_MAX_FAILOVERS")
                .unwrap_or_else(|_| "3".to_string())
                .parse()
                .unwrap_or(3),
            repository_storage_type: env::var("REPOSITORY_STORAGE_TYPE")
                .unwrap_or_else(|_| "in_memory".to_string())
                .parse()
                .unwrap_or(RepositoryStorageType::InMemory),
            reset_storage_on_start: env::var("RESET_STORAGE_ON_START")
                .map(|v| v.to_lowercase() == "true")
                .unwrap_or(false),
            storage_encryption_key: env::var("STORAGE_ENCRYPTION_KEY")
                .map(|v| SecretString::new(&v))
                .ok(),
        }
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
        assert_eq!(
            config.repository_storage_type,
            RepositoryStorageType::InMemory
        );
        assert!(!config.reset_storage_on_start);
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

        let _ = ServerConfig::from_env();

        panic!("Test should have panicked before reaching here");
    }
}
