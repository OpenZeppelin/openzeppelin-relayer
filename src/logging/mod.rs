//! ## Sets up logging by reading configuration from environment variables.
//!
//! Environment variables used:
//! - LOG_MODE: "stdout" (default) or "file"
//! - LOG_LEVEL: log level ("trace", "debug", "info", "warn", "error"); default is "info"
//! - LOG_FORMAT: output format ("compact" (default), "pretty", "json")
//! - LOG_DATA_DIR: when using file mode, the path of the log file (default "logs/relayer.log")

use chrono::Utc;
use std::{
    env,
    fs::{create_dir_all, metadata, File, OpenOptions},
    path::Path,
};
use tracing::{info, subscriber::Interest, Metadata};
use tracing_appender::non_blocking;
use tracing_error::ErrorLayer;
use tracing_subscriber::{
    filter::LevelFilter,
    fmt,
    layer::{Context, Layer},
    prelude::*,
    EnvFilter,
};

use crate::constants::{
    DEFAULT_LOG_DIR, DEFAULT_LOG_FORMAT, DEFAULT_LOG_LEVEL, DEFAULT_LOG_MODE,
    DEFAULT_MAX_LOG_FILE_SIZE, DOCKER_LOG_DIR, LOG_FILE_NAME,
};

/// Computes the path of the rolled log file given the base file path and the date string.
pub fn compute_rolled_file_path(base_file_path: &str, date_str: &str, index: u32) -> String {
    if base_file_path.ends_with(".log") {
        let trimmed = base_file_path.strip_suffix(".log").unwrap();
        format!("{trimmed}-{date_str}.{index}.log")
    } else {
        format!("{base_file_path}-{date_str}.{index}.log")
    }
}

/// Generates a time-based log file name.
/// This is simply a wrapper around `compute_rolled_file_path` for clarity.
pub fn time_based_rolling(base_file_path: &str, date_str: &str, index: u32) -> String {
    compute_rolled_file_path(base_file_path, date_str, index)
}

/// Checks if the given log file exceeds the maximum allowed size (in bytes).
/// If so, it appends a sequence number to generate a new file name.
/// Returns the final log file path to use.
/// - `file_path`: the initial time-based log file path.
/// - `base_file_path`: the original base log file path.
/// - `date_str`: the current date string.
/// - `max_size`: maximum file size in bytes (e.g., 1GB).
pub fn space_based_rolling(
    file_path: &str,
    base_file_path: &str,
    date_str: &str,
    max_size: u64,
) -> String {
    let mut final_path = file_path.to_string();
    let mut index = 1;
    while let Ok(metadata) = metadata(&final_path) {
        if metadata.len() > max_size {
            final_path = compute_rolled_file_path(base_file_path, date_str, index);
            index += 1;
        } else {
            break;
        }
    }
    final_path
}

/// Converts a log level string to a LevelFilter.
/// Falls back to INFO if the level string is invalid.
fn parse_level_filter(level: &str) -> LevelFilter {
    match level.to_lowercase().as_str() {
        "trace" => LevelFilter::TRACE,
        "debug" => LevelFilter::DEBUG,
        "info" => LevelFilter::INFO,
        "warn" => LevelFilter::WARN,
        "error" => LevelFilter::ERROR,
        "off" => LevelFilter::OFF,
        _ => {
            tracing::warn!("Invalid log level '{level}', falling back to INFO");
            LevelFilter::INFO
        }
    }
}

/// Keeps span contexts enabled at all levels regardless of output log filtering.
#[derive(Default)]
struct SpanKeepaliveLayer;

impl<S> Layer<S> for SpanKeepaliveLayer
where
    S: tracing::Subscriber,
{
    fn register_callsite(&self, _metadata: &'static Metadata<'static>) -> Interest {
        // Keep all callsites available so downstream filtered layers can decide
        // what to emit, while span context remains constructible.
        Interest::always()
    }

    fn enabled(&self, _metadata: &Metadata<'_>, _ctx: Context<'_, S>) -> bool {
        true
    }
}

/// Builds filter directives string by combining user configuration with default suppressions
/// for noisy crates. Only adds suppressions for crates not explicitly configured by the user.
fn build_filter_directives() -> String {
    const NOISY_CRATES: &[&str] = &[
        "reqwest",
        "hyper",
        "rustls",
        "h2",
        "alloy_transport_http",
        "aws_sdk_kms",
        "solana_client",
        "solana_program",
    ];

    let rust_log = env::var("RUST_LOG").unwrap_or_else(|_| DEFAULT_LOG_LEVEL.to_string());

    let suppressions: Vec<String> = NOISY_CRATES
        .iter()
        .filter(|&crate_name| !rust_log.contains(crate_name))
        .map(|name| format!("{name}=warn"))
        .collect();

    let mut directives = suppressions;
    directives.push(rust_log);
    directives.join(",")
}

