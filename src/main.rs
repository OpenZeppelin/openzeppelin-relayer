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
use logging::setup_logging;

use actix_web::HttpResponse;

use config::ApiKeyRateLimit;
use dotenvy::dotenv;
use init::{initialize_app_state, initialize_relayers, initialize_workers, process_config_file};
use log::info;

mod api;
mod config;
mod constants;
mod domain;
mod init;
mod jobs;
mod logging;
mod models;
mod repositories;
mod services;
mod utils;
pub use models::{ApiError, AppState};

fn load_config_file(config_file_path: &str) -> Result<Config> {
    config::load_config(config_file_path).wrap_err("Failed to load config file")
}

#[actix_web::main]
async fn main() -> Result<()> {
    // Initialize error reporting with eyre
    color_eyre::install().wrap_err("Failed to initialize error reporting")?;

    dotenv().ok();
    setup_logging();

    let config = Arc::new(config::ServerConfig::from_env());
    let config_file = load_config_file(&config.config_file_path)?;

    let app_state = initialize_app_state().await?;

    // Setup workers for processing jobs
    initialize_workers(app_state.clone()).await?;

    info!("Processing config file");
    process_config_file(config_file, app_state.clone()).await?;

    // Initialize relayers: sync and validate relayers
    initialize_relayers(app_state.clone()).await?;

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
