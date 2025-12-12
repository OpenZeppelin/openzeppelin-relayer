//! Test logging initialization
//!
//! Provides a simple helper to initialize tracing for integration tests.
//! Uses `std::sync::Once` to ensure initialization only happens once,
//! even if called from multiple tests.

use std::sync::Once;

static INIT: Once = Once::new();

/// Initialize tracing subscriber for integration tests.
///
/// This function can be called multiple times safely - initialization
/// will only happen once thanks to `std::sync::Once`.
///
/// Note: The `.env.integration` file is used for API keys and secrets only
/// (e.g., `RELAYER_API_KEY`, `KEYSTORE_PASSPHRASE`, `WEBHOOK_SIGNING_KEY`).
///
/// Configuration is controlled by the `RUST_LOG` environment variable.
/// Default: Shows info-level logs from all integration test modules
///
/// # Example
///
/// ```rust
/// #[tokio::test]
/// async fn my_test() {
///     init_test_logging();
///
///     info!("Test started");
///     // ... test code
/// }
/// ```
pub fn init_test_logging() {
    INIT.call_once(|| {
        // Default filter: show info-level logs from all integration test modules
        // Covers both the module path (integration::*) and the test binary name
        let default_filter = "info";

        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter)),
            )
            // Don't use with_test_writer() - it buffers output and only shows on test failure
            // We want real-time logs for integration tests
            .with_target(false)
            .with_ansi(false)
            .try_init();
    });
}
