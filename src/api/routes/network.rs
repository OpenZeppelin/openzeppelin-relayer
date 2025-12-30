//! This module defines the HTTP routes for network operations.
//! It includes handlers for listing, retrieving, and updating network configurations.
//! The routes are integrated with the Actix-web framework and interact with the network controller.

use crate::{
    api::controllers::network,
    models::{DefaultAppState, PaginationQuery, UpdateNetworkRequest},
};
use actix_web::{get, patch, web, Responder};

/// Lists all networks with pagination support.
#[get("/networks")]
async fn list_networks(
    query: web::Query<PaginationQuery>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    network::list_networks(query.into_inner(), data).await
}

/// Retrieves details of a specific network by ID.
#[get("/networks/{network_id}")]
async fn get_network(
    network_id: web::Path<String>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    network::get_network(network_id.into_inner(), data).await
}

/// Updates a network's configuration.
/// Currently supports updating RPC URLs only. Can be extended to support other fields.
#[patch("/networks/{network_id}")]
async fn update_network(
    network_id: web::Path<String>,
    request: web::Json<UpdateNetworkRequest>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    network::update_network(network_id.into_inner(), request.into_inner(), data).await
}

/// Initializes the routes for the network module.
pub fn init(cfg: &mut web::ServiceConfig) {
    // Register routes with literal segments before routes with path parameters
    cfg.service(update_network); // /networks/{network_id}
    cfg.service(get_network); // /networks/{network_id}
    cfg.service(list_networks); // /networks
}

