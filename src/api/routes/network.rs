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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::mocks::mockutils::create_mock_app_state;
    use actix_web::{http::StatusCode, test, App};

    // ============================================
    // Route Registration Tests
    // ============================================

    #[actix_web::test]
    async fn test_network_routes_are_registered() {
        // Arrange - Create app with network routes
        let app_state = create_mock_app_state(None, None, None, None, None, None).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(app_state))
                .configure(init),
        )
        .await;

        // Test GET /networks - should not return 404 (route exists)
        let req = test::TestRequest::get().uri("/networks").to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "GET /networks route not registered"
        );

        // Test GET /networks/{network_id} - should not return 404
        let req = test::TestRequest::get()
            .uri("/networks/evm:sepolia")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "GET /networks/{{network_id}} route not registered"
        );

        // Test PATCH /networks/{network_id} - should not return 404
        let req = test::TestRequest::patch()
            .uri("/networks/evm:sepolia")
            .set_json(serde_json::json!({
                "rpc_urls": ["https://rpc.example.com"]
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "PATCH /networks/{{network_id}} route not registered"
        );
    }

    #[actix_web::test]
    async fn test_network_routes_with_query_params() {
        // Arrange - Create app with network routes
        let app_state = create_mock_app_state(None, None, None, None, None, None).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(app_state))
                .configure(init),
        )
        .await;

        // Test GET /networks with pagination parameters
        let req = test::TestRequest::get()
            .uri("/networks?page=1&per_page=10")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "GET /networks with query params route not registered"
        );
    }

    #[actix_web::test]
    async fn test_network_routes_with_special_characters_in_path() {
        // Network IDs use format like "evm:sepolia" which contains a colon
        let app_state = create_mock_app_state(None, None, None, None, None, None).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(app_state))
                .configure(init),
        )
        .await;

        // Test that route handles colons in path parameters
        let req = test::TestRequest::get()
            .uri("/networks/evm:mainnet")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "GET /networks/evm:mainnet route should handle colon in path"
        );

        let req = test::TestRequest::get()
            .uri("/networks/solana:devnet")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "GET /networks/solana:devnet route should handle colon in path"
        );

        let req = test::TestRequest::get()
            .uri("/networks/stellar:testnet")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "GET /networks/stellar:testnet route should handle colon in path"
        );
    }

    #[actix_web::test]
    async fn test_patch_network_route_accepts_json() {
        let app_state = create_mock_app_state(None, None, None, None, None, None).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(app_state))
                .configure(init),
        )
        .await;

        // Test PATCH with valid JSON payload structure
        let req = test::TestRequest::patch()
            .uri("/networks/evm:sepolia")
            .set_json(serde_json::json!({
                "rpc_urls": ["https://rpc1.example.com", "https://rpc2.example.com"]
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "PATCH /networks/{{network_id}} with JSON body route not registered"
        );
    }

    #[actix_web::test]
    async fn test_patch_network_route_accepts_weighted_rpc_urls() {
        let app_state = create_mock_app_state(None, None, None, None, None, None).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(app_state))
                .configure(init),
        )
        .await;

        // Test PATCH with weighted RPC URL format
        let req = test::TestRequest::patch()
            .uri("/networks/evm:sepolia")
            .set_json(serde_json::json!({
                "rpc_urls": [
                    {"url": "https://primary.example.com", "weight": 80},
                    {"url": "https://backup.example.com", "weight": 20}
                ]
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "PATCH /networks/{{network_id}} with weighted RPC URLs route not registered"
        );
    }

    #[actix_web::test]
    async fn test_patch_network_route_accepts_mixed_rpc_url_formats() {
        let app_state = create_mock_app_state(None, None, None, None, None, None).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(app_state))
                .configure(init),
        )
        .await;

        // Test PATCH with mixed RPC URL formats (strings and objects)
        let req = test::TestRequest::patch()
            .uri("/networks/evm:sepolia")
            .set_json(serde_json::json!({
                "rpc_urls": [
                    "https://simple.example.com",
                    {"url": "https://weighted.example.com", "weight": 50}
                ]
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "PATCH /networks/{{network_id}} with mixed RPC URL formats route not registered"
        );
    }
}
