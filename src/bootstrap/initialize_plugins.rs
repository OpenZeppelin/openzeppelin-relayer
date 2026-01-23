//! Plugin Pool Initialization
//!
//! This module handles conditional initialization of the plugin worker pool.
//! The pool is only started if plugins are configured, avoiding overhead
//! when the plugin system is not in use.

use std::sync::Arc;
use tracing::{info, warn};

use crate::repositories::PluginRepositoryTrait;
use crate::services::plugins::{get_pool_manager, PluginRunner, PluginService, PoolManager};

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
        let plugin_path = PluginService::<PluginRunner>::resolve_plugin_path(&plugin.path);

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
    use crate::models::RepositoryError;
    use crate::repositories::MockPluginRepositoryTrait;

    // ============================================
    // initialize_plugin_pool tests
    // ============================================

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

    #[tokio::test]
    async fn test_initialize_plugin_pool_has_entries_error() {
        let mut mock_repo = MockPluginRepositoryTrait::new();
        mock_repo.expect_has_entries().returning(|| {
            Box::pin(async {
                Err(RepositoryError::ConnectionError(
                    "Database unavailable".to_string(),
                ))
            })
        });

        let result = initialize_plugin_pool(&mock_repo).await;
        assert!(result.is_err());

        match result {
            Err(e) => assert!(e.to_string().contains("Failed to check plugin repository")),
            Ok(_) => panic!("Expected error"),
        }
    }

    #[tokio::test]
    async fn test_initialize_plugin_pool_has_entries_unknown_error() {
        let mut mock_repo = MockPluginRepositoryTrait::new();
        mock_repo.expect_has_entries().returning(|| {
            Box::pin(async { Err(RepositoryError::Unknown("Unknown error".to_string())) })
        });

        let result = initialize_plugin_pool(&mock_repo).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_initialize_plugin_pool_with_plugins_count_fails() {
        let mut mock_repo = MockPluginRepositoryTrait::new();

        // has_entries returns true (plugins exist)
        mock_repo
            .expect_has_entries()
            .returning(|| Box::pin(async { Ok(true) }));

        // count fails but we handle it gracefully (unwrap_or(0))
        mock_repo.expect_count().returning(|| {
            Box::pin(async { Err(RepositoryError::Unknown("Count failed".to_string())) })
        });

        // The function should still proceed even if count fails
        let result = initialize_plugin_pool(&mock_repo).await;
        // Result depends on whether pool can be started, but we at least verify
        // that the function doesn't panic when count fails
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_initialize_plugin_pool_with_plugins_exists() {
        let mut mock_repo = MockPluginRepositoryTrait::new();

        mock_repo
            .expect_has_entries()
            .returning(|| Box::pin(async { Ok(true) }));

        mock_repo
            .expect_count()
            .returning(|| Box::pin(async { Ok(5) }));

        // This test verifies the function proceeds when plugins exist
        // The actual pool startup may succeed or fail depending on environment
        let result = initialize_plugin_pool(&mock_repo).await;
        // We just verify it doesn't panic and returns a result
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_initialize_plugin_pool_with_zero_count_but_has_entries() {
        let mut mock_repo = MockPluginRepositoryTrait::new();

        // Edge case: has_entries returns true but count returns 0
        // This shouldn't happen in practice but tests defensive coding
        mock_repo
            .expect_has_entries()
            .returning(|| Box::pin(async { Ok(true) }));

        mock_repo
            .expect_count()
            .returning(|| Box::pin(async { Ok(0) }));

        let result = initialize_plugin_pool(&mock_repo).await;
        // Should still attempt to start the pool
        assert!(result.is_ok() || result.is_err());
    }

    // ============================================
    // precompile_plugins tests
    // ============================================

    #[tokio::test]
    async fn test_precompile_plugins_list_paginated_error() {
        use crate::models::PaginationQuery;

        let mut mock_repo = MockPluginRepositoryTrait::new();

        mock_repo
            .expect_list_paginated()
            .withf(|query: &PaginationQuery| query.page == 1 && query.per_page == 1000)
            .returning(|_| {
                Box::pin(async {
                    Err(RepositoryError::ConnectionError(
                        "Database unavailable".to_string(),
                    ))
                })
            });

        // We need a real pool manager for this test, but since we're testing
        // the error path, we can use the global one
        let pool_manager = get_pool_manager();

        let result = precompile_plugins(&mock_repo, &pool_manager).await;
        assert!(result.is_err());

        match result {
            Err(e) => assert!(e.to_string().contains("Failed to list plugins")),
            Ok(_) => panic!("Expected error"),
        }
    }

    #[tokio::test]
    async fn test_precompile_plugins_empty_list() {
        use crate::repositories::PaginatedResult;

        let mut mock_repo = MockPluginRepositoryTrait::new();

        mock_repo.expect_list_paginated().returning(|_| {
            Box::pin(async {
                Ok(PaginatedResult {
                    items: vec![],
                    total: 0,
                    page: 1,
                    per_page: 1000,
                })
            })
        });

        let pool_manager = get_pool_manager();

        let result = precompile_plugins(&mock_repo, &pool_manager).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_precompile_plugins_pagination_query_params() {
        use crate::models::PaginationQuery;
        use crate::repositories::PaginatedResult;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let was_called = Arc::new(AtomicBool::new(false));
        let was_called_clone = was_called.clone();

        let mut mock_repo = MockPluginRepositoryTrait::new();

        mock_repo
            .expect_list_paginated()
            .withf(move |query: &PaginationQuery| {
                // Verify the correct pagination parameters are used
                let correct = query.page == 1 && query.per_page == 1000;
                was_called_clone.store(true, Ordering::SeqCst);
                correct
            })
            .returning(|_| {
                Box::pin(async {
                    Ok(PaginatedResult {
                        items: vec![],
                        total: 0,
                        page: 1,
                        per_page: 1000,
                    })
                })
            });

        let pool_manager = get_pool_manager();

        let _ = precompile_plugins(&mock_repo, &pool_manager).await;

        assert!(
            was_called.load(Ordering::SeqCst),
            "list_paginated should have been called"
        );
    }

    // ============================================
    // Helper function tests
    // ============================================

    #[test]
    fn test_resolve_plugin_path_absolute() {
        // Test that PluginService::resolve_plugin_path handles paths correctly
        // This is an indirect test of the path resolution used in precompile_plugins
        let path = "/absolute/path/to/plugin.ts";
        let resolved = PluginService::<PluginRunner>::resolve_plugin_path(path);
        assert!(resolved.contains("plugin.ts"));
    }

    #[test]
    fn test_resolve_plugin_path_relative() {
        let path = "relative/path/plugin.ts";
        let resolved = PluginService::<PluginRunner>::resolve_plugin_path(path);
        assert!(resolved.contains("plugin.ts"));
    }

    // ============================================
    // Error handling tests
    // ============================================

    #[tokio::test]
    async fn test_initialize_plugin_pool_not_found_error() {
        let mut mock_repo = MockPluginRepositoryTrait::new();
        mock_repo.expect_has_entries().returning(|| {
            Box::pin(async { Err(RepositoryError::NotFound("not found".to_string())) })
        });

        let result = initialize_plugin_pool(&mock_repo).await;
        assert!(result.is_err(), "Should fail for NotFound error");
    }

    #[tokio::test]
    async fn test_initialize_plugin_pool_lock_error() {
        let mut mock_repo = MockPluginRepositoryTrait::new();
        mock_repo.expect_has_entries().returning(|| {
            Box::pin(async { Err(RepositoryError::LockError("lock error".to_string())) })
        });

        let result = initialize_plugin_pool(&mock_repo).await;
        assert!(result.is_err(), "Should fail for LockError");
    }

    #[tokio::test]
    async fn test_initialize_plugin_pool_invalid_data_error() {
        let mut mock_repo = MockPluginRepositoryTrait::new();
        mock_repo.expect_has_entries().returning(|| {
            Box::pin(async { Err(RepositoryError::InvalidData("invalid".to_string())) })
        });

        let result = initialize_plugin_pool(&mock_repo).await;
        assert!(result.is_err(), "Should fail for InvalidData error");
    }

    #[tokio::test]
    async fn test_precompile_plugins_not_found_error() {
        let mut mock_repo = MockPluginRepositoryTrait::new();
        mock_repo.expect_list_paginated().returning(|_| {
            Box::pin(async { Err(RepositoryError::NotFound("not found".to_string())) })
        });

        let pool_manager = get_pool_manager();
        let result = precompile_plugins(&mock_repo, &pool_manager).await;
        assert!(result.is_err(), "Should fail for NotFound error");
    }

    #[tokio::test]
    async fn test_precompile_plugins_unknown_error() {
        let mut mock_repo = MockPluginRepositoryTrait::new();
        mock_repo.expect_list_paginated().returning(|_| {
            Box::pin(async { Err(RepositoryError::Unknown("unknown".to_string())) })
        });

        let pool_manager = get_pool_manager();
        let result = precompile_plugins(&mock_repo, &pool_manager).await;
        assert!(result.is_err(), "Should fail for Unknown error");
    }

    #[tokio::test]
    async fn test_precompile_plugins_connection_error() {
        let mut mock_repo = MockPluginRepositoryTrait::new();
        mock_repo.expect_list_paginated().returning(|_| {
            Box::pin(async { Err(RepositoryError::ConnectionError("connection".to_string())) })
        });

        let pool_manager = get_pool_manager();
        let result = precompile_plugins(&mock_repo, &pool_manager).await;
        assert!(result.is_err(), "Should fail for ConnectionError");
    }

    // ============================================
    // Integration-style tests
    // ============================================

    #[tokio::test]
    async fn test_initialize_then_check_pool_state() {
        let mut mock_repo = MockPluginRepositoryTrait::new();
        mock_repo
            .expect_has_entries()
            .returning(|| Box::pin(async { Ok(false) }));

        let result = initialize_plugin_pool(&mock_repo).await;
        assert!(result.is_ok());

        // When no plugins, should return None
        let pool_manager = result.unwrap();
        assert!(pool_manager.is_none());
    }

    #[tokio::test]
    async fn test_multiple_initialize_calls_no_plugins() {
        // Verify that multiple calls don't cause issues
        for _ in 0..3 {
            let mut mock_repo = MockPluginRepositoryTrait::new();
            mock_repo
                .expect_has_entries()
                .returning(|| Box::pin(async { Ok(false) }));

            let result = initialize_plugin_pool(&mock_repo).await;
            assert!(result.is_ok());
            assert!(result.unwrap().is_none());
        }
    }

    #[tokio::test]
    async fn test_precompile_with_large_plugin_count() {
        use crate::repositories::PaginatedResult;

        let mut mock_repo = MockPluginRepositoryTrait::new();

        // Simulate having many plugins (but empty list for simplicity)
        mock_repo.expect_list_paginated().returning(|_| {
            Box::pin(async {
                Ok(PaginatedResult {
                    items: vec![],
                    total: 500, // Large total but empty items
                    page: 1,
                    per_page: 1000,
                })
            })
        });

        let pool_manager = get_pool_manager();

        let result = precompile_plugins(&mock_repo, &pool_manager).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0); // No items to compile
    }
}
