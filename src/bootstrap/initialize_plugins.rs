//! Plugin Pool Initialization
//!
//! This module handles conditional initialization of the plugin worker pool.
//! The pool is only started if plugins are configured, avoiding overhead
//! when the plugin system is not in use.

use std::sync::Arc;
use tracing::{info, warn};

use crate::repositories::PluginRepositoryTrait;
use crate::services::plugins::{get_pool_manager, PoolManager};

/// Initialize the plugin worker pool if plugins are configured.
///
/// This function checks if any plugins are registered in the repository.
/// If plugins exist, it starts the Piscina worker pool for efficient
/// plugin execution. If no plugins are configured, it skips initialization.
///
/// # Arguments
///
/// * `plugin_repository` - Reference to the plugin repository
///
/// # Returns
///
/// * `Ok(Some(Arc<PoolManager>))` - Pool manager if plugins are configured
/// * `Ok(None)` - If no plugins are configured
/// * `Err` - If pool initialization fails
pub async fn initialize_plugin_pool<PR: PluginRepositoryTrait>(
    plugin_repository: &PR,
) -> eyre::Result<Option<Arc<PoolManager>>> {
    let has_plugins = plugin_repository
        .has_entries()
        .await
        .map_err(|e| eyre::eyre!("Failed to check plugin repository: {}", e))?;

    if !has_plugins {
        info!("No plugins configured, skipping plugin pool initialization");
        return Ok(None);
    }

    let plugin_count = plugin_repository.count().await.unwrap_or(0);
    info!(
        plugin_count = plugin_count,
        "Plugins detected, initializing worker pool"
    );

    let pool_manager = get_pool_manager();

    match pool_manager.ensure_started().await {
        Ok(()) => {
            info!("Plugin worker pool initialized successfully");
            Ok(Some(pool_manager))
        }
        Err(e) => {
            warn!(error = %e, "Failed to start plugin worker pool, falling back to ts-node execution");
            Ok(None)
        }
    }
}

/// Precompile all configured plugins.
///
/// This function loads all plugins from the repository and triggers
/// precompilation via the worker pool. Compiled code is cached in
/// the pool for fast execution.
///
/// # Arguments
///
/// * `plugin_repository` - Reference to the plugin repository
/// * `pool_manager` - The pool manager to use for compilation
///
/// # Returns
///
/// * `Ok(usize)` - Number of plugins successfully precompiled
/// * `Err` - If precompilation fails critically
pub async fn precompile_plugins<PR: PluginRepositoryTrait>(
    plugin_repository: &PR,
    pool_manager: &PoolManager,
) -> eyre::Result<usize> {
    use crate::models::PaginationQuery;

    let query = PaginationQuery {
        page: 1,
        per_page: 1000,
    };

    let plugins = plugin_repository
        .list_paginated(query)
        .await
        .map_err(|e| eyre::eyre!("Failed to list plugins: {}", e))?;

    let mut compiled_count = 0;

    for plugin in plugins.items {
        let plugin_path = if plugin.path.starts_with("plugins/") {
            plugin.path.clone()
        } else {
            format!("plugins/{}", plugin.path)
        };

        match pool_manager
            .precompile_plugin(plugin.id.clone(), Some(plugin_path), None)
            .await
        {
            Ok(compiled_code) => {
                if let Err(e) = pool_manager
                    .cache_compiled_code(plugin.id.clone(), compiled_code)
                    .await
                {
                    warn!(
                        plugin_id = %plugin.id,
                        error = %e,
                        "Failed to cache compiled plugin code"
                    );
                } else {
                    compiled_count += 1;
                    info!(plugin_id = %plugin.id, "Plugin precompiled successfully");
                }
            }
            Err(e) => {
                warn!(
                    plugin_id = %plugin.id,
                    error = %e,
                    "Failed to precompile plugin"
                );
            }
        }
    }

    info!(
        compiled_count = compiled_count,
        total_plugins = plugins.total,
        "Plugin precompilation complete"
    );

    Ok(compiled_count)
}

/// Shutdown the plugin pool gracefully.
///
/// This should be called during application shutdown to properly
/// terminate the worker pool and clean up resources.
pub async fn shutdown_plugin_pool() -> eyre::Result<()> {
    let pool_manager = get_pool_manager();
    pool_manager
        .shutdown()
        .await
        .map_err(|e| eyre::eyre!("Failed to shutdown plugin pool: {}", e))?;
    info!("Plugin worker pool shutdown complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repositories::MockPluginRepositoryTrait;

    #[tokio::test]
    async fn test_initialize_plugin_pool_no_plugins() {
        let mut mock_repo = MockPluginRepositoryTrait::new();
        mock_repo
            .expect_has_entries()
            .returning(|| Box::pin(async { Ok(false) }));

        let result = initialize_plugin_pool(&mock_repo).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }
}
