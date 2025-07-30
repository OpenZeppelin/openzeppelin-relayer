//! # API Routes Module
//!
//! Configures HTTP routes for the relayer service API.
//!
//! ## Routes
//!
//! * `/health` - Health check endpoints
//! * `/relayers` - Relayer management endpoints

pub mod api_keys;
pub mod docs;
pub mod health;
pub mod metrics;
pub mod plugin;
pub mod relayer;

use actix_web::web;
pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.configure(health::init)
        .configure(relayer::init)
        .configure(plugin::init)
        .configure(metrics::init);

    #[cfg(feature = "authV2")]
    cfg.configure(api_keys::init);
}