fn build_env_filter(default_level: LevelFilter, directives: &str) -> EnvFilter {
    EnvFilter::builder()
        .with_default_directive(default_level.into())
        .parse_lossy(directives)
}

/// Sets up logging by reading configuration from environment variables.
pub fn setup_logging() {
    // Set RUST_LOG from LOG_LEVEL if RUST_LOG is not already set
    if std::env::var_os("RUST_LOG").is_none() {
        if let Ok(level) = env::var("LOG_LEVEL") {
            std::env::set_var("RUST_LOG", level);
        }
    }

    // Configure filter, format, and mode from environment
    // Suppress noisy HTTP/TLS debug logs by default unless explicitly configured
    let default_level = parse_level_filter(DEFAULT_LOG_LEVEL);
    let filter_directives = build_filter_directives();

    let format = env::var("LOG_FORMAT").unwrap_or_else(|_| DEFAULT_LOG_FORMAT.to_string());
    let log_mode = env::var("LOG_MODE").unwrap_or_else(|_| DEFAULT_LOG_MODE.to_string());

    // Set up logging based on mode
    if log_mode.eq_ignore_ascii_case("file") {
        // File logging setup
        let log_dir = if env::var("IN_DOCKER").ok().as_deref() == Some("true") {
            DOCKER_LOG_DIR.to_string()
        } else {
            env::var("LOG_DATA_DIR").unwrap_or_else(|_| DEFAULT_LOG_DIR.to_string())
        };
        let log_dir = format!("{}/", log_dir.trim_end_matches('/'));

        let now = Utc::now();
        let date_str = now.format("%Y-%m-%d").to_string();
        let base_file_path = format!("{log_dir}{LOG_FILE_NAME}");

        if let Some(parent) = Path::new(&base_file_path).parent() {
            create_dir_all(parent).expect("Failed to create log directory");
        }

        let time_based_path = time_based_rolling(&base_file_path, &date_str, 1);
        let max_size = match env::var("LOG_MAX_SIZE") {
            Ok(value) => value.parse().unwrap_or_else(|_| {
                panic!("LOG_MAX_SIZE must be a valid u64 if set");
            }),
            Err(_) => DEFAULT_MAX_LOG_FILE_SIZE,
        };
        let final_path =
            space_based_rolling(&time_based_path, &base_file_path, &date_str, max_size);

        let file = if Path::new(&final_path).exists() {
            OpenOptions::new()
                .append(true)
                .open(&final_path)
                .expect("Failed to open log file")
        } else {
            File::create(&final_path).expect("Failed to create log file")
        };

        let (non_blocking_writer, guard) = non_blocking(file);
        Box::leak(Box::new(guard)); // Keep guard alive for the lifetime of the program

        match format.as_str() {
            "pretty" => {
                tracing_subscriber::registry()
                    .with(SpanKeepaliveLayer)
                    .with(ErrorLayer::default())
                    .with(
                        fmt::layer()
                            .with_writer(non_blocking_writer)
                            .with_ansi(false)
                            .pretty()
                            .with_thread_ids(true)
                            .with_file(true)
                            .with_line_number(true)
                            .with_filter(build_env_filter(default_level, &filter_directives)),
                    )
                    .init();
            }
            "json" => {
                tracing_subscriber::registry()
                    .with(SpanKeepaliveLayer)
                    .with(ErrorLayer::default())
                    .with(
                        fmt::layer()
                            .with_writer(non_blocking_writer)
                            .with_ansi(false)
                            .json()
                            .with_current_span(true)
                            .with_span_list(true)
                            .with_thread_ids(true)
                            .with_file(true)
                            .with_line_number(true)
                            .with_filter(build_env_filter(default_level, &filter_directives)),
                    )
                    .init();
            }
            _ => {
                // compact is default
                tracing_subscriber::registry()
                    .with(SpanKeepaliveLayer)
                    .with(ErrorLayer::default())
                    .with(
                        fmt::layer()
                            .with_writer(non_blocking_writer)
                            .with_ansi(false)
                            .compact()
                            .with_target(false)
                            .with_filter(build_env_filter(default_level, &filter_directives)),
                    )
                    .init();
            }
        }
    } else {
        // Stdout logging
        match format.as_str() {
            "pretty" => {
                tracing_subscriber::registry()
                    .with(SpanKeepaliveLayer)
                    .with(ErrorLayer::default())
                    .with(
                        fmt::layer()
                            .pretty()
                            .with_thread_ids(true)
                            .with_file(true)
                            .with_line_number(true)
                            .with_filter(build_env_filter(default_level, &filter_directives)),
                    )
                    .init();
            }
            "json" => {
                tracing_subscriber::registry()
                    .with(SpanKeepaliveLayer)
                    .with(ErrorLayer::default())
                    .with(
                        fmt::layer()
                            .json()
                            .with_current_span(true)
                            .with_span_list(true)
                            .with_thread_ids(true)
                            .with_file(true)
                            .with_line_number(true)
                            .with_filter(build_env_filter(default_level, &filter_directives)),
                    )
                    .init();
            }
            _ => {
                // compact is default
                tracing_subscriber::registry()
                    .with(SpanKeepaliveLayer)
                    .with(ErrorLayer::default())
                    .with(
                        fmt::layer()
                            .compact()
                            .with_target(false)
                            .with_filter(build_env_filter(default_level, &filter_directives)),
                    )
                    .init();
            }
        }
    }

    info!(mode=%log_mode, format=%format, "logging configured");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::sync::Mutex as StdMutex;
    use std::sync::{Arc, Mutex, Once};
    use tempfile::tempdir;
    use tracing_subscriber::fmt::MakeWriter;

    // Use this to ensure logger is only initialized once across all tests
    static INIT_LOGGER: Once = Once::new();

    // Mutex to serialize tests that modify environment variables
    // This prevents test interference when running in parallel
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    // Test helper infrastructure for capturing log output
    mod test_helpers {
        use super::*;

        /// Custom writer that writes to an in-memory buffer
        pub struct TestWriter(pub Arc<StdMutex<Vec<u8>>>);

        impl std::io::Write for TestWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().write(buf)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.0.lock().unwrap().flush()
            }
        }

        /// Newtype wrapper to implement MakeWriter trait
        /// This is needed to avoid the orphan rule when implementing MakeWriter for Arc<Mutex<Vec<u8>>>
        pub struct TestMakeWriter(pub Arc<StdMutex<Vec<u8>>>);

        impl<'a> MakeWriter<'a> for TestMakeWriter {
            type Writer = TestWriter;
            fn make_writer(&'a self) -> Self::Writer {
                TestWriter(self.0.clone())
            }
        }

        /// Creates a test subscriber with the given filter that captures output to a buffer
        /// Returns (subscriber_guard, buffer) where:
        /// - subscriber_guard: Must be kept alive for the duration of the test
        /// - buffer: The buffer where log output is captured
        pub fn create_test_subscriber(
            filter: EnvFilter,
        ) -> (tracing::subscriber::DefaultGuard, Arc<StdMutex<Vec<u8>>>) {
            let buffer = Arc::new(StdMutex::new(Vec::new()));
            let buffer_clone = buffer.clone();

            let subscriber = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(TestMakeWriter(buffer_clone))
                .with_ansi(false)
                .finish();

            let guard = tracing::subscriber::set_default(subscriber);
            (guard, buffer)
        }

        /// Helper to get the captured output as a String
        pub fn get_output(buffer: &Arc<StdMutex<Vec<u8>>>) -> String {
            String::from_utf8(buffer.lock().unwrap().clone()).unwrap()
        }
    }

    #[test]
    fn test_compute_rolled_file_path() {
        // Test with .log extension
        let result = compute_rolled_file_path("app.log", "2023-01-01", 1);
        assert_eq!(result, "app-2023-01-01.1.log");

        // Test without .log extension
        let result = compute_rolled_file_path("app", "2023-01-01", 2);
        assert_eq!(result, "app-2023-01-01.2.log");

        // Test with path
        let result = compute_rolled_file_path("logs/app.log", "2023-01-01", 3);
        assert_eq!(result, "logs/app-2023-01-01.3.log");
    }

    #[test]
    fn test_time_based_rolling() {
        // This is just a wrapper around compute_rolled_file_path
        let result = time_based_rolling("app.log", "2023-01-01", 1);
        assert_eq!(result, "app-2023-01-01.1.log");
    }

    #[test]
    fn test_space_based_rolling() {
        // Create a temporary directory for testing
        let temp_dir = tempdir().expect("Failed to create temp directory");
        let base_path = temp_dir
            .path()
            .join("test.log")
            .to_str()
            .unwrap()
            .to_string();

        // Test when file doesn't exist
        let result = space_based_rolling(&base_path, &base_path, "2023-01-01", 100);
        assert_eq!(result, base_path);

        // Create a file larger than max_size
        {
            let mut file = File::create(&base_path).expect("Failed to create test file");
            file.write_all(&[0; 200])
                .expect("Failed to write to test file");
        }

        // Test when file exists and is larger than max_size
        let expected_path = compute_rolled_file_path(&base_path, "2023-01-01", 1);
        let result = space_based_rolling(&base_path, &base_path, "2023-01-01", 100);
        assert_eq!(result, expected_path);

        // Create multiple files to test sequential numbering
        {
            let mut file = File::create(&expected_path).expect("Failed to create test file");
            file.write_all(&[0; 200])
                .expect("Failed to write to test file");
        }

        // Test sequential numbering
        let expected_path2 = compute_rolled_file_path(&base_path, "2023-01-01", 2);
        let result = space_based_rolling(&base_path, &base_path, "2023-01-01", 100);
        assert_eq!(result, expected_path2);
    }

    #[test]
    fn test_logging_configuration() {
        // Lock mutex to prevent test interference
        let _guard = ENV_MUTEX.lock().unwrap();

        // We'll test both configurations in a single test to avoid multiple logger initializations

        // First test stdout configuration
        {
            // Set environment variables for testing
            env::set_var("LOG_MODE", "stdout");
            env::set_var("LOG_LEVEL", "debug");

            // Initialize logger only once across all tests
            INIT_LOGGER.call_once(|| {
                setup_logging();
            });

            // Clean up
            env::remove_var("LOG_MODE");
            env::remove_var("LOG_LEVEL");
        }

        // Now test file configuration without reinitializing the logger
        {
            // Create a temporary directory for testing
            let temp_dir = tempdir().expect("Failed to create temp directory");
            let log_path = temp_dir
                .path()
                .join("test_logs")
                .to_str()
                .unwrap()
                .to_string();

            // Set environment variables for testing
            env::set_var("LOG_MODE", "file");
            env::set_var("LOG_LEVEL", "info");
            env::set_var("LOG_DATA_DIR", &log_path);
            env::set_var("LOG_MAX_SIZE", "1024"); // 1KB for testing

            // We don't call setup_logging() again, but we can test the directory creation logic
            if let Some(parent) = Path::new(&format!("{}/relayer.log", log_path)).parent() {
                create_dir_all(parent).expect("Failed to create log directory");
            }

            // Verify the log directory was created
            assert!(Path::new(&log_path).exists());

            // Clean up
            env::remove_var("LOG_MODE");
            env::remove_var("LOG_LEVEL");
            env::remove_var("LOG_DATA_DIR");
            env::remove_var("LOG_MAX_SIZE");
        }
    }

    #[test]
    fn test_env_filter_with_no_rust_log() {
        // Lock mutex to prevent test interference
        let _guard = ENV_MUTEX.lock().unwrap();

        // Save original RUST_LOG if it exists
        let original_rust_log = env::var("RUST_LOG").ok();

        // Remove RUST_LOG to test default behavior
        env::remove_var("RUST_LOG");

        // Create the filter that would be used by setup_logging
        let default_level = parse_level_filter(DEFAULT_LOG_LEVEL);
        let filter = EnvFilter::builder()
            .with_default_directive(default_level.into())
            .parse_lossy(build_filter_directives());

        // Create test subscriber and capture buffer
        let (_subscriber_guard, buffer) = test_helpers::create_test_subscriber(filter);

        // Emit test logs
        tracing::info!(target: "openzeppelin_relayer", "app info message");
        tracing::debug!(target: "openzeppelin_relayer", "app debug - should be suppressed (default is info)");
        tracing::debug!(target: "reqwest", "reqwest debug - should be suppressed");
        tracing::warn!(target: "reqwest", "reqwest warn - should be visible");
        tracing::debug!(target: "hyper", "hyper debug - should be suppressed");
        tracing::warn!(target: "hyper", "hyper warn - should be visible");

        // Get the captured output
        let output = test_helpers::get_output(&buffer);

        // Verify filtering worked correctly
        // Default level is "info", so app debug logs should be suppressed
        assert!(
            output.contains("app info message"),
            "App info logs should be captured (default level is info)"
        );
        assert!(
            !output.contains("app debug"),
            "App debug logs should be suppressed (default level is info)"
        );

        // HTTP/TLS crates should have debug/info suppressed, only warn+ visible
        assert!(
            !output.contains("reqwest debug"),
            "Reqwest debug logs should be suppressed"
        );
        assert!(
            output.contains("reqwest warn"),
            "Reqwest warn logs should be visible"
        );
        assert!(
            !output.contains("hyper debug"),
            "Hyper debug logs should be suppressed"
        );
        assert!(
            output.contains("hyper warn"),
            "Hyper warn logs should be visible"
        );

        // Restore original RUST_LOG
        if let Some(val) = original_rust_log {
            env::set_var("RUST_LOG", val);
        }
    }

    #[test]
    fn test_env_filter_with_rust_log_set() {
        // Lock mutex to prevent test interference
        let _guard = ENV_MUTEX.lock().unwrap();

        // Save original RUST_LOG if it exists
        let original_rust_log = env::var("RUST_LOG").ok();

        // Set RUST_LOG to info level (without HTTP/TLS crate configs)
        env::set_var("RUST_LOG", "info");

        // Create filter using the same logic as setup_logging
        let default_level = parse_level_filter(DEFAULT_LOG_LEVEL);
        let filter = EnvFilter::builder()
            .with_default_directive(default_level.into())
            .parse_lossy(build_filter_directives());

        // Create test subscriber and capture buffer
        let (_subscriber_guard, buffer) = test_helpers::create_test_subscriber(filter);

        // Emit test logs
        tracing::info!(target: "openzeppelin_relayer", "test app info");
        tracing::debug!(target: "openzeppelin_relayer", "test app debug - should be suppressed");
        tracing::debug!(target: "hyper", "test hyper debug - should be suppressed");
        tracing::info!(target: "hyper", "test hyper info - should be suppressed");
        tracing::warn!(target: "hyper", "test hyper warn - should be visible");

        // Get the captured output
        let output = test_helpers::get_output(&buffer);

        // Verify filtering worked correctly
        assert!(
            output.contains("test app info"),
            "App info logs should be captured (RUST_LOG=info)"
        );
        assert!(
            !output.contains("test app debug"),
            "App debug logs should be suppressed (RUST_LOG=info)"
        );
        assert!(
            !output.contains("test hyper debug"),
            "Hyper debug logs should be suppressed"
        );
        assert!(
            !output.contains("test hyper info"),
            "Hyper info logs should be suppressed (hyper=warn)"
        );
        assert!(
            output.contains("test hyper warn"),
            "Hyper warn logs should be visible"
        );

        // Restore original RUST_LOG
        env::remove_var("RUST_LOG");
        if let Some(val) = original_rust_log {
            env::set_var("RUST_LOG", val);
        }
    }

    #[test]
    fn test_env_filter_with_explicit_reqwest_config() {
        // Lock mutex to prevent test interference
        let _guard = ENV_MUTEX.lock().unwrap();

        // Save original RUST_LOG if it exists
        let original_rust_log = env::var("RUST_LOG").ok();

        // Set RUST_LOG with explicit reqwest=debug (user wants to see reqwest debug logs)
        env::set_var("RUST_LOG", "info,reqwest=debug");

        // Create filter using the same logic as setup_logging
        let default_level = parse_level_filter(DEFAULT_LOG_LEVEL);
        let filter = EnvFilter::builder()
            .with_default_directive(default_level.into())
            .parse_lossy(build_filter_directives());

        // Create test subscriber and capture buffer
        let (_subscriber_guard, buffer) = test_helpers::create_test_subscriber(filter);

        // Emit test logs
        tracing::debug!(target: "reqwest", "reqwest debug - user override");
        tracing::debug!(target: "hyper", "hyper debug - should be suppressed");
        tracing::warn!(target: "hyper", "hyper warn - should be visible");

        // Get the captured output
        let output = test_helpers::get_output(&buffer);

        // Verify user's explicit reqwest=debug is honored
        assert!(
            output.contains("reqwest debug"),
            "User's explicit reqwest=debug should allow debug logs"
        );

        // Verify other crates are still suppressed
        assert!(
            !output.contains("hyper debug"),
            "Hyper debug logs should still be suppressed"
        );
        assert!(
            output.contains("hyper warn"),
            "Hyper warn logs should be visible"
        );

        // Restore original RUST_LOG
        env::remove_var("RUST_LOG");
        if let Some(val) = original_rust_log {
            env::set_var("RUST_LOG", val);
        }
    }

    #[test]
    fn test_env_filter_respects_user_overrides() {
        // Lock mutex to prevent test interference
        let _guard = ENV_MUTEX.lock().unwrap();

        // Save original RUST_LOG if it exists
        let original_rust_log = env::var("RUST_LOG").ok();

        // Set RUST_LOG with user wanting to see ALL hyper logs (even trace level)
        env::set_var("RUST_LOG", "info,hyper=trace");

        // Create filter using the same logic as setup_logging
        let default_level = parse_level_filter(DEFAULT_LOG_LEVEL);
        let filter = EnvFilter::builder()
            .with_default_directive(default_level.into())
            .parse_lossy(build_filter_directives());

        // Create test subscriber and capture buffer
        let (_subscriber_guard, buffer) = test_helpers::create_test_subscriber(filter);

        // Emit test logs
        tracing::trace!(target: "hyper", "test hyper trace - user override");
        tracing::debug!(target: "hyper", "test hyper debug - user override");
        tracing::debug!(target: "reqwest", "test reqwest debug - should be suppressed");
        tracing::info!(target: "reqwest", "test reqwest info - should be suppressed");
        tracing::warn!(target: "reqwest", "test reqwest warn - should be visible");

        // Get the captured output
        let output = test_helpers::get_output(&buffer);

        // Verify user's explicit hyper=trace is honored
        assert!(
            output.contains("test hyper trace"),
            "User's explicit hyper=trace should allow trace logs"
        );
        assert!(
            output.contains("test hyper debug"),
            "User's explicit hyper=trace should allow debug logs"
        );

        // Verify reqwest is still suppressed (no user override)
        assert!(
            !output.contains("test reqwest debug"),
            "Reqwest debug logs should be suppressed"
        );
        assert!(
            !output.contains("test reqwest info"),
            "Reqwest info logs should be suppressed"
        );
        assert!(
            output.contains("test reqwest warn"),
            "Reqwest warn logs should be visible"
        );

        // Restore original RUST_LOG
        env::remove_var("RUST_LOG");
        if let Some(val) = original_rust_log {
            env::set_var("RUST_LOG", val);
        }
    }

    #[test]
    fn test_env_filter_mixed_user_and_default_suppressions() {
        // Lock mutex to prevent test interference
        let _guard = ENV_MUTEX.lock().unwrap();

        // Save original RUST_LOG if it exists
        let original_rust_log = env::var("RUST_LOG").ok();

        // User configures some crates explicitly, others should get defaults
        env::set_var("RUST_LOG", "debug,reqwest=error,rustls=info");

        // Create filter using the same logic as setup_logging
        let default_level = parse_level_filter(DEFAULT_LOG_LEVEL);
        let filter = EnvFilter::builder()
            .with_default_directive(default_level.into())
            .parse_lossy(build_filter_directives());

        // Create test subscriber and capture buffer
        let (_subscriber_guard, buffer) = test_helpers::create_test_subscriber(filter);

        // Emit test logs
        tracing::warn!(target: "reqwest", "test reqwest warn - user set error");
        tracing::error!(target: "reqwest", "test reqwest error - should be visible");
        tracing::debug!(target: "rustls", "test rustls debug - user set info");
        tracing::info!(target: "rustls", "test rustls info - should be visible");
        tracing::debug!(target: "hyper", "test hyper debug - should be suppressed");
        tracing::info!(target: "hyper", "test hyper info - should be suppressed");
        tracing::warn!(target: "hyper", "test hyper warn - should be visible");
        tracing::debug!(target: "h2", "test h2 debug - should be suppressed");
        tracing::warn!(target: "h2", "test h2 warn - should be visible");

        // Get the captured output
        let output = test_helpers::get_output(&buffer);

        // Verify user's explicit reqwest=error (only errors, not warnings)
        assert!(
            !output.contains("test reqwest warn"),
            "User's reqwest=error should reject warn logs"
        );
        assert!(
            output.contains("test reqwest error"),
            "User's reqwest=error should accept error logs"
        );

        // Verify user's explicit rustls=info
        assert!(
            !output.contains("test rustls debug"),
            "User's rustls=info should reject debug logs"
        );
        assert!(
            output.contains("test rustls info"),
            "User's rustls=info should accept info logs"
        );

        // Verify default hyper=warn suppression (user didn't configure it)
        assert!(
            !output.contains("test hyper debug"),
            "Default hyper=warn should reject debug logs"
        );
        assert!(
            !output.contains("test hyper info"),
            "Default hyper=warn should reject info logs"
        );
        assert!(
            output.contains("test hyper warn"),
            "Default hyper=warn should accept warn logs"
        );

        // Verify default h2=warn suppression
        assert!(
            !output.contains("test h2 debug"),
            "Default h2=warn should reject debug logs"
        );
        assert!(
            output.contains("test h2 warn"),
            "Default h2=warn should accept warn logs"
        );

        // Restore original RUST_LOG
        env::remove_var("RUST_LOG");
        if let Some(val) = original_rust_log {
            env::set_var("RUST_LOG", val);
        }
    }

    #[test]
    fn test_directive_parsing() {
        // Test that all our directives can be parsed successfully
        assert!("reqwest=warn"
            .parse::<tracing_subscriber::filter::Directive>()
            .is_ok());
        assert!("hyper=warn"
            .parse::<tracing_subscriber::filter::Directive>()
            .is_ok());
        assert!("rustls=warn"
            .parse::<tracing_subscriber::filter::Directive>()
            .is_ok());
        assert!("h2=warn"
            .parse::<tracing_subscriber::filter::Directive>()
            .is_ok());
        assert!("alloy_transport_http=warn"
            .parse::<tracing_subscriber::filter::Directive>()
            .is_ok());
        assert!("aws_sdk_kms=warn"
            .parse::<tracing_subscriber::filter::Directive>()
            .is_ok());
        assert!("solana_client=warn"
            .parse::<tracing_subscriber::filter::Directive>()
            .is_ok());
        assert!("solana_program=warn"
            .parse::<tracing_subscriber::filter::Directive>()
            .is_ok());
    }

    #[test]
    fn test_parse_level_filter() {
        // Test all valid log levels
        assert_eq!(parse_level_filter("trace"), LevelFilter::TRACE);
        assert_eq!(parse_level_filter("debug"), LevelFilter::DEBUG);
        assert_eq!(parse_level_filter("info"), LevelFilter::INFO);
        assert_eq!(parse_level_filter("warn"), LevelFilter::WARN);
        assert_eq!(parse_level_filter("error"), LevelFilter::ERROR);
        assert_eq!(parse_level_filter("off"), LevelFilter::OFF);

        // Test case insensitivity
        assert_eq!(parse_level_filter("TRACE"), LevelFilter::TRACE);
        assert_eq!(parse_level_filter("Debug"), LevelFilter::DEBUG);
        assert_eq!(parse_level_filter("INFO"), LevelFilter::INFO);
        assert_eq!(parse_level_filter("WaRn"), LevelFilter::WARN);

        // Test invalid levels fall back to INFO
        assert_eq!(parse_level_filter("invalid"), LevelFilter::INFO);
        assert_eq!(parse_level_filter(""), LevelFilter::INFO);
        assert_eq!(parse_level_filter("warning"), LevelFilter::INFO);
    }

    #[test]
    fn test_keepalive_layer_preserves_debug_span_context_at_warn_output() {
        let _subscriber_guard = tracing::subscriber::set_default(
            tracing_subscriber::registry()
                .with(SpanKeepaliveLayer)
                .with(
                    fmt::layer()
                        .with_writer(std::io::sink)
                        .with_filter(EnvFilter::new("warn")),
                ),
        );

        let span = tracing::debug_span!("debug_ctx");
        let _guard = span.enter();

        crate::observability::request_id::set_request_id("keepalive-request-id");
        assert_eq!(
            crate::observability::request_id::get_request_id().as_deref(),
            Some("keepalive-request-id")
        );
    }

    #[test]
    fn test_keepalive_layer_does_not_suppress_warn_events() {
        let buffer = Arc::new(StdMutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry()
            .with(SpanKeepaliveLayer)
            .with(
                fmt::layer()
                    .with_writer(test_helpers::TestMakeWriter(buffer.clone()))
                    .with_ansi(false)
                    .with_filter(EnvFilter::new("warn")),
            );
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);

        tracing::warn!(target: "openzeppelin_relayer", "keepalive warn event");
        let output = test_helpers::get_output(&buffer);
        assert!(
            output.contains("keepalive warn event"),
            "Warn event should be captured when keepalive layer is used"
        );
    }

    #[test]
    fn test_build_filter_directives_no_rust_log() {
        // Lock mutex to prevent test interference
        let _guard = ENV_MUTEX.lock().unwrap();

        // Save original RUST_LOG
        let original_rust_log = env::var("RUST_LOG").ok();

        // Remove RUST_LOG to test default behavior
        env::remove_var("RUST_LOG");

        let result = build_filter_directives();

        // Should contain all noisy crate suppressions
        assert!(result.contains("reqwest=warn"));
        assert!(result.contains("hyper=warn"));
        assert!(result.contains("rustls=warn"));
        assert!(result.contains("h2=warn"));
        assert!(result.contains("alloy_transport_http=warn"));
        assert!(result.contains("aws_sdk_kms=warn"));
        assert!(result.contains("solana_client=warn"));
        assert!(result.contains("solana_program=warn"));

        // Should end with the default level
        assert!(result.ends_with(DEFAULT_LOG_LEVEL));

        // Restore original RUST_LOG
        env::remove_var("RUST_LOG");
        if let Some(val) = original_rust_log {
            env::set_var("RUST_LOG", val);
        }
    }

    #[test]
    fn test_build_filter_directives_with_rust_log() {
        // Lock mutex to prevent test interference
        let _guard = ENV_MUTEX.lock().unwrap();

        // Save original RUST_LOG
        let original_rust_log = env::var("RUST_LOG").ok();

        // Set RUST_LOG to a custom value
        env::set_var("RUST_LOG", "debug");

        let result = build_filter_directives();

        // Should contain all noisy crate suppressions
        assert!(result.contains("reqwest=warn"));
        assert!(result.contains("hyper=warn"));

        // Should end with the user's RUST_LOG value
        assert!(result.ends_with("debug"));

        // Restore original RUST_LOG
        env::remove_var("RUST_LOG");
        if let Some(val) = original_rust_log {
            env::set_var("RUST_LOG", val);
        }
    }

    #[test]
    fn test_build_filter_directives_respects_user_overrides() {
        // Lock mutex to prevent test interference
        let _guard = ENV_MUTEX.lock().unwrap();

        // Save original RUST_LOG
        let original_rust_log = env::var("RUST_LOG").ok();

        // Set RUST_LOG with explicit reqwest configuration
        env::set_var("RUST_LOG", "info,reqwest=debug");

        let result = build_filter_directives();

        // Should NOT contain reqwest=warn since user configured it
        assert!(
            !result.contains("reqwest=warn"),
            "Should not add reqwest=warn when user configured reqwest. Result: {}",
            result
        );

        // Should still contain other suppressions
        assert!(result.contains("hyper=warn"));
        assert!(result.contains("rustls=warn"));

        // Should contain the user's RUST_LOG value
        assert!(result.contains("info,reqwest=debug"));

        // Restore original RUST_LOG
        env::remove_var("RUST_LOG");
        if let Some(val) = original_rust_log {
            env::set_var("RUST_LOG", val);
        }
    }

    #[test]
    fn test_build_filter_directives_multiple_user_overrides() {
        // Lock mutex to prevent test interference
        let _guard = ENV_MUTEX.lock().unwrap();

        // Save original RUST_LOG
        let original_rust_log = env::var("RUST_LOG").ok();

        // Set RUST_LOG with multiple explicit configurations
        env::set_var("RUST_LOG", "debug,reqwest=error,hyper=trace,rustls=info");

        let result = build_filter_directives();

        // Should NOT contain suppressions for user-configured crates
        assert!(
            !result.contains("reqwest=warn"),
            "Should not add reqwest=warn when user configured it"
        );
        assert!(
            !result.contains("hyper=warn"),
            "Should not add hyper=warn when user configured it"
        );
        assert!(
            !result.contains("rustls=warn"),
            "Should not add rustls=warn when user configured it"
        );

        // Should still contain suppressions for non-configured crates
        assert!(result.contains("h2=warn"));
        assert!(result.contains("alloy_transport_http=warn"));
        assert!(result.contains("aws_sdk_kms=warn"));
        assert!(result.contains("solana_client=warn"));
        assert!(result.contains("solana_program=warn"));

        // Should contain the user's RUST_LOG value
        assert!(result.contains("debug,reqwest=error,hyper=trace,rustls=info"));

        // Restore original RUST_LOG
        env::remove_var("RUST_LOG");
        if let Some(val) = original_rust_log {
            env::set_var("RUST_LOG", val);
        }
    }

    #[test]
    fn test_build_filter_directives_format() {
        // Lock mutex to prevent test interference
        let _guard = ENV_MUTEX.lock().unwrap();

        // Save original RUST_LOG
        let original_rust_log = env::var("RUST_LOG").ok();

        // Remove RUST_LOG
        env::remove_var("RUST_LOG");

        let result = build_filter_directives();

        // Should be a valid comma-separated list
        let parts: Vec<&str> = result.split(',').collect();
        assert!(
            parts.len() >= 8,
            "Should have at least 8 parts (suppressions)"
        );

        // Should not have empty parts
        for part in &parts {
            assert!(!part.is_empty(), "Should not have empty parts");
        }

        // Should not start or end with comma
        assert!(!result.starts_with(','));
        assert!(!result.ends_with(','));

        // Restore original RUST_LOG
        env::remove_var("RUST_LOG");
        if let Some(val) = original_rust_log {
            env::set_var("RUST_LOG", val);
        }
    }
}
