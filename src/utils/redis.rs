use std::sync::Arc;
use std::time::Duration;

use color_eyre::Result;
use deadpool_redis::{Config, Pool, Runtime};

use crate::config::ServerConfig;

/// Initializes a Redis connection pool using deadpool-redis.
///
/// # Arguments
///
/// * `config` - The server configuration.
///
/// # Returns
///
/// A connection pool for Redis connections with health checks and timeouts.
///
/// # Features
///
/// - Connection pooling with configurable max size
/// - Automatic health checks and connection recycling
/// - Timeout for acquiring connections from the pool
pub async fn initialize_redis_connection(config: &ServerConfig) -> Result<Arc<Pool>> {
    let cfg = Config::from_url(&config.redis_url);

    let pool = cfg
        .builder()
        .map_err(|e| eyre::eyre!("Failed to create Redis pool builder: {}", e))?
        .max_size(config.redis_pool_max_size)
        .wait_timeout(Some(Duration::from_millis(config.redis_pool_timeout_ms)))
        .create_timeout(Some(Duration::from_millis(
            config.redis_connection_timeout_ms,
        )))
        .recycle_timeout(Some(Duration::from_millis(
            config.redis_connection_timeout_ms,
        )))
        .runtime(Runtime::Tokio1)
        .build()
        .map_err(|e| eyre::eyre!("Failed to build Redis pool: {}", e))?;

    // Verify the pool is working by getting a connection
    let conn = pool
        .get()
        .await
        .map_err(|e| eyre::eyre!("Failed to get initial Redis connection: {}", e))?;
    drop(conn);

    Ok(Arc::new(pool))
}
