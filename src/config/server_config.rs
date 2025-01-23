use std::env;

#[derive(Debug)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub redis_url: String,
    pub config_file_path: String,
}

impl ServerConfig {
    pub fn from_env() -> Self {
        Self {
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            port: env::var("PORT")
                .unwrap_or_else(|_| "8080".to_string())
                .parse()
                .unwrap_or(8080),
            redis_url: env::var("REDIS_URL").expect("REDIS_URL must be set"),
            config_file_path: env::var("CONFIG_FILE_PATH")
                .unwrap_or_else(|_| "config/config.json".to_string()),
        }
    }
}
