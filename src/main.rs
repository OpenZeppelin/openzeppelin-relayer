//! # OpenZeppelin Relayer
//!
//! A blockchain transaction relayer service that supports multiple networks
//! including EVM, Solana, and Stellar.
//!
//! ## Features
//!
//! - Multi-network support
//! - Transaction monitoring
//! - Policy enforcement
//! - REST API
//!
//! ## Architecture
//!
//! The service is built using Actix-web and provides:
//! - HTTP endpoints for transaction submission
//! - In-memory repository implementations
//! - Configurable network policies
//!
//! ## Usage
//!
//! ```bash
//! cargo run
//! ```

use std::sync::Arc;

use actix_web::{middleware, middleware::Logger, web, App, HttpServer};
use color_eyre::{eyre::WrapErr, Report, Result};
use config::Config;
use dotenvy::dotenv;
use futures::future::try_join_all;
use log::info;
use models::RelayerRepoModel;
use repositories::{InMemoryRelayerRepository, InMemoryTransactionRepository, Repository};
use simple_logger::SimpleLogger;

mod api;
mod config;
mod domain;
mod models;
mod repositories;
mod services;
pub use models::{ApiError, AppState};

/// Sets up logging and environment configuration
///
/// # Returns
///
/// * `Result<()>` - Setup result
///
/// # Errors
///
/// Returns error if:
/// - Environment file cannot be loaded
/// - Logger initialization fails
fn setup_logging_and_env() -> Result<()> {
    dotenv().ok();
    SimpleLogger::new()
        .env()
        .init()
        .wrap_err("Failed to initialize logger")
}

fn load_config_file() -> Result<Config> {
    config::load_config().wrap_err("Failed to load config file")
}

/// Initializes application state and repositories
///
/// # Returns
///
/// * `Result<web::Data<AppState>>` - Initialized application state
///
/// # Errors
///
/// Returns error if:
/// - Repository initialization fails
/// - Configuration loading fails
async fn initialize_app_state(config_file: Config) -> Result<web::ThinData<AppState>> {
    let relayer_repository = Arc::new(InMemoryRelayerRepository::new());
    let transaction_repository = Arc::new(InMemoryTransactionRepository::new());

    let app_state = web::ThinData(AppState {
        relayer_repository,
        transaction_repository,
    });

    let relayer_futures = config_file.relayers.iter().map(|relayer| async {
        let repo_model = RelayerRepoModel::try_from(relayer.clone())
            .wrap_err("Failed to convert relayer config")?;
        app_state
            .relayer_repository
            .create(repo_model)
            .await
            .wrap_err("Failed to create relayer repository entry")?;
        Ok::<(), Report>(())
    });

    try_join_all(relayer_futures)
        .await
        .wrap_err("Failed to initialize relayer repository")?;

    Ok(app_state)
}

#[actix_web::main]
async fn main() -> Result<()> {
    // Initialize error reporting with eyre
    color_eyre::install().wrap_err("Failed to initialize error reporting")?;

    setup_logging_and_env()?;

    let config_file = load_config_file()?;
    info!("Config: {:?}", config_file);

    let config = config::ServerConfig::from_env();

    let app_state = initialize_app_state(config_file).await?;

    info!("Starting server on {}:{}", config.host, config.port);
    HttpServer::new(move || {
        App::new()
            .wrap(middleware::Compress::default())
            .wrap(middleware::NormalizePath::trim())
            .wrap(middleware::DefaultHeaders::new())
            .wrap(Logger::default())
            .app_data(app_state.clone())
            .service(web::scope("/api/v1").configure(api::routes::configure_routes))
    })
    .bind((config.host.as_str(), config.port))
    .wrap_err_with(|| format!("Failed to bind server to {}:{}", config.host, config.port))?
    .shutdown_timeout(5)
    .run()
    .await
    .wrap_err("Server runtime error")
}
