/// Configuration for the server, including network and rate limiting settings.
use std::env;

use crate::models::SecretString;

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
    pub fn from_env() -> Self {
        let conf_dir = env::var("IN_DOCKER")
            .map(|val| val == "true")
            .unwrap_or(false)
            .then(|| "config/".to_string())
            .unwrap_or_else(|| env::var("CONFIG_DIR").unwrap_or_else(|_| "./config".to_string()));

        let conf_dir = format!("{}/", conf_dir.trim_end_matches('/'));

        // Get config filename (default: config.json), applies to both local and Docker
        let config_file_name =
            env::var("CONFIG_FILE_NAME").unwrap_or_else(|_| "config.json".to_string());

        // Construct full path
        let config_file_path = format!("{}{}", conf_dir, config_file_name);

        Self {
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            port: env::var("APP_PORT")
                .unwrap_or_else(|_| "8080".to_string())
                .parse()
                .unwrap_or(8080),
            redis_url: env::var("REDIS_URL").expect("REDIS_URL must be set"),
            config_file_path,
            api_key: SecretString::new(&env::var("API_KEY").expect("API_KEY must be set")),
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
        }
    }
}
