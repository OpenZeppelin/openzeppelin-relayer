//! # API Routes Module
//!
//! Configures HTTP routes for the relayer service API.
//!
//! ## Routes
//!
//! * `/health` - Health check endpoints
//! * `/relayers` - Relayer management endpoints

pub mod health;
pub mod relayer;

use actix_web::web;

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.configure(health::init).configure(relayer::init);
}
