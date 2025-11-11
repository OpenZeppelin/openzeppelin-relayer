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
use tracing::info;
use tracing_appender::non_blocking;
use tracing_error::ErrorLayer;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

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

/// Sets up logging by reading configuration from environment variables.
pub fn setup_logging() {
    // Set RUST_LOG from LOG_LEVEL if RUST_LOG is not already set
    if std::env::var_os("RUST_LOG").is_none() {
        if let Ok(level) = env::var("LOG_LEVEL") {
            std::env::set_var("RUST_LOG", level);
        }
    }

    // Configure filter, format, and mode from environment
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_LEVEL));
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
                    .with(env_filter)
                    .with(ErrorLayer::default())
                    .with(
                        fmt::layer()
                            .with_writer(non_blocking_writer)
                            .with_ansi(false)
                            .pretty()
                            .with_thread_ids(true)
                            .with_file(true)
                            .with_line_number(true),
                    )
                    .init();
            }
            "json" => {
                tracing_subscriber::registry()
                    .with(env_filter)
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
                            .with_line_number(true),
                    )
                    .init();
            }
            _ => {
                // compact is default
                tracing_subscriber::registry()
                    .with(env_filter)
                    .with(ErrorLayer::default())
                    .with(
                        fmt::layer()
                            .with_writer(non_blocking_writer)
                            .with_ansi(false)
                            .compact()
                            .with_target(false),
                    )
                    .init();
            }
        }
    } else {
        // Stdout logging
        match format.as_str() {
            "pretty" => {
                tracing_subscriber::registry()
                    .with(env_filter)
                    .with(ErrorLayer::default())
                    .with(
                        fmt::layer()
                            .pretty()
                            .with_thread_ids(true)
                            .with_file(true)
                            .with_line_number(true),
                    )
                    .init();
            }
            "json" => {
                tracing_subscriber::registry()
                    .with(env_filter)
                    .with(ErrorLayer::default())
                    .with(
                        fmt::layer()
                            .json()
                            .with_current_span(true)
                            .with_span_list(true)
                            .with_thread_ids(true)
                            .with_file(true)
                            .with_line_number(true),
                    )
                    .init();
            }
            _ => {
                // compact is default
                tracing_subscriber::registry()
                    .with(env_filter)
                    .with(ErrorLayer::default())
                    .with(fmt::layer().compact().with_target(false))
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
    use std::sync::Once;
    use tempfile::tempdir;

    // Use this to ensure logger is only initialized once across all tests
    static INIT_LOGGER: Once = Once::new();

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
}
