use std::env;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub redis_url: String,
    pub config_file_path: String,
    pub api_key: String,
    pub rate_limit_requests_per_second: u64,
    pub rate_limit_burst_size: u32,
    pub metrics_port: u16,
}

impl ServerConfig {
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
                .unwrap_or_else(|_| "2".to_string())
                .parse()
                .unwrap_or(100),
            rate_limit_burst_size: env::var("RATE_LIMIT_BURST_SIZE")
                .unwrap_or_else(|_| "3".to_string())
                .parse()
                .unwrap_or(300),
            metrics_port: env::var("METRICS_PORT")
                .unwrap_or_else(|_| "8081".to_string())
                .parse()
                .unwrap_or(8081),
        }
    }
}
