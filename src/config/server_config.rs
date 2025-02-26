/// Configuration for the server, including network and rate limiting settings.
use std::env;

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
    pub api_key: String,
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
    /// - `CONFIG_FILE_PATH` defaults to `"config/config.json"`.
    /// - `RATE_LIMIT_REQUESTS_PER_SECOND` defaults to `100`.
    /// - `RATE_LIMIT_BURST_SIZE` defaults to `300`.
    /// - `METRICS_PORT` defaults to `8081`.
    pub fn from_env() -> Self {
        Self {
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            port: env::var("APP_PORT")
                .unwrap_or_else(|_| "8080".to_string())
                .parse()
                .unwrap_or(8080),
            redis_url: env::var("REDIS_URL").expect("REDIS_URL must be set"),
            config_file_path: env::var("CONFIG_FILE_PATH")
                .unwrap_or_else(|_| "config/config.json".to_string()),
            api_key: env::var("API_KEY").expect("API_KEY must be set"),
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
