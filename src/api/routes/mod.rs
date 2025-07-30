//! # API Routes Module
//!
//! Configures HTTP routes for the relayer service API.
//!
//! ## Routes
//!
//! * `/health` - Health check endpoints
//! * `/relayers` - Relayer management endpoints
//! * `/notifications` - Notification management endpoints
//! * `/signers` - Signer management endpoints

pub mod docs;
pub mod health;
pub mod metrics;
pub mod notification;
pub mod plugin;
pub mod relayer;
pub mod signer;

use actix_web::web;
pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.configure(health::init)
        .configure(relayer::init)
        .configure(plugin::init)
        .configure(metrics::init)
        .configure(notification::init)
        .configure(signer::init);
}
