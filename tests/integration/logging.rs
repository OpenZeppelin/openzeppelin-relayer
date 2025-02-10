//! Sample integration test for file logging.
//!
//! Environment variables used:
//! - LOG_MODE: "stdout" (default) or "file"
//! - LOG_LEVEL: log level ("trace", "debug", "info", "warn", "error"); default is "info"
//! - LOG_FILE_PATH: when using file mode, the path of the log file (default "logs/relayer.log")
//!   Refer to `src/logging/mod.rs` for more details.
use chrono::Utc;
use openzeppelin_relayer::logging::{compute_rolled_file_path, setup_logging};
use serial_test::serial;
use std::{
    env, fs,
    fs::{create_dir_all, remove_dir_all},
    path::Path,
    thread,
    time::Duration,
};

// This integration test simulates file logging
// Setting to file mode.
#[test]
#[serial]
fn test_setup_logging_file_mode_creates_log_file() {
    env::set_var("LOG_MODE", "file");
    env::set_var("LOG_LEVEL", "debug");

    let temp_log_dir = "/tmp/int_logs_1";
    env::set_var(
        "LOG_FILE_PATH",
        format!("{}/test_relayer.log", temp_log_dir),
    );
    let _ = fs::remove_dir_all(temp_log_dir);
    create_dir_all(temp_log_dir).unwrap();

    setup_logging();
    // Sleep for logger to flush
    thread::sleep(Duration::from_millis(200));

    // Compute expected file path using UTC date.
    let now = Utc::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let expected_path: String = {
        let base = format!("{}/test_relayer.log", temp_log_dir);
        compute_rolled_file_path(&base, &date_str)
    };

    assert!(
        Path::new(&expected_path).exists(),
        "Expected log file {} does not exist",
        expected_path
    );

    let _ = remove_dir_all(temp_log_dir);
}

/// This integration test simulates error when setting to file mode and the log file already exists.
/// The test creates a log file and then tries to create another one with the same name.
/// It expects the second creation to fail.
#[test]
#[serial]
#[should_panic(expected = "Failed to initialize file logger")]
fn test_setup_logging_file_mode_creates_log_file_fails_on_existing_file() {
    // Setup environment variables
    env::set_var("LOG_MODE", "file");
    env::set_var("LOG_LEVEL", "debug");
    let temp_log_dir = "/tmp/int_log_2";
    let log_file_path = format!("{}/test_relayer.log", temp_log_dir);
    env::set_var("LOG_FILE_PATH", &log_file_path);

    // Clean up any previous logs and create the log directory.
    let _ = fs::remove_dir_all(temp_log_dir);
    create_dir_all(temp_log_dir).expect("Failed to create log directory");

    // Pre-create the log file to simulate an existing log file.
    fs::write(&log_file_path, "Existing log file").expect("Failed to create pre-existing log file");

    setup_logging();

    let _ = fs::remove_dir_all(temp_log_dir);
}
