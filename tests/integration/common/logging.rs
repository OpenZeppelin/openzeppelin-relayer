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
/// Configuration is controlled by the `RUST_LOG` environment variable.
/// Default: `integration=info`
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
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("integration=info")),
            )
            .with_test_writer()
            .with_target(false)
            .with_ansi(false)
            .try_init();
    });
}
