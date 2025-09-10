//! This module contains the function to log service information at startup.
use std::env;
use tracing::info;

/// Logs service information at startup
pub fn log_service_info() {
    let service_name = env!("CARGO_PKG_NAME");
    let service_version = env!("CARGO_PKG_VERSION");

    info!("=== OpenZeppelin Relayer Service Starting ===");
    info!(service_name = %service_name, service_version = %service_version, "🚀 service");
    info!(rust_version = %env!("CARGO_PKG_RUST_VERSION"), "🦀 rust version");

    // Log environment information
    if let Ok(profile) = env::var("CARGO_PKG_PROFILE") {
        info!(profile = %profile, "🔧 build profile");
    }

    // Log system information
    info!(platform = %env::consts::OS, "💻 platform");
    info!(architecture = %env::consts::ARCH, "💻 architecture");

    // Log current working directory
    if let Ok(cwd) = env::current_dir() {
        info!(working_directory = %cwd.display(), "📁 working directory");
    }

    // Log important environment variables if present
    if let Ok(rust_log) = env::var("RUST_LOG") {
        info!(log_level = %rust_log, "🔧 log level");
    }

    if let Ok(config_path) = env::var("CONFIG_PATH") {
        info!(config_path = %config_path, "🔧 config path");
    }

    // Log startup timestamp
    info!(
        started_at = %chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
        "🕒 started at"
    );

    // log docs url
    info!("ℹ️ Visit the Relayer documentation for more information https://docs.openzeppelin.com/relayer/");
}
