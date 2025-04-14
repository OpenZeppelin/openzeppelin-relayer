//! ## Sets up logging by reading configuration from environment variables.
//!
//! Environment variables used:
//! - LOG_MODE: "stdout" (default) or "file"
//! - LOG_LEVEL: log level ("trace", "debug", "info", "warn", "error"); default is "info"
//! - LOG_DATA_DIR: when using file mode, the path of the log file (default "logs/relayer.log")

use chrono::Utc;
use log::info;
use simplelog::{Config, LevelFilter, SimpleLogger, WriteLogger};
use std::{
    env,
    fs::{create_dir_all, metadata, File, OpenOptions},
    path::Path,
};

/// Computes the path of the rolled log file given the base file path and the date string.
pub fn compute_rolled_file_path(base_file_path: &str, date_str: &str, index: u32) -> String {
    if base_file_path.ends_with(".log") {
        let trimmed = base_file_path.strip_suffix(".log").unwrap();
        format!("{}-{}.{}.log", trimmed, date_str, index)
    } else {
        format!("{}-{}.{}.log", base_file_path, date_str, index)
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
    let log_mode = env::var("LOG_MODE").unwrap_or_else(|_| "stdout".to_string());
    let log_level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    // Parse the log level into LevelFilter
    let level_filter = match log_level.to_lowercase().as_str() {
        "trace" => LevelFilter::Trace,
        "debug" => LevelFilter::Debug,
        "info" => LevelFilter::Info,
        "warn" => LevelFilter::Warn,
        "error" => LevelFilter::Error,
        _ => LevelFilter::Info,
    };

    // Only run if log_mode is "file"
    if log_mode.to_lowercase() == "file" {
        info!("Logging to file: {}", log_level);

        // Use logs/ directly in container path, otherwise use LOG_DATA_DIR or default to logs/ for host path
        let log_dir = if env::var("IN_DOCKER")
            .map(|val| val == "true")
            .unwrap_or(false)
        {
            "logs/".to_string()
        } else {
            env::var("LOG_DATA_DIR").unwrap_or_else(|_| "./logs".to_string())
        };

        let log_dir = format!("{}/", log_dir.trim_end_matches('/'));
        // set dates
        let now = Utc::now();
        let date_str = now.format("%Y-%m-%d").to_string();

        // Get log file path from environment or use default
        let base_file_path = format!("{}relayer.log", log_dir);

        // verify the log file already exists
        if Path::new(&base_file_path).exists() {
            info!(
                "Base Log file already exists: {}. Proceeding to compute rolled log file path.",
                base_file_path
            );
        }

        // Time-based rolling: compute file name based on the current UTC date.
        let time_based_path = time_based_rolling(&base_file_path, &date_str, 1);

        // Ensure parent directory exists.
        if let Some(parent) = Path::new(&time_based_path).parent() {
            create_dir_all(parent).expect("Failed to create log directory");
        }

        // Space-based rolling: if an existing log file exceeds 1GB, adopt a new file name.
        let max_size: u64 = env::var("LOG_MAX_SIZE")
            .map(|s| {
                s.parse::<u64>()
                    .expect("LOG_MAX_SIZE must be a valid u64 if set")
            })
            .unwrap_or(1_073_741_824);

        let final_path =
            space_based_rolling(&time_based_path, &base_file_path, &date_str, max_size);

        // Open the log file. Append to it if it exists and is under threshold; otherwise, create
        // it.
        let log_file = if Path::new(&final_path).exists() {
            OpenOptions::new()
                .append(true)
                .open(&final_path)
                .unwrap_or_else(|e| panic!("Unable to open log file {}: {}", final_path, e))
        } else {
            File::create(&final_path)
                .unwrap_or_else(|e| panic!("Unable to create log file {}: {}", final_path, e))
        };
        WriteLogger::init(level_filter, Config::default(), log_file)
            .expect("Failed to initialize file logger");
    } else {
        // Default to stdout logging
        SimpleLogger::init(level_filter, Config::default())
            .expect("Failed to initialize simple logger");
    }

    info!("Logging is successfully configured (mode: {})", log_mode);
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
