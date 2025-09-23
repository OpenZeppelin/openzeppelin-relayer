//! This module defines the HTTP routes for api keys operations.
//! The routes are integrated with the Actix-web framework and interact with the api key controller.
use crate::{
    api::controllers::api_key,
    models::{ApiKeyRequest, DefaultAppState, PaginationQuery},
};
use actix_web::{delete, get, post, web, Responder};

/// List API keys
#[get("/api-keys")]
async fn list_api_keys(
    query: web::Query<PaginationQuery>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    api_key::list_api_keys(query.into_inner(), data).await
}

#[get("/api-keys/{api_key_id}/permissions")]
async fn get_api_key_permissions(
    api_key_id: web::Path<String>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    api_key::get_api_key_permissions(api_key_id.into_inner(), data).await
}

/// Create a new API key
#[post("/api-keys")]
async fn create_api_key(
    req: web::Json<ApiKeyRequest>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    api_key::create_api_key(req.into_inner(), data).await
}

#[delete("/api-keys/{api_key_id}")]
async fn delete_api_key(
    api_key_id: web::Path<String>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    api_key::delete_api_key(api_key_id.into_inner(), data).await
}

/// Initializes the routes for api keys.
pub fn init(cfg: &mut web::ServiceConfig) {
    // Register routes with literal segments before routes with path parameters
    cfg.service(create_api_key); // /api-keys
    cfg.service(list_api_keys); // /api-keys
    cfg.service(get_api_key_permissions); // /api-keys/{api_key_id}/permissions
    cfg.service(delete_api_key); // /api-keys/{api_key_id}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::mocks::mockutils::create_mock_app_state;
    use actix_web::{http::StatusCode, test, web, App};

    #[actix_web::test]
    async fn test_api_key_routes_are_registered() {
        // Arrange - Create app with API key routes
        let app_state = create_mock_app_state(None, None, None, None, None, None).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(app_state))
                .configure(init),
        )
        .await;

        // Test GET /api-keys - should not return 404 (route exists)
        let req = test::TestRequest::get().uri("/api-keys").to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "GET /api-keys route not registered"
        );

        // Test POST /api-keys - should not return 404
        let req = test::TestRequest::post()
            .uri("/api-keys")
            .set_json(serde_json::json!({
                "name": "Test API Key",
                "permissions": ["relayer:all:execute"],
                "allowed_origins": ["*"]
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "POST /api-keys route not registered"
        );

        // Test GET /api-keys/{api_key_id}/permissions - should not return 404
        let req = test::TestRequest::get()
            .uri("/api-keys/test-id/permissions")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "GET /api-keys/{{api_key_id}}/permissions route not registered"
        );

        // Test DELETE /api-keys/{api_key_id} - should not return 404
        let req = test::TestRequest::delete()
            .uri("/api-keys/test-id")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "DELETE /api-keys/{{api_key_id}} route not registered"
        );
    }

    #[actix_web::test]
    async fn test_api_key_routes_with_query_params() {
        // Arrange - Create app with API key routes
        let app_state = create_mock_app_state(None, None, None, None, None, None).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(app_state))
                .configure(init),
        )
        .await;

        // Test GET /api-keys with pagination parameters
        let req = test::TestRequest::get()
            .uri("/api-keys?page=1&per_page=10")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "GET /api-keys with query params route not registered"
        );
    }
}
