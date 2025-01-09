use actix_web::middleware::Logger;
use actix_web::{middleware, web, App, HttpServer};
use color_eyre::Result;
use config::Config;
use dotenvy::dotenv;
use futures::future::try_join_all;
use log::{error, info};
use models::RelayerRepoModel;
use repositories::{InMemoryRelayerRepository, Repository};
use simple_logger::SimpleLogger;

mod config;
mod controllers;
mod errors;
mod models;
mod repositories;
mod routes;
mod services;
pub use errors::*;
pub use models::AppState;

fn setup_logging_and_env() {
    dotenv().ok();
    if let Err(e) = SimpleLogger::new().env().init() {
        eprintln!("Failed to initialize logger: {}", e);
    }
}

fn load_config_file() -> Config {
    config::load_config().expect("Failed to load config file")
}

async fn initialize_app_state(config_file: Config) -> Result<web::Data<AppState>, std::io::Error> {
    let relayer_repository = InMemoryRelayerRepository::new();
    let transaction_repository = repositories::InMemoryTransactionRepository::new();
    let app_state = web::Data::new(AppState {
        relayer_repository,
        transaction_repository,
    });

    let relayer_futures = config_file.relayers.iter().map(|relayer| async {
        let repo_model = RelayerRepoModel::try_from(relayer.clone()).map_err(|e| {
            error!("Failed to convert relayer config: {:?}", e);
            std::io::Error::new(std::io::ErrorKind::Other, e)
        })?;
        if let Err(e) = app_state.relayer_repository.create(repo_model).await {
            error!("Failed to create relayer repository entry: {:?}", e);
            // TODO verify if this is the correct way to handle this error
            return Err(std::io::Error::new(std::io::ErrorKind::Other, e));
        }
        Ok::<(), std::io::Error>(())
    });

    try_join_all(relayer_futures).await.map_err(|e| {
        error!("Failed to initialize relayer repository: {:?}", e);
        e
    })?;

    Ok(app_state)
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Load environment variables from .env file
    dotenv().ok();

    let config = config::ServerConfig::from_env();

    setup_logging_and_env();

    let config_file = load_config_file();
    info!("Config: {:?}", config_file);

    let app_state = initialize_app_state(config_file).await?;

    info!("Starting server on {}:{}", config.host, config.port);
    let server = HttpServer::new(move || {
        App::new()
            .wrap(middleware::Compress::default())
            .wrap(middleware::NormalizePath::trim())
            .wrap(middleware::DefaultHeaders::new())
            .wrap(Logger::default())
            .app_data(app_state.clone())
            .configure(routes::configure_routes)
    })
    .bind((config.host.as_str(), config.port))?
    .shutdown_timeout(5);

    info!("Server running at http://{}:{}", config.host, config.port);

    server.run().await
}
