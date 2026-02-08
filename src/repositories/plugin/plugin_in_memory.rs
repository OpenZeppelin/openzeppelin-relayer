//! This module provides an in-memory implementation of plugins.
//!
//! The `InMemoryPluginRepository` struct is used to store and retrieve plugins
//! script paths for further execution. Also provides compiled code caching.
use crate::{
    models::{PaginationQuery, PluginModel},
    repositories::{PaginatedResult, PluginRepositoryTrait, RepositoryError},
};

use async_trait::async_trait;

use std::collections::HashMap;
use tokio::sync::{Mutex, MutexGuard};

/// Compiled plugin code entry
#[derive(Debug, Clone)]
struct CompiledCodeEntry {
    code: String,
    source_hash: Option<String>,
}

#[derive(Debug)]
pub struct InMemoryPluginRepository {
    store: Mutex<HashMap<String, PluginModel>>,
    compiled_cache: Mutex<HashMap<String, CompiledCodeEntry>>,
}

impl Clone for InMemoryPluginRepository {
    fn clone(&self) -> Self {
        // Try to get the current data, or use empty HashMap if lock fails
        let data = self
            .store
            .try_lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| HashMap::new());

        let compiled = self
            .compiled_cache
            .try_lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| HashMap::new());

        Self {
            store: Mutex::new(data),
            compiled_cache: Mutex::new(compiled),
        }
    }
}

impl InMemoryPluginRepository {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
            compiled_cache: Mutex::new(HashMap::new()),
        }
    }

    pub async fn get_by_id(&self, id: &str) -> Result<Option<PluginModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        Ok(store.get(id).cloned())
    }

    async fn acquire_lock<T>(lock: &Mutex<T>) -> Result<MutexGuard<T>, RepositoryError> {
        Ok(lock.lock().await)
    }
}

impl Default for InMemoryPluginRepository {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PluginRepositoryTrait for InMemoryPluginRepository {
    async fn get_by_id(&self, id: &str) -> Result<Option<PluginModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        Ok(store.get(id).cloned())
    }

    async fn add(&self, plugin: PluginModel) -> Result<(), RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        store.insert(plugin.id.clone(), plugin);
        Ok(())
    }

    async fn update(&self, plugin: PluginModel) -> Result<PluginModel, RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        if !store.contains_key(&plugin.id) {
            return Err(RepositoryError::NotFound(format!(
                "Plugin with id {} not found",
                plugin.id
            )));
        }
        store.insert(plugin.id.clone(), plugin.clone());
        Ok(plugin)
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<PluginModel>, RepositoryError> {
        let total = self.count().await?;
        let start = ((query.page - 1) * query.per_page) as usize;

        let items = self
            .store
            .lock()
            .await
            .values()
            .skip(start)
            .take(query.per_page as usize)
            .cloned()
            .collect();

        Ok(PaginatedResult {
            items,
            total: total as u64,
            page: query.page,
            per_page: query.per_page,
        })
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        let store = self.store.lock().await;
        Ok(store.len())
    }

    async fn has_entries(&self) -> Result<bool, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        Ok(!store.is_empty())
    }

    async fn drop_all_entries(&self) -> Result<(), RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        store.clear();
        Ok(())
    }

    // Compiled code cache methods

    async fn get_compiled_code(&self, plugin_id: &str) -> Result<Option<String>, RepositoryError> {
        let cache = Self::acquire_lock(&self.compiled_cache).await?;
        Ok(cache.get(plugin_id).map(|e| e.code.clone()))
    }

    async fn store_compiled_code(
        &self,
        plugin_id: &str,
        compiled_code: &str,
        source_hash: Option<&str>,
    ) -> Result<(), RepositoryError> {
        let mut cache = Self::acquire_lock(&self.compiled_cache).await?;
        cache.insert(
            plugin_id.to_string(),
            CompiledCodeEntry {
                code: compiled_code.to_string(),
                source_hash: source_hash.map(|s| s.to_string()),
            },
        );
        Ok(())
    }

    async fn invalidate_compiled_code(&self, plugin_id: &str) -> Result<(), RepositoryError> {
        let mut cache = Self::acquire_lock(&self.compiled_cache).await?;
        cache.remove(plugin_id);
        Ok(())
    }

    async fn invalidate_all_compiled_code(&self) -> Result<(), RepositoryError> {
        let mut cache = Self::acquire_lock(&self.compiled_cache).await?;
        cache.clear();
        Ok(())
    }

    async fn has_compiled_code(&self, plugin_id: &str) -> Result<bool, RepositoryError> {
        let cache = Self::acquire_lock(&self.compiled_cache).await?;
        Ok(cache.contains_key(plugin_id))
    }

    async fn get_source_hash(&self, plugin_id: &str) -> Result<Option<String>, RepositoryError> {
        let cache = Self::acquire_lock(&self.compiled_cache).await?;
        Ok(cache.get(plugin_id).and_then(|e| e.source_hash.clone()))
    }
}

