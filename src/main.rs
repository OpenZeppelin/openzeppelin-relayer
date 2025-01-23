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

use actix_web::{
    middleware::{self, Logger},
    web::{self, ThinData},
    App, HttpServer,
};
use alloy::{
    network::{EthereumWallet, NetworkWallet},
    primitives::FixedBytes,
    signers::local::LocalSigner,
};
use color_eyre::{eyre::WrapErr, Report, Result};
use config::Config;
use dotenvy::dotenv;
use futures::future::try_join_all;
use jobs::{setup_workers, Queue};
use log::{error, info};
use models::{RelayerRepoModel, SignerRepoModel};
use oz_keystore::LocalClient;
use repositories::{
    InMemoryRelayerRepository, InMemorySignerRepository, InMemoryTransactionRepository, Repository,
};
use simple_logger::SimpleLogger;

mod api;
mod config;
mod domain;
mod jobs;
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

fn load_config_file(config_file_path: &str) -> Result<Config> {
    config::load_config(config_file_path).wrap_err("Failed to load config file")
}

/// Initializes application state
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
async fn initialize_app_state() -> Result<web::ThinData<AppState>> {
    let relayer_repository = Arc::new(InMemoryRelayerRepository::new());
    let transaction_repository = Arc::new(InMemoryTransactionRepository::new());
    let signer_repository = Arc::new(InMemorySignerRepository::new());

    let queue = Queue::setup().await;
    let job_producer = Arc::new(jobs::JobProducer::new(queue.clone()));

    let app_state = web::ThinData(AppState {
        relayer_repository,
        transaction_repository,
        signer_repository,
        job_producer,
    });

    Ok(app_state)
}

async fn process_config_file(config_file: Config, app_state: ThinData<AppState>) -> Result<()> {
    error!("Loaded key");
    let key_raw = LocalClient::load(
        "examples/basic-example/keys/local-signer.json".into(),
        "test".into(),
    );
    // transforms the key into alloy wallet
    let key_bytes = FixedBytes::from_slice(&key_raw);
    let signer = LocalSigner::from_bytes(&key_bytes).expect("failed to create signer");

    // info!("Key {:?}", key_raw);

    let relayer_futures = config_file.relayers.iter().map(|relayer| async {
        let repo_model = RelayerRepoModel::try_from(relayer.clone())
            .wrap_err("Failed to convert relayer config")?;
        // TODO populate address
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

    let signer_futures = config_file.signers.iter().map(|signer| async {
        let repo_model = SignerRepoModel::try_from(signer.clone())
            .wrap_err("Failed to convert signer config")?;
        // TODO populate address
        app_state
            .signer_repository
            .create(repo_model)
            .await
            .wrap_err("Failed to create signer repository entry")?;
        Ok::<(), Report>(())
    });

    try_join_all(signer_futures)
        .await
        .wrap_err("Failed to initialize signer repository")?;

    let signers = app_state.signer_repository.list_all().await?;

    info!("Signers: {:?}", signers);

    Ok(())
}

#[actix_web::main]
async fn main() -> Result<()> {
    // Initialize error reporting with eyre
    color_eyre::install().wrap_err("Failed to initialize error reporting")?;

    setup_logging_and_env()?;

    let config = config::ServerConfig::from_env();
    // info!("Config: {:?}", config_file);
    let config_file = load_config_file(&config.config_file_path)?;

    let app_state = initialize_app_state().await?;

    // Setup workers for processing jobs
    setup_workers(app_state.clone()).await?;

    info!("Processing config file");
    process_config_file(config_file, app_state.clone()).await?;

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
