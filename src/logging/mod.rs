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
    fs::{create_dir_all, File},
    path::Path,
};

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

    if log_mode.to_lowercase() == "file" {
        info!("Logging to file: {}", log_level);
        // Get log file path from environment, or default if not provided
        let base_file_path =
            env::var("LOG_FILE_PATH").unwrap_or_else(|_| "logs/relayer.log".to_string());

        // Compute the current UTC date string
        let now = Utc::now();
        let date_str = now.format("%Y-%m-%d").to_string();
        // Build the rolled file path by appending the UTC date
        // If the base_file_path ends with ".log", remove and replace with rolled date
        let rolled_file_path = if base_file_path.ends_with(".log") {
            let trimmed = &base_file_path[..base_file_path.len() - 4];
            format!("{}-{}.log", trimmed, date_str)
        } else {
            format!("{}-{}.log", base_file_path, date_str)
        };

        // Ensure parent directory exists
        if let Some(parent) = Path::new(&rolled_file_path).parent() {
            create_dir_all(parent).expect("Failed to create log directory");
        }

        // Create the log file
        let log_file = File::create(&rolled_file_path)
            .unwrap_or_else(|e| panic!("Unable to create log file {}: {}", rolled_file_path, e));

        WriteLogger::init(level_filter, Config::default(), log_file)
            .expect("Failed to initialize file logger");
    } else {
        // Default to stdout logging
        SimpleLogger::init(level_filter, Config::default())
            .expect("Failed to initialize simple logger");
    }

    // Log that setup is complete (this requires the logger to be initialized)
    info!("Logging is successfully configured (mode: {})", log_mode);
}
