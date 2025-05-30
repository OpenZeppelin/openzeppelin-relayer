//! Sample integration test for file logging.
//!
//! Environment variables used:
//! - LOG_MODE: "stdout" (default) or "file"
//! - LOG_LEVEL: log level ("trace", "debug", "info", "warn", "error"); default is "info"
//! - LOG_DATA_DIR: when using file mode, the path of the log file (default "logs/relayer.log")
//!   Refer to `src/logging/mod.rs` for more details.
use chrono::Utc;
use openzeppelin_relayer::logging::{setup_logging, space_based_rolling, time_based_rolling};
use std::{
    env, fs,
    fs::{create_dir_all, remove_dir_all},
    io::Write,
    path::Path,
    sync::Mutex,
    thread,
    time::Duration,
};
use tempfile::TempDir;

use lazy_static::lazy_static;

static ENV_MUTEX: Mutex<()> = Mutex::new(());

// Global lazy_static that initializes logging only once.
lazy_static! {
    // This will call setup_logging() the first time INIT_LOGGING is dereferenced.
    static ref INIT_LOGGING: () = {
        setup_logging();
    };
}

pub fn compute_final_log_path(base_file_path: &str, date_str: &str, max_size: u64) -> String {
    let time_based_path = time_based_rolling(base_file_path, date_str, 1);
    space_based_rolling(&time_based_path, base_file_path, date_str, max_size)
}

// This test checks if the LOG_MAX_SIZE environment variable is set to a valid u64 value.
#[test]
#[should_panic(expected = "LOG_MAX_SIZE must be a valid u64 if set")]
fn test_invalid_log_max_size() {
    let _guard = ENV_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // Create a unique temporary directory.
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let temp_log_dir = temp_dir.path().to_str().unwrap();

    // Set LOG_MAX_SIZE to an invalid value.
    env::set_var("LOG_MODE", "file");
    env::set_var("LOG_LEVEL", "debug");
    env::set_var("LOG_DATA_DIR", format!("{}/", temp_log_dir));
    env::set_var("LOG_MAX_SIZE", "invalid_value");

    // Initialize separate from lazy static.
    setup_logging();
}

// This integration test simulates file logging
// Setting to file mode.
#[test]
fn test_setup_logging_file_mode_creates_log_file() {
    let _guard = ENV_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // Create a unique temporary directory.
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let temp_log_dir = temp_dir.path().to_str().unwrap();

    // Unset env var to ensure default values are used and not to interfere with the test.
    env::remove_var("LOG_MAX_SIZE");
    env::set_var("LOG_MODE", "file");
    env::set_var("LOG_LEVEL", "debug");
    env::set_var("LOG_DATA_DIR", format!("{}/", temp_log_dir));

    // Clean up any previous logs and create the log directory.
    let _ = remove_dir_all(temp_log_dir);
    create_dir_all(temp_log_dir).expect("Failed to create log directory");

    // Force the lazy_static to initialize logging.
    *INIT_LOGGING;

    // Sleep for the logger to flush.
    thread::sleep(Duration::from_millis(200));

    // Compute expected file path using UTC date.
    let now = Utc::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let expected_path: String = {
        let base = format!("{}/relayer.log", temp_log_dir);
        time_based_rolling(&base, &date_str, 1)
    };

    assert!(
        Path::new(&expected_path).exists(),
        "Expected log file {} does not exist",
        expected_path
    );
}

/// This integration test simulates when the relayer.log file already exists and verify if the new
/// computed file is written The test creates a log file and then checks if the log file rolls over
/// when the file size exceeds the max size.
#[test]
fn test_log_file_rolls_when_existing() {
    let _guard = ENV_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    thread::sleep(Duration::from_millis(1000));
    // Create a temporary directory for logs.
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let temp_log_dir = temp_dir.path();

    // Setup environment variables for file logging.
    env::set_var("LOG_MODE", "file");
    env::set_var("LOG_LEVEL", "debug");
    env::set_var("LOG_DATA_DIR", temp_log_dir.to_str().unwrap());

    // Clean up any previous logs in the temporary directory.
    let _ = fs::remove_dir_all(temp_log_dir);
    fs::create_dir_all(temp_log_dir).expect("Failed to create log directory");

    let base_file = temp_log_dir.join("relayer.log");

    fs::write(&base_file, "Existing log file").expect("Failed to create pre-existing log file");

    // Wait a moment to ensure the file system state is updated.
    thread::sleep(Duration::from_millis(200));

    // Get current date string.
    let now = Utc::now();
    let date_str = now.format("%Y-%m-%d").to_string();

    let max_size = 10; // bytes
    let rolled_path = compute_final_log_path(base_file.to_str().unwrap(), &date_str, max_size);

    // The rolled path should differ from the base file path and contain "relayer-".
    assert_ne!(
        rolled_path,
        base_file.to_str().unwrap(),
        "Expected rolled log file path to differ from base file path"
    );
    assert!(
        rolled_path.contains("relayer-"),
        "Expected rolled log file path to contain 'relayer-'"
    );
}

#[test]
fn test_space_based_rolling_returns_original_when_under_max_size() {
    // Create a temporary directory.
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let temp_log_dir = temp_dir.path().to_str().unwrap();
    // Define the base file path.
    let base_file_path = format!("{}/test_relayer.log", temp_log_dir);
    let now = Utc::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let time_based_path = time_based_rolling(&base_file_path, &date_str, 1);

    // Clean up any previous logs and create the log directory.
    let _ = remove_dir_all(temp_log_dir);
    create_dir_all(temp_log_dir).expect("Failed to create log directory");

    // Ensure the file does not exist.
    let _ = fs::remove_file(&time_based_path);

    // Call space_based_rolling with a high max_size so that even if file exists, it won't trigger
    // rolling.
    let max_size: u64 = 10_000;
    // Create a file with content under max_size.
    let mut file = fs::File::create(&time_based_path).expect("Failed to create test log file");
    write!(file, "small file").expect("Failed to write to test log file");

    let rolled_file_path =
        space_based_rolling(&time_based_path, &base_file_path, &date_str, max_size);
    // Since the file size is under max_size, it should return the original file path.
    assert_eq!(
        rolled_file_path, time_based_path,
        "space_based_rolling should return the original file path when within size threshold"
    );
}