#[cfg(test)]
mod tests {
    use crate::{config::PluginFileConfig, constants::DEFAULT_PLUGIN_TIMEOUT_SECONDS};

    use super::*;
    use std::{sync::Arc, time::Duration};

    // ============================================
    // Helper functions
    // ============================================

    fn create_test_plugin(id: &str) -> PluginModel {
        PluginModel {
            id: id.to_string(),
            path: format!("path/{id}"),
            timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS),
            emit_logs: false,
            emit_traces: false,
            raw_response: false,
            allow_get_invocation: false,
            config: None,
            forward_logs: false,
        }
    }

    fn create_test_plugin_with_options(
        id: &str,
        emit_logs: bool,
        emit_traces: bool,
        raw_response: bool,
    ) -> PluginModel {
        PluginModel {
            id: id.to_string(),
            path: format!("path/{id}"),
            timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS),
            emit_logs,
            emit_traces,
            raw_response,
            allow_get_invocation: false,
            config: None,
            forward_logs: false,
        }
    }

    // ============================================
    // Basic repository tests
    // ============================================

    #[tokio::test]
    async fn test_new_creates_empty_repository() {
        let repo = InMemoryPluginRepository::new();

        assert_eq!(repo.count().await.unwrap(), 0);
        assert!(!repo.has_entries().await.unwrap());
    }

    #[tokio::test]
    async fn test_default_creates_empty_repository() {
        let repo = InMemoryPluginRepository::default();

        assert_eq!(repo.count().await.unwrap(), 0);
        assert!(!repo.has_entries().await.unwrap());
    }

    #[tokio::test]
    async fn test_add_and_get_by_id() {
        let repo = Arc::new(InMemoryPluginRepository::new());

        let plugin = create_test_plugin("test-plugin");
        repo.add(plugin.clone()).await.unwrap();

        let retrieved = repo.get_by_id("test-plugin").await.unwrap();
        assert_eq!(retrieved, Some(plugin));
    }

    #[tokio::test]
    async fn test_get_nonexistent_plugin() {
        let repo = Arc::new(InMemoryPluginRepository::new());

        let result = repo.get_by_id("nonexistent").await;
        assert!(matches!(result, Ok(None)));
    }

    #[tokio::test]
    async fn test_add_multiple_plugins() {
        let repo = Arc::new(InMemoryPluginRepository::new());

        for i in 1..=5 {
            let plugin = create_test_plugin(&format!("plugin-{i}"));
            repo.add(plugin).await.unwrap();
        }

        assert_eq!(repo.count().await.unwrap(), 5);

        for i in 1..=5 {
            let result = repo.get_by_id(&format!("plugin-{i}")).await.unwrap();
            assert!(result.is_some());
        }
    }

    #[tokio::test]
    async fn test_add_overwrites_existing() {
        let repo = Arc::new(InMemoryPluginRepository::new());

        let plugin1 = create_test_plugin_with_options("test-plugin", false, false, false);
        repo.add(plugin1).await.unwrap();

        let plugin2 = create_test_plugin_with_options("test-plugin", true, true, true);
        repo.add(plugin2.clone()).await.unwrap();

        // Should have overwritten
        let retrieved = repo.get_by_id("test-plugin").await.unwrap().unwrap();
        assert!(retrieved.emit_logs);
        assert!(retrieved.emit_traces);
        assert!(retrieved.raw_response);

        // Count should still be 1
        assert_eq!(repo.count().await.unwrap(), 1);
    }

    // ============================================
    // Update tests
    // ============================================

    #[tokio::test]
    async fn test_update_existing_plugin() {
        let repo = Arc::new(InMemoryPluginRepository::new());

        let plugin = create_test_plugin("test-plugin");
        repo.add(plugin).await.unwrap();

        let updated_plugin = create_test_plugin_with_options("test-plugin", true, true, true);
        let result = repo.update(updated_plugin.clone()).await;

        assert!(result.is_ok());
        let returned = result.unwrap();
        assert_eq!(returned.id, "test-plugin");
        assert!(returned.emit_logs);
        assert!(returned.emit_traces);

        // Verify persisted
        let retrieved = repo.get_by_id("test-plugin").await.unwrap().unwrap();
        assert!(retrieved.emit_logs);
    }

    #[tokio::test]
    async fn test_update_nonexistent_plugin_returns_error() {
        let repo = Arc::new(InMemoryPluginRepository::new());

        let plugin = create_test_plugin("nonexistent");
        let result = repo.update(plugin).await;

        assert!(result.is_err());
        match result {
            Err(RepositoryError::NotFound(msg)) => {
                assert!(msg.contains("nonexistent"));
            }
            _ => panic!("Expected NotFound error"),
        }
    }

    #[tokio::test]
    async fn test_update_preserves_other_plugins() {
        let repo = Arc::new(InMemoryPluginRepository::new());

        repo.add(create_test_plugin("plugin-1")).await.unwrap();
        repo.add(create_test_plugin("plugin-2")).await.unwrap();
        repo.add(create_test_plugin("plugin-3")).await.unwrap();

        let updated = create_test_plugin_with_options("plugin-2", true, false, false);
        repo.update(updated).await.unwrap();

        // Check other plugins unchanged
        let p1 = repo.get_by_id("plugin-1").await.unwrap().unwrap();
        assert!(!p1.emit_logs);

        let p3 = repo.get_by_id("plugin-3").await.unwrap().unwrap();
        assert!(!p3.emit_logs);

        // Check updated plugin changed
        let p2 = repo.get_by_id("plugin-2").await.unwrap().unwrap();
        assert!(p2.emit_logs);
    }

    // ============================================
    // Count tests
    // ============================================

    #[tokio::test]
    async fn test_count_empty_repository() {
        let repo = InMemoryPluginRepository::new();
        assert_eq!(repo.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_count_with_entries() {
        let repo = Arc::new(InMemoryPluginRepository::new());

        for i in 1..=10 {
            repo.add(create_test_plugin(&format!("plugin-{i}")))
                .await
                .unwrap();
        }

        assert_eq!(repo.count().await.unwrap(), 10);
    }

    #[tokio::test]
    async fn test_count_after_drop_all() {
        let repo = Arc::new(InMemoryPluginRepository::new());

        for i in 1..=5 {
            repo.add(create_test_plugin(&format!("plugin-{i}")))
                .await
                .unwrap();
        }

        assert_eq!(repo.count().await.unwrap(), 5);

        repo.drop_all_entries().await.unwrap();

        assert_eq!(repo.count().await.unwrap(), 0);
    }

    // ============================================
    // Pagination tests
    // ============================================

    #[tokio::test]
    async fn test_list_paginated_first_page() {
        let repo = Arc::new(InMemoryPluginRepository::new());

        for i in 1..=10 {
            repo.add(create_test_plugin(&format!("plugin-{i:02}")))
                .await
                .unwrap();
        }

        let query = PaginationQuery {
            page: 1,
            per_page: 3,
        };

        let result = repo.list_paginated(query).await.unwrap();

        assert_eq!(result.items.len(), 3);
        assert_eq!(result.total, 10);
        assert_eq!(result.page, 1);
        assert_eq!(result.per_page, 3);
    }

    #[tokio::test]
    async fn test_list_paginated_middle_page() {
        let repo = Arc::new(InMemoryPluginRepository::new());

        for i in 1..=10 {
            repo.add(create_test_plugin(&format!("plugin-{i:02}")))
                .await
                .unwrap();
        }

        let query = PaginationQuery {
            page: 2,
            per_page: 3,
        };

        let result = repo.list_paginated(query).await.unwrap();

        assert_eq!(result.items.len(), 3);
        assert_eq!(result.total, 10);
        assert_eq!(result.page, 2);
    }

    #[tokio::test]
    async fn test_list_paginated_last_partial_page() {
        let repo = Arc::new(InMemoryPluginRepository::new());

        for i in 1..=10 {
            repo.add(create_test_plugin(&format!("plugin-{i:02}")))
                .await
                .unwrap();
        }

        let query = PaginationQuery {
            page: 4,
            per_page: 3,
        };

        let result = repo.list_paginated(query).await.unwrap();

        // 10 items, 3 per page: page 4 has only 1 item
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.total, 10);
    }

    #[tokio::test]
    async fn test_list_paginated_empty_repository() {
        let repo = Arc::new(InMemoryPluginRepository::new());

        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };

        let result = repo.list_paginated(query).await.unwrap();

        assert_eq!(result.items.len(), 0);
        assert_eq!(result.total, 0);
    }

    #[tokio::test]
    async fn test_list_paginated_page_beyond_data() {
        let repo = Arc::new(InMemoryPluginRepository::new());

        for i in 1..=5 {
            repo.add(create_test_plugin(&format!("plugin-{i}")))
                .await
                .unwrap();
        }

        let query = PaginationQuery {
            page: 10, // Way beyond available data
            per_page: 2,
        };

        let result = repo.list_paginated(query).await.unwrap();

        assert_eq!(result.items.len(), 0);
        assert_eq!(result.total, 5);
    }

    #[tokio::test]
    async fn test_list_paginated_large_per_page() {
        let repo = Arc::new(InMemoryPluginRepository::new());

        for i in 1..=5 {
            repo.add(create_test_plugin(&format!("plugin-{i}")))
                .await
                .unwrap();
        }

        let query = PaginationQuery {
            page: 1,
            per_page: 100, // More than available
        };

        let result = repo.list_paginated(query).await.unwrap();

        assert_eq!(result.items.len(), 5);
        assert_eq!(result.total, 5);
    }

    // ============================================
    // has_entries and drop_all tests
    // ============================================

    #[tokio::test]
    async fn test_has_entries_empty() {
        let repo = InMemoryPluginRepository::new();
        assert!(!repo.has_entries().await.unwrap());
    }

    #[tokio::test]
    async fn test_has_entries_with_data() {
        let repo = Arc::new(InMemoryPluginRepository::new());
        repo.add(create_test_plugin("test")).await.unwrap();
        assert!(repo.has_entries().await.unwrap());
    }

    #[tokio::test]
    async fn test_drop_all_entries_clears_store() {
        let repo = Arc::new(InMemoryPluginRepository::new());

        for i in 1..=5 {
            repo.add(create_test_plugin(&format!("plugin-{i}")))
                .await
                .unwrap();
        }

        assert!(repo.has_entries().await.unwrap());
        assert_eq!(repo.count().await.unwrap(), 5);

        repo.drop_all_entries().await.unwrap();

        assert!(!repo.has_entries().await.unwrap());
        assert_eq!(repo.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_drop_all_entries_on_empty_repo() {
        let repo = InMemoryPluginRepository::new();

        // Should not fail on empty repo
        let result = repo.drop_all_entries().await;
        assert!(result.is_ok());
    }

    // ============================================
    // Compiled code cache tests
    // ============================================

    #[tokio::test]
    async fn test_store_and_get_compiled_code() {
        let repo = InMemoryPluginRepository::new();

        repo.store_compiled_code("plugin-1", "compiled code here", None)
            .await
            .unwrap();

        let result = repo.get_compiled_code("plugin-1").await.unwrap();
        assert_eq!(result, Some("compiled code here".to_string()));
    }

    #[tokio::test]
    async fn test_get_compiled_code_nonexistent() {
        let repo = InMemoryPluginRepository::new();

        let result = repo.get_compiled_code("nonexistent").await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_store_compiled_code_with_source_hash() {
        let repo = InMemoryPluginRepository::new();

        repo.store_compiled_code("plugin-1", "code", Some("abc123hash"))
            .await
            .unwrap();

        let code = repo.get_compiled_code("plugin-1").await.unwrap();
        assert_eq!(code, Some("code".to_string()));

        let hash = repo.get_source_hash("plugin-1").await.unwrap();
        assert_eq!(hash, Some("abc123hash".to_string()));
    }

    #[tokio::test]
    async fn test_store_compiled_code_overwrites_existing() {
        let repo = InMemoryPluginRepository::new();

        repo.store_compiled_code("plugin-1", "old code", Some("oldhash"))
            .await
            .unwrap();

        repo.store_compiled_code("plugin-1", "new code", Some("newhash"))
            .await
            .unwrap();

        let code = repo.get_compiled_code("plugin-1").await.unwrap();
        assert_eq!(code, Some("new code".to_string()));

        let hash = repo.get_source_hash("plugin-1").await.unwrap();
        assert_eq!(hash, Some("newhash".to_string()));
    }

    #[tokio::test]
    async fn test_has_compiled_code() {
        let repo = InMemoryPluginRepository::new();

        assert!(!repo.has_compiled_code("plugin-1").await.unwrap());

        repo.store_compiled_code("plugin-1", "code", None)
            .await
            .unwrap();

        assert!(repo.has_compiled_code("plugin-1").await.unwrap());
        assert!(!repo.has_compiled_code("plugin-2").await.unwrap());
    }

    #[tokio::test]
    async fn test_invalidate_compiled_code() {
        let repo = InMemoryPluginRepository::new();

        repo.store_compiled_code("plugin-1", "code1", None)
            .await
            .unwrap();
        repo.store_compiled_code("plugin-2", "code2", None)
            .await
            .unwrap();

        assert!(repo.has_compiled_code("plugin-1").await.unwrap());
        assert!(repo.has_compiled_code("plugin-2").await.unwrap());

        repo.invalidate_compiled_code("plugin-1").await.unwrap();

        assert!(!repo.has_compiled_code("plugin-1").await.unwrap());
        assert!(repo.has_compiled_code("plugin-2").await.unwrap());
    }

    #[tokio::test]
    async fn test_invalidate_compiled_code_nonexistent() {
        let repo = InMemoryPluginRepository::new();

        // Should not fail on nonexistent
        let result = repo.invalidate_compiled_code("nonexistent").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_invalidate_all_compiled_code() {
        let repo = InMemoryPluginRepository::new();

        for i in 1..=5 {
            repo.store_compiled_code(&format!("plugin-{i}"), &format!("code-{i}"), None)
                .await
                .unwrap();
        }

        for i in 1..=5 {
            assert!(repo
                .has_compiled_code(&format!("plugin-{i}"))
                .await
                .unwrap());
        }

        repo.invalidate_all_compiled_code().await.unwrap();

        for i in 1..=5 {
            assert!(!repo
                .has_compiled_code(&format!("plugin-{i}"))
                .await
                .unwrap());
        }
    }

    #[tokio::test]
    async fn test_invalidate_all_compiled_code_empty() {
        let repo = InMemoryPluginRepository::new();

        // Should not fail on empty cache
        let result = repo.invalidate_all_compiled_code().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_source_hash() {
        let repo = InMemoryPluginRepository::new();

        // No hash stored
        repo.store_compiled_code("plugin-1", "code", None)
            .await
            .unwrap();
        let hash = repo.get_source_hash("plugin-1").await.unwrap();
        assert_eq!(hash, None);

        // Hash stored
        repo.store_compiled_code("plugin-2", "code", Some("sha256:abc"))
            .await
            .unwrap();
        let hash = repo.get_source_hash("plugin-2").await.unwrap();
        assert_eq!(hash, Some("sha256:abc".to_string()));
    }

    #[tokio::test]
    async fn test_get_source_hash_nonexistent() {
        let repo = InMemoryPluginRepository::new();

        let hash = repo.get_source_hash("nonexistent").await.unwrap();
        assert_eq!(hash, None);
    }

    // ============================================
    // Clone tests
    // ============================================

    #[tokio::test]
    async fn test_clone_copies_store_data() {
        let repo = InMemoryPluginRepository::new();
        repo.add(create_test_plugin("plugin-1")).await.unwrap();
        repo.add(create_test_plugin("plugin-2")).await.unwrap();

        let cloned = repo.clone();

        // Cloned should have same data
        assert_eq!(cloned.count().await.unwrap(), 2);
        assert!(cloned.get_by_id("plugin-1").await.unwrap().is_some());
        assert!(cloned.get_by_id("plugin-2").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_clone_copies_compiled_cache() {
        let repo = InMemoryPluginRepository::new();
        repo.store_compiled_code("plugin-1", "code1", Some("hash1"))
            .await
            .unwrap();

        let cloned = repo.clone();

        // Cloned should have same compiled cache
        assert!(cloned.has_compiled_code("plugin-1").await.unwrap());
        assert_eq!(
            cloned.get_compiled_code("plugin-1").await.unwrap(),
            Some("code1".to_string())
        );
        assert_eq!(
            cloned.get_source_hash("plugin-1").await.unwrap(),
            Some("hash1".to_string())
        );
    }

    #[tokio::test]
    async fn test_clone_is_independent() {
        let repo = InMemoryPluginRepository::new();
        repo.add(create_test_plugin("plugin-1")).await.unwrap();

        let cloned = repo.clone();

        // Modify original
        repo.add(create_test_plugin("plugin-2")).await.unwrap();

        // Clone should not have the new plugin (independent copy)
        // Note: This tests the independence after clone
        assert_eq!(repo.count().await.unwrap(), 2);
        // cloned was made before plugin-2 was added, so it only has 1
        assert_eq!(cloned.count().await.unwrap(), 1);
    }

    // ============================================
    // PluginModel conversion tests
    // ============================================

    #[tokio::test]
    async fn test_plugin_model_try_from_config() {
        let config = PluginFileConfig {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
            timeout: None,
            emit_logs: false,
            emit_traces: false,
            raw_response: false,
            allow_get_invocation: false,
            config: None,
            forward_logs: false,
        };

        let result = PluginModel::try_from(config);
        assert!(result.is_ok());

        let plugin = result.unwrap();
        assert_eq!(plugin.id, "test-plugin");
        assert_eq!(plugin.path, "test-path");
        assert_eq!(
            plugin.timeout,
            Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS)
        );
    }

    #[tokio::test]
    async fn test_plugin_model_try_from_config_with_timeout() {
        let mut config_map = serde_json::Map::new();
        config_map.insert("key".to_string(), serde_json::json!("value"));

        let config = PluginFileConfig {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
            timeout: Some(120),
            emit_logs: true,
            emit_traces: true,
            raw_response: true,
            allow_get_invocation: true,
            config: Some(config_map),
            forward_logs: true,
        };

        let result = PluginModel::try_from(config);
        assert!(result.is_ok());

        let plugin = result.unwrap();
        assert_eq!(plugin.timeout, Duration::from_secs(120));
        assert!(plugin.emit_logs);
        assert!(plugin.emit_traces);
        assert!(plugin.raw_response);
        assert!(plugin.allow_get_invocation);
        assert!(plugin.config.is_some());
        assert!(plugin.forward_logs);
    }

    // ============================================
    // Compiled cache independence from store
    // ============================================

    #[tokio::test]
    async fn test_compiled_cache_independent_of_plugin_store() {
        let repo = InMemoryPluginRepository::new();

        // Store compiled code without adding plugin to store
        repo.store_compiled_code("plugin-1", "compiled", None)
            .await
            .unwrap();

        // Plugin doesn't exist in store
        assert!(repo.get_by_id("plugin-1").await.unwrap().is_none());

        // But compiled code exists in cache
        assert!(repo.has_compiled_code("plugin-1").await.unwrap());
    }

    #[tokio::test]
    async fn test_drop_all_entries_does_not_clear_compiled_cache() {
        let repo = InMemoryPluginRepository::new();

        repo.add(create_test_plugin("plugin-1")).await.unwrap();
        repo.store_compiled_code("plugin-1", "compiled", None)
            .await
            .unwrap();

        repo.drop_all_entries().await.unwrap();

        // Plugin gone
        assert!(repo.get_by_id("plugin-1").await.unwrap().is_none());

        // But compiled cache still has entry
        assert!(repo.has_compiled_code("plugin-1").await.unwrap());
    }

    #[tokio::test]
    async fn test_invalidate_all_compiled_does_not_clear_store() {
        let repo = InMemoryPluginRepository::new();

        repo.add(create_test_plugin("plugin-1")).await.unwrap();
        repo.store_compiled_code("plugin-1", "compiled", None)
            .await
            .unwrap();

        repo.invalidate_all_compiled_code().await.unwrap();

        // Compiled cache cleared
        assert!(!repo.has_compiled_code("plugin-1").await.unwrap());

        // But plugin still exists in store
        assert!(repo.get_by_id("plugin-1").await.unwrap().is_some());
    }
}
