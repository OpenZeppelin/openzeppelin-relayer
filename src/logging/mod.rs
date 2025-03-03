//! ## Sets up logging by reading configuration from environment variables.
//!
//! Environment variables used:
//! - LOG_MODE: "stdout" (default) or "file"
//! - LOG_LEVEL: log level ("trace", "debug", "info", "warn", "error"); default is "info"
//! - LOG_FILE_PATH: when using file mode, the path of the log file (default "logs/relayer.log")

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

        // Read LOG_FILE_PATH from environment or use default
        let log_dir = env::var("LOG_FILE_PATH").unwrap_or_else(|_| "logs/".to_string());
        // Ensure the directory ends with a '/'.
        let log_dir = if log_dir.ends_with('/') {
            log_dir
        } else {
            format!("{}/", log_dir)
        };
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
