//! This module defines the HTTP routes for signer operations.
//! It includes handlers for listing, retrieving, creating, updating, and deleting signers.
//! The routes are integrated with the Actix-web framework and interact with the signer controller.

use crate::{
    api::controllers::signer,
    models::{DefaultAppState, PaginationQuery, SignerCreateRequest, SignerUpdateRequest},
};
use actix_web::{delete, get, patch, post, web, Responder};

/// Lists all signers with pagination support.
#[get("/signers")]
async fn list_signers(
    query: web::Query<PaginationQuery>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    signer::list_signers(query.into_inner(), data).await
}

/// Retrieves details of a specific signer by ID.
#[get("/signers/{signer_id}")]
async fn get_signer(
    signer_id: web::Path<String>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    signer::get_signer(signer_id.into_inner(), data).await
}

/// Creates a new signer.
#[post("/signers")]
async fn create_signer(
    request: web::Json<SignerCreateRequest>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    signer::create_signer(request.into_inner(), data).await
}

/// Updates an existing signer.
#[patch("/signers/{signer_id}")]
async fn update_signer(
    signer_id: web::Path<String>,
    request: web::Json<SignerUpdateRequest>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    signer::update_signer(signer_id.into_inner(), request.into_inner(), data).await
}

/// Deletes a signer by ID.
#[delete("/signers/{signer_id}")]
async fn delete_signer(
    signer_id: web::Path<String>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    signer::delete_signer(signer_id.into_inner(), data).await
}

/// Configures the signer routes.
pub fn init(cfg: &mut web::ServiceConfig) {
    cfg.service(list_signers)
        .service(get_signer)
        .service(create_signer)
        .service(update_signer)
        .service(delete_signer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::mocks::mockutils::create_mock_app_state;
    use actix_web::{http::StatusCode, test, web, App};

    #[actix_web::test]
    async fn test_signer_routes_are_registered() {
        // Arrange - Create app with signer routes
        let app_state = create_mock_app_state(None, None, None, None, None).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(app_state))
                .configure(init),
        )
        .await;

        // Test GET /signers - should not return 404 (route exists)
        let req = test::TestRequest::get().uri("/signers").to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "GET /signers route not registered"
        );

        // Test GET /signers/{id} - should not return 404
        let req = test::TestRequest::get()
            .uri("/signers/test-id")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "GET /signers/{{id}} route not registered"
        );

        // Test POST /signers - should not return 404
        let req = test::TestRequest::post()
            .uri("/signers")
            .set_json(serde_json::json!({
                "id": "test",
                "signer_type": "test",
                "name": "Test Signer",
                "description": "A test signer"
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "POST /signers route not registered"
        );

        // Test PATCH /signers/{id} - should not return 404
        let req = test::TestRequest::patch()
            .uri("/signers/test-id")
            .set_json(serde_json::json!({"name": "Updated Name"}))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "PATCH /signers/{{id}} route not registered"
        );

        // Test DELETE /signers/{id} - should not return 404
        let req = test::TestRequest::delete()
            .uri("/signers/test-id")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "DELETE /signers/{{id}} route not registered"
        );
    }
}
