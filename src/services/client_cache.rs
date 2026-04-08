//! # Client Cache
//!
//! Typed caching primitives for long-lived SDK and HTTP clients.
//!
//! - [`AsyncClientCache`] — for client construction that requires `.await`
//!   (e.g., AWS KMS via `aws_config::load().await`)
//! - [`SyncClientCache`] — for synchronous client constructors
//!   (e.g., `soroban_rs::Client::new(url)`, Solana `RpcClient::new(...)`)
//!
//! Both guarantee at most one in-flight initializer per key. If `init` returns
//! `Err`, the entry is not cached and subsequent calls will retry initialization.

use std::{
    hash::Hash,
    sync::{Arc, Mutex},
};

use dashmap::DashMap;
use tokio::sync::OnceCell;

// ---------------------------------------------------------------------------
// AsyncClientCache
// ---------------------------------------------------------------------------

/// A thread-safe cache for async client construction. At most one caller
/// runs the init closure per key; others `.await` the same result.
/// If init returns `Err`, the entry remains uninitialized and the next caller retries.
#[derive(Debug)]
pub struct AsyncClientCache<K: Eq + Hash, V> {
    entries: DashMap<K, Arc<OnceCell<Arc<V>>>>,
}

impl<K: Eq + Hash + Clone, V> AsyncClientCache<K, V> {
    pub fn new() -> Self {
        Self {
            entries: DashMap::new(),
        }
    }

    pub async fn get_or_try_init<E, F, Fut>(&self, key: K, init: F) -> Result<Arc<V>, E>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<V, E>>,
    {
        let cell = self
            .entries
            .entry(key)
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();

        let value = cell
            .get_or_try_init(|| async { Ok(Arc::new(init().await?)) })
            .await?;

        Ok(Arc::clone(value))
    }

    #[cfg(test)]
    pub fn remove(&self, key: &K) {
        self.entries.remove(key);
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

impl<K: Eq + Hash + Clone, V> Default for AsyncClientCache<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// SyncClientCache
// ---------------------------------------------------------------------------

/// A thread-safe cache for synchronous client construction. At most one caller
/// runs the init closure per key; others block on a mutex and receive the same result.
/// If init returns `Err`, the entry remains uninitialized and the next caller retries.
#[derive(Debug)]
pub struct SyncClientCache<K: Eq + Hash, V> {
    entries: DashMap<K, Arc<Mutex<Option<Arc<V>>>>>,
}

impl<K: Eq + Hash + Clone, V> SyncClientCache<K, V> {
    pub fn new() -> Self {
        Self {
            entries: DashMap::new(),
        }
    }

    pub fn get_or_try_init<E, F>(&self, key: K, init: F) -> Result<Arc<V>, E>
    where
        F: FnOnce() -> Result<V, E>,
    {
        let cell = self
            .entries
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(None)))
            .clone();

        let mut guard = cell.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(value) = &*guard {
            return Ok(Arc::clone(value));
        }

        let value = Arc::new(init()?);
        *guard = Some(Arc::clone(&value));
        Ok(value)
    }

