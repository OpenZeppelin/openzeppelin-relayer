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

use actix_governor::{Governor, GovernorConfigBuilder};
use actix_web::{
    dev::Service,
    middleware::{self, Logger},
    web::{self},
    App, HttpServer,
};
use color_eyre::{eyre::WrapErr, Result};
use config::Config;

use actix_web::HttpResponse;

use config::ApiKeyRateLimit;
use dotenvy::dotenv;
use jobs::{setup_workers, Queue};
use log::info;
use repositories::{
    InMemoryNotificationRepository, InMemoryRelayerRepository, InMemorySignerRepository,
    InMemoryTransactionRepository,
};
use simple_logger::SimpleLogger;

mod api;
mod config;
mod domain;
mod jobs;
mod models;
mod repositories;
mod services;
mod utils;
pub use models::{ApiError, AppState};
mod config_processor;
use config_processor::process_config_file;

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
    let notification_repository = Arc::new(InMemoryNotificationRepository::new());

    let queue = Queue::setup().await;
    let job_producer = Arc::new(jobs::JobProducer::new(queue.clone()));

    let app_state = web::ThinData(AppState {
        relayer_repository,
        transaction_repository,
        signer_repository,
        notification_repository,
        job_producer,
    });

    Ok(app_state)
}

#[actix_web::main]
async fn main() -> Result<()> {
    // Initialize error reporting with eyre
    color_eyre::install().wrap_err("Failed to initialize error reporting")?;

    setup_logging_and_env()?;

    let config = Arc::new(config::ServerConfig::from_env());
    let config_file = load_config_file(&config.config_file_path)?;

    let app_state = initialize_app_state().await?;

    // Setup workers for processing jobs
    setup_workers(app_state.clone()).await?;

    info!("Processing config file");
    process_config_file(config_file, app_state.clone()).await?;

    // Rate limit configuration
    let rate_limit_config = GovernorConfigBuilder::default()
        .requests_per_second(config.rate_limit_requests_per_second)
        .key_extractor(ApiKeyRateLimit)
        .burst_size(config.rate_limit_burst_size)
        .finish()
        .unwrap();

    let moved_cfg = Arc::clone(&config);
    info!("Starting server on {}:{}", config.host, config.port);
    HttpServer::new(move || {
        let config = Arc::clone(&moved_cfg);
        App::new()
            .wrap_fn(move |req, srv| {
                // Check for x-api-key header
                let expected_key = config.api_key.clone();
                if let Some(header_value) = req.headers().get("x-api-key") {
                    if let Ok(key) = header_value.to_str() {
                        if key == expected_key {
                            return srv.call(req);
                        }
                    }
                }

                Box::pin(async move {
                    Ok(req.into_response(
                        HttpResponse::Unauthorized().body(
                            r#"{"success": false, "code":401, "error": "Unauthorized", "message": "Unauthorized"}"#.to_string(),
                        ),
                    ))
                })
            })
            .wrap(Governor::new(&rate_limit_config))
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
