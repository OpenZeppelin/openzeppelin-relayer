//! Sample integration test for file logging.
//!
//! Environment variables used:
//! - LOG_MODE: "stdout" (default) or "file"
//! - LOG_LEVEL: log level ("trace", "debug", "info", "warn", "error"); default is "info"
//! - LOG_FILE_PATH: when using file mode, the path of the log file (default "logs/relayer.log")
//!   Refer to `src/logging/mod.rs` for more details.
use chrono::Utc;
use openzeppelin_relayer::logging::{setup_logging, space_based_rolling, time_based_rolling};
use serial_test::serial;
use std::{
    env, fs,
    fs::{create_dir_all, remove_dir_all},
    io::Write,
    path::Path,
    thread,
    time::Duration,
};
use tempfile::TempDir;

// This integration test simulates file logging
// Setting to file mode.
#[test]
#[serial]
fn test_setup_logging_file_mode_creates_log_file() {
    // Create a unique temporary directory.
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let temp_log_dir = temp_dir.path().to_str().unwrap();

    env::set_var("LOG_MODE", "file");
    env::set_var("LOG_LEVEL", "debug");
    env::set_var("LOG_FILE_PATH", format!("{}/", temp_log_dir));

    setup_logging();
    // Sleep for logger to flush
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

/// This integration test simulates error when setting to file mode and the log file already exists.
/// The test creates a log file and then tries to create another one with the same name.
/// It expects the second creation to fail.
#[test]
#[serial]
#[should_panic(expected = "Failed to initialize file logger")]
fn test_setup_logging_file_mode_creates_log_file_fails_on_existing_file() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let temp_log_dir = temp_dir.path().to_str().unwrap();
    // Setup environment variables
    env::set_var("LOG_MODE", "file");
    env::set_var("LOG_LEVEL", "debug");
    env::set_var("LOG_FILE_PATH", format!("{}/", temp_log_dir));

    // Clean up any previous logs and create the log directory.
    let _ = remove_dir_all(temp_log_dir);
    create_dir_all(temp_log_dir).expect("Failed to create log directory");

    let base_file = format!("{}/relayer.log", temp_log_dir);
    // Pre-create the log file to simulate an existing log file.
    fs::write(&base_file, "Existing log file").expect("Failed to create pre-existing log file");

    setup_logging();
}

#[test]
#[serial]
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
