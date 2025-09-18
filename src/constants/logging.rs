//! Logging configuration constants

/// Default maximum log file size in bytes (1GB)
pub const DEFAULT_MAX_LOG_FILE_SIZE: u64 = 1_073_741_824;

/// Default log level when not specified
pub const DEFAULT_LOG_LEVEL: &str = "info";

/// Default log format when not specified
pub const DEFAULT_LOG_FORMAT: &str = "compact";

/// Default log mode when not specified
pub const DEFAULT_LOG_MODE: &str = "stdout";

/// Default log directory for file logging
pub const DEFAULT_LOG_DIR: &str = "./logs";

/// Default log directory when running in Docker
pub const DOCKER_LOG_DIR: &str = "logs/";

/// Log file name
pub const LOG_FILE_NAME: &str = "relayer.log";
