use std::sync::Arc;
use std::time::Duration;

use color_eyre::Result;
use redis::aio::ConnectionManager;
use tokio::time::timeout;

use crate::config::ServerConfig;

/// Initializes a Redis connection manager.
///
/// # Arguments
///
/// * `config` - The server configuration.
///
/// # Returns
///
/// A connection manager for the Redis connection.
pub async fn initialize_redis_connection(config: &ServerConfig) -> Result<Arc<ConnectionManager>> {
    let redis_client = redis::Client::open(config.redis_url.as_str())?;
    let connection_manager = timeout(
        Duration::from_millis(config.redis_connection_timeout_ms),
        redis::aio::ConnectionManager::new(redis_client),
    )
    .await
    .map_err(|_| {
        eyre::eyre!(
            "Redis connection timeout after {}ms",
            config.redis_connection_timeout_ms
        )
    })??;
    let connection_manager = Arc::new(connection_manager);

    Ok(connection_manager)
}
