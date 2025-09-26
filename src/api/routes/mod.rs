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

pub mod api_keys;
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
        .configure(signer::init)
        .configure(api_keys::init);
}

#[cfg(test)]
mod tests {
    use crate::models::{ApiError, DefaultAppState};
    use actix_web::{test, web, App, HttpRequest, HttpResponse, Responder};
    use relayer_macros::require_permissions;

    // Simple mock endpoint for testing the macro
    async fn mock_endpoint_without_macro() -> impl Responder {
        HttpResponse::Ok().json("mock works!")
    }

    // Mock endpoint with require_permissions macro using const
    #[require_permissions(["relayers:get:all"])]
    async fn mock_endpoint_with_macro(
        raw_request: HttpRequest,
        data: web::ThinData<DefaultAppState>,
    ) -> impl Responder {
        async fn inner() -> Result<HttpResponse, ApiError> {
            Ok(HttpResponse::Ok().json("macro works!"))
        }
        inner().await
    }

    #[actix_web::test]
    async fn test_require_permissions_macro_compiles() {
        // Simple test to verify the macro compiles with const definitions
        let app = test::init_service(
            App::new()
                .route("/test-no-macro", web::get().to(mock_endpoint_without_macro))
                .route("/test-with-macro", web::get().to(mock_endpoint_with_macro)),
        )
        .await;

        // Test endpoint without macro works
        let req = test::TestRequest::get().uri("/test-no-macro").to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        let req = test::TestRequest::get()
            .uri("/test-with-macro")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_server_error());
    }
}
