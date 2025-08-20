//! This module contains the function to log service information at startup.
use log::info;
use std::env;

/// Logs service information at startup
pub fn log_service_info() {
    let service_name = env!("CARGO_PKG_NAME");
    let service_version = env!("CARGO_PKG_VERSION");

    info!("=== OpenZeppelin Relayer Service Starting ===");
    info!("🚀 Service: {} v{}", service_name, service_version);
    info!("🦀 Rust Version: {}", env!("CARGO_PKG_RUST_VERSION"));

    // Log environment information
    if let Ok(profile) = env::var("CARGO_PKG_PROFILE") {
        info!("🔧 Build Profile: {}", profile);
    }

    // Log system information
    info!("💻 Platform: {}", env::consts::OS);
    info!("💻 Architecture: {}", env::consts::ARCH);

    // Log current working directory
    if let Ok(cwd) = env::current_dir() {
        info!("📁 Working Directory: {}", cwd.display());
    }

    // Log important environment variables if present
    if let Ok(rust_log) = env::var("RUST_LOG") {
        info!("🔧 Log Level: {}", rust_log);
    }

    if let Ok(config_path) = env::var("CONFIG_PATH") {
        info!("🔧 Config Path: {}", config_path);
    }

    // Log startup timestamp
    info!(
        "🕒 Started at: {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );

    // log docs url
    info!("ℹ️ Visit the Relayer documentation for more information https://docs.openzeppelin.com/relayer/");
}
