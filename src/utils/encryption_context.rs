//! Task-local context for AAD (Additional Authenticated Data) in encryption operations.
//!
//! This module provides a mechanism to pass AAD context through the async task stack
//! without explicitly threading it through function signatures. This is particularly
//! useful for serde serializers/deserializers that don't have access to external context.
//!
//! # Why `task_local!` instead of `thread_local!`?
//!
//! Async tasks can move between threads during execution. Using `task_local!` ensures
//! the context follows the task, not the thread, making it correct for async Rust.
//!
//! # Usage
//!
//! ```ignore
//! // In repository operations, wrap serialization with AAD context:
//! let key = self.signer_key(&signer.id);
//! let serialized = EncryptionContext::with_aad(key.clone(), || async {
//!     serde_json::to_string(&signer)
//! }).await?;
//! ```

tokio::task_local! {
    static AAD_CONTEXT: String;
}

/// Provides task-local context for AAD during encryption/decryption operations.
///
/// This allows passing the Redis key (or other storage identifier) as AAD
/// to encryption operations without modifying function signatures throughout
/// the codebase.
pub struct EncryptionContext;

impl EncryptionContext {
    /// Runs a future with AAD context set.
    ///
    /// The AAD context will be available to any code running within the future
    /// via `EncryptionContext::get()`.
    ///
    /// # Arguments
    ///
    /// * `aad` - The AAD string (typically the Redis key) to use for encryption
    /// * `f` - A closure that returns a Future to execute with the AAD context
    ///
    /// # Example
    ///
    /// ```ignore
    /// let key = "oz-relayer:signer:my-signer-id";
    /// let result = EncryptionContext::with_aad(key.to_string(), || async {
    ///     // Within this block, EncryptionContext::get() returns Some(key)
    ///     serialize_signer(&signer)
    /// }).await;
    /// ```
    pub async fn with_aad<F, Fut, R>(aad: String, f: F) -> R
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = R>,
    {
        AAD_CONTEXT.scope(aad, f()).await
    }

    /// Runs a synchronous closure with AAD context set.
    ///
    /// This is useful when the encryption/decryption operation is synchronous
    /// (like serde serialization) but needs access to the AAD context.
    /// Unlike `with_aad`, this doesn't require the closure to return a Future,
    /// avoiding Send/Sync issues with captured references.
    ///
    /// # Arguments
    ///
    /// * `aad` - The AAD string (typically the Redis key) to use for encryption
    /// * `f` - A synchronous closure to execute with the AAD context
    ///
    /// # Example
    ///
    /// ```ignore
    /// let key = "oz-relayer:signer:my-signer-id";
    /// let result = EncryptionContext::with_aad_sync(key.to_string(), || {
    ///     // Within this block, EncryptionContext::get() returns Some(key)
    ///     serde_json::to_string(&signer)
    /// })?;
    /// ```
    pub fn with_aad_sync<F, R>(aad: String, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        AAD_CONTEXT.sync_scope(aad, f)
    }

    /// Gets the current AAD context, if set.
    ///
    /// Returns `Some(aad)` if called within a `with_aad` scope,
    /// otherwise returns `None`.
    ///
    /// This is safe to call from synchronous code (like serde serializers)
    /// as long as it's running within an async task that has set the context.
    pub fn get() -> Option<String> {
        AAD_CONTEXT.try_with(|s| s.clone()).ok()
    }

    /// Checks if AAD context is currently set.
    pub fn is_set() -> bool {
        AAD_CONTEXT.try_with(|_| ()).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_with_aad_provides_context() {
        let aad = "oz-relayer:signer:test-id".to_string();

        let result =
            EncryptionContext::with_aad(aad.clone(), || async { EncryptionContext::get() }).await;

        assert_eq!(result, Some(aad));
    }

    #[tokio::test]
    async fn test_get_returns_none_outside_scope() {
        // Outside of with_aad scope, get() should return None
        let result = EncryptionContext::get();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_is_set_returns_correct_value() {
        // Outside scope
        assert!(!EncryptionContext::is_set());

        // Inside scope
        EncryptionContext::with_aad("test".to_string(), || async {
            assert!(EncryptionContext::is_set());
        })
        .await;

        // Back outside
        assert!(!EncryptionContext::is_set());
    }

    #[tokio::test]
    async fn test_nested_contexts() {
        let outer_aad = "outer-key".to_string();
        let inner_aad = "inner-key".to_string();

        EncryptionContext::with_aad(outer_aad.clone(), || async {
            // Outer context
            assert_eq!(EncryptionContext::get(), Some(outer_aad.clone()));

            // Nested context shadows outer
            EncryptionContext::with_aad(inner_aad.clone(), || async {
                assert_eq!(EncryptionContext::get(), Some(inner_aad.clone()));
            })
            .await;

            // Back to outer context
            assert_eq!(EncryptionContext::get(), Some(outer_aad));
        })
        .await;
    }

    #[tokio::test]
    async fn test_context_works_with_sync_code() {
        let aad = "sync-test-key".to_string();

        EncryptionContext::with_aad(aad.clone(), || async {
            // Simulate a sync function call within async context
            fn sync_function() -> Option<String> {
                EncryptionContext::get()
            }

            let result = sync_function();
            assert_eq!(result, Some(aad));
        })
        .await;
    }

    #[tokio::test]
    async fn test_with_aad_sync_provides_context() {
        let aad = "oz-relayer:signer:sync-test-id".to_string();

        // with_aad_sync should set context for synchronous code
        let result = EncryptionContext::with_aad_sync(aad.clone(), || EncryptionContext::get());

        assert_eq!(result, Some(aad));
    }

    #[tokio::test]
    async fn test_with_aad_sync_nested_contexts() {
        let outer_aad = "outer-sync-key".to_string();
        let inner_aad = "inner-sync-key".to_string();

        let result = EncryptionContext::with_aad_sync(outer_aad.clone(), || {
            // Outer context
            let outer_result = EncryptionContext::get();
            assert_eq!(outer_result, Some(outer_aad.clone()));

            // Nested context shadows outer
            let inner_result =
                EncryptionContext::with_aad_sync(inner_aad.clone(), || EncryptionContext::get());
            assert_eq!(inner_result, Some(inner_aad));

            // Back to outer context
            EncryptionContext::get()
        });

        assert_eq!(result, Some(outer_aad));
    }

    #[tokio::test]
    async fn test_with_aad_sync_returns_closure_result() {
        let aad = "test-key".to_string();

        // Test that with_aad_sync correctly returns the closure's result
        let result: Result<i32, &str> = EncryptionContext::with_aad_sync(aad, || Ok(42));

        assert_eq!(result, Ok(42));
    }
}