    #[cfg(test)]
    pub fn remove(&self, key: &K) {
        self.entries.remove(key);
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

impl<K: Eq + Hash + Clone, V> Default for SyncClientCache<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ── AsyncClientCache ─────────────────────────────────────────────

    #[tokio::test]
    async fn async_same_key_returns_same_arc() {
        let cache = AsyncClientCache::<String, String>::new();

        let v1 = cache
            .get_or_try_init("key1".to_string(), || async {
                Ok::<_, String>("value1".to_string())
            })
            .await
            .unwrap();

        let v2 = cache
            .get_or_try_init("key1".to_string(), || async {
                Ok::<_, String>("should_not_be_used".to_string())
            })
            .await
            .unwrap();

        assert!(Arc::ptr_eq(&v1, &v2));
        assert_eq!(*v1, "value1");
    }

    #[tokio::test]
    async fn async_different_keys_return_different_values() {
        let cache = AsyncClientCache::<String, String>::new();

        let v1 = cache
            .get_or_try_init("key1".to_string(), || async {
                Ok::<_, String>("value1".to_string())
            })
            .await
            .unwrap();

        let v2 = cache
            .get_or_try_init("key2".to_string(), || async {
                Ok::<_, String>("value2".to_string())
            })
            .await
            .unwrap();

        assert!(!Arc::ptr_eq(&v1, &v2));
        assert_eq!(cache.len(), 2);
    }

    #[tokio::test]
    async fn async_concurrent_access_creates_once() {
        let cache = Arc::new(AsyncClientCache::<String, String>::new());
        let init_count = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..50 {
            let cache = Arc::clone(&cache);
            let count = Arc::clone(&init_count);
            handles.push(tokio::spawn(async move {
                cache
                    .get_or_try_init("shared_key".to_string(), || {
                        let count = Arc::clone(&count);
                        async move {
                            count.fetch_add(1, Ordering::SeqCst);
                            tokio::task::yield_now().await;
                            Ok::<_, String>("shared_value".to_string())
                        }
                    })
                    .await
                    .unwrap()
            }));
        }

        let results: Vec<Arc<String>> = futures::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        for result in &results {
            assert!(Arc::ptr_eq(result, &results[0]));
        }
        assert_eq!(init_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn async_error_does_not_cache() {
        let cache = AsyncClientCache::<String, String>::new();

        let result = cache
            .get_or_try_init("key1".to_string(), || async {
                Err::<String, _>("fail".to_string())
            })
            .await;
        assert!(result.is_err());

        let v = cache
            .get_or_try_init("key1".to_string(), || async {
                Ok::<_, String>("recovered".to_string())
            })
            .await
            .unwrap();
        assert_eq!(*v, "recovered");
    }

    #[tokio::test]
    async fn async_remove_allows_reinit() {
        let cache = AsyncClientCache::<String, String>::new();

        let v1 = cache
            .get_or_try_init("key1".to_string(), || async {
                Ok::<_, String>("first".to_string())
            })
            .await
            .unwrap();

        cache.remove(&"key1".to_string());

        let v2 = cache
            .get_or_try_init("key1".to_string(), || async {
                Ok::<_, String>("second".to_string())
            })
            .await
            .unwrap();

        assert!(!Arc::ptr_eq(&v1, &v2));
        assert_eq!(*v2, "second");
    }

    // ── SyncClientCache ──────────────────────────────────────────────

    #[test]
    fn sync_same_key_returns_same_arc() {
        let cache = SyncClientCache::<String, String>::new();

        let v1 = cache
            .get_or_try_init("key1".to_string(), || Ok::<_, String>("value1".to_string()))
            .unwrap();

        let v2 = cache
            .get_or_try_init("key1".to_string(), || {
                Ok::<_, String>("should_not_be_used".to_string())
            })
            .unwrap();

        assert!(Arc::ptr_eq(&v1, &v2));
        assert_eq!(*v1, "value1");
    }

    #[test]
    fn sync_different_keys_return_different_values() {
        let cache = SyncClientCache::<String, String>::new();

        let v1 = cache
            .get_or_try_init("key1".to_string(), || Ok::<_, String>("value1".to_string()))
            .unwrap();

        let v2 = cache
            .get_or_try_init("key2".to_string(), || Ok::<_, String>("value2".to_string()))
            .unwrap();

        assert!(!Arc::ptr_eq(&v1, &v2));
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn sync_concurrent_access_creates_once() {
        let cache = Arc::new(SyncClientCache::<String, String>::new());
        let init_count = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..50 {
            let cache = Arc::clone(&cache);
            let count = Arc::clone(&init_count);
            handles.push(std::thread::spawn(move || {
                cache
                    .get_or_try_init("shared_key".to_string(), || {
                        count.fetch_add(1, Ordering::SeqCst);
                        std::thread::yield_now();
                        Ok::<_, String>("shared_value".to_string())
                    })
                    .unwrap()
            }));
        }

        let results: Vec<Arc<String>> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        for result in &results {
            assert!(Arc::ptr_eq(result, &results[0]));
        }
        assert_eq!(init_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn sync_error_does_not_cache() {
        let cache = SyncClientCache::<String, String>::new();

        let result =
            cache.get_or_try_init("key1".to_string(), || Err::<String, _>("fail".to_string()));
        assert!(result.is_err());

        let v = cache
            .get_or_try_init("key1".to_string(), || {
                Ok::<_, String>("recovered".to_string())
            })
            .unwrap();
        assert_eq!(*v, "recovered");
    }

    #[test]
    fn sync_remove_allows_reinit() {
        let cache = SyncClientCache::<String, String>::new();

        let v1 = cache
            .get_or_try_init("key1".to_string(), || Ok::<_, String>("first".to_string()))
            .unwrap();

        cache.remove(&"key1".to_string());

        let v2 = cache
            .get_or_try_init("key1".to_string(), || Ok::<_, String>("second".to_string()))
            .unwrap();

        assert!(!Arc::ptr_eq(&v1, &v2));
        assert_eq!(*v2, "second");
    }
}
