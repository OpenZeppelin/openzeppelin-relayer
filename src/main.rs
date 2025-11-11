//! Top-level crate documentation.
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
use actix_web::HttpResponse;
use actix_web::{
    dev::Service,
    middleware::{self},
    web::{self},
    App, HttpServer,
};
use color_eyre::{eyre::WrapErr, Result};
use config::Config;
use futures::future::{ready, Either};
use metrics::middleware::MetricsMiddleware;

use config::ApiKeyRateLimit;
use dotenvy::dotenv;
use std::env;
use tracing::info;

use openzeppelin_relayer::{
    api,
    bootstrap::{
        initialize_app_state, initialize_relayers, initialize_token_swap_workers,
        initialize_workers, process_config_file,
    },
    config,
    constants::PUBLIC_ENDPOINTS,
    logging::setup_logging,
    metrics,
    observability::RequestIdMiddleware,
    utils::check_authorization_header,
};
use tracing_actix_web::TracingLogger;

fn load_config_file(config_file_path: &str) -> Result<Config> {
    config::load_config(config_file_path).wrap_err("Failed to load config file")
}

#[actix_web::main]
async fn main() -> Result<()> {
    dotenv().ok();

    // Setup logging first so ErrorLayer is available
    setup_logging();

    // Initialize error reporting with eyre (after logging setup)
    color_eyre::install().wrap_err("Failed to initialize error reporting")?;

    // Log service information at startup
    openzeppelin_relayer::utils::log_service_info();

    // Set metrics enabled flag to false by default
    let metrics_enabled = env::var("METRICS_ENABLED")
        .map(|v| v.to_lowercase() == "true")
        .unwrap_or(false);

    let config = Arc::new(config::ServerConfig::from_env());
    let server_config = Arc::clone(&config); // clone for use in binding below
    let config_file = load_config_file(&config.config_file_path)?;

    let app_state = initialize_app_state(server_config.clone()).await?;

    process_config_file(config_file, server_config.clone(), &app_state).await?;

    info!("Initializing relayers");
    // Initialize relayers: sync and validate relayers
    initialize_relayers(app_state.clone()).await?;

    initialize_token_swap_workers(app_state.clone()).await?;

    // Setup workers for processing jobs
    initialize_workers(app_state.clone()).await?;

    // Rate limit configuration
    let rate_limit_config = GovernorConfigBuilder::default()
        .requests_per_second(config.rate_limit_requests_per_second)
        .key_extractor(ApiKeyRateLimit)
        .burst_size(config.rate_limit_burst_size)
        .finish()
        .unwrap();

    info!(host = %config.host, port = %config.port, "starting server");
    let app_server = HttpServer::new({
        // Clone the config for use within the closure.
        let server_config_clone = Arc::clone(&server_config);
        let app_state = app_state.clone();
        move || {
            let config = Arc::clone(&server_config_clone);
            let app = App::new();

            app
            .wrap_fn(move |req, srv| {
                let path = req.path();
                let is_public = PUBLIC_ENDPOINTS.iter().any(|p| path.starts_with(p));
                let authorized = is_public || check_authorization_header(&req, &config.api_key);

                if authorized {
                    Either::Left(srv.call(req))
                } else {
                    let res = HttpResponse::Unauthorized()
                        .body(r#"{"success":false,"code":401,"error":"Unauthorized","message":"Unauthorized"}"#)
                        .map_into_boxed_body();
                    Either::Right(ready(Ok(req.into_response(res))))
                }
            })
            .wrap(RequestIdMiddleware)
            .wrap(TracingLogger::default())
            .wrap(MetricsMiddleware)
            .wrap(Governor::new(&rate_limit_config))
            .wrap(middleware::DefaultHeaders::new())
            .wrap(middleware::NormalizePath::trim())
            .wrap(middleware::Compress::default())
            .app_data(app_state.clone())
            .service(web::scope("/api/v1").configure(api::routes::configure_routes))
        }
    })
    .bind((config.host.as_str(), config.port))
    .wrap_err_with(|| format!("Failed to bind server to {}:{}", config.host, config.port))?
    .shutdown_timeout(5)
    .run();

    let metrics_server_future = if metrics_enabled {
        tracing::info!("metrics server enabled, starting metrics server");
        Some(
            HttpServer::new(|| {
                App::new()
                    .wrap(middleware::Compress::default())
                    .wrap(middleware::NormalizePath::trim())
                    .wrap(middleware::DefaultHeaders::new())
                    .configure(api::routes::metrics::init)
            })
            .workers(2)
            .bind((config.host.as_str(), config.metrics_port))
            .wrap_err_with(|| {
                format!(
                    "Failed to bind server to {}:{}",
                    config.host, config.metrics_port
                )
            })?
            .shutdown_timeout(5)
            .run(),
        )
    } else {
        tracing::info!("metrics server disabled");
        None
    };

    if let Some(metrics_server) = metrics_server_future {
        futures::try_join!(app_server, metrics_server)?;
    } else {
        app_server.await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::test::TestRequest;
    use openzeppelin_relayer::{
        constants::{AUTHORIZATION_HEADER_NAME, AUTHORIZATION_HEADER_VALUE_PREFIX},
        models::SecretString,
    };

    #[test]
    fn test_check_authorization_header_success() {
        let req = TestRequest::default()
            .insert_header((
                AUTHORIZATION_HEADER_NAME,
                format!("{}{}", AUTHORIZATION_HEADER_VALUE_PREFIX, "test_key"),
            ))
            .to_srv_request();

        assert!(check_authorization_header(
            &req,
            &SecretString::new("test_key")
        ));
    }

    #[test]
    fn test_check_authorization_header_missing_header() {
        let req = TestRequest::default().to_srv_request();

        assert!(!check_authorization_header(
            &req,
            &SecretString::new("test_key")
        ));
    }

    #[test]
    fn test_check_authorization_header_invalid_prefix() {
        let req = TestRequest::default()
            .insert_header((AUTHORIZATION_HEADER_NAME, "InvalidPrefix test_key"))
            .to_srv_request();

        assert!(!check_authorization_header(
            &req,
            &SecretString::new("test_key")
        ));
    }

    #[test]
    fn test_check_authorization_header_invalid_key() {
        let req = TestRequest::default()
            .insert_header((
                AUTHORIZATION_HEADER_NAME,
                format!("{}{}", AUTHORIZATION_HEADER_VALUE_PREFIX, "invalid_key"),
            ))
            .to_srv_request();

        assert!(!check_authorization_header(
            &req,
            &SecretString::new("test_key")
        ));
    }
}
