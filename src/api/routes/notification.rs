//! This module defines the HTTP routes for notification operations.
//! It includes handlers for listing, retrieving, creating, updating, and deleting notifications.
//! The routes are integrated with the Actix-web framework and interact with the notification controller.

use crate::{
    api::controllers::notification,
    models::{
        DefaultAppState, NotificationCreateRequest, NotificationUpdateRequest, PaginationQuery,
    },
};
use actix_web::{delete, get, patch, post, web, Responder};

/// Lists all notifications with pagination support.
#[get("/notifications")]
async fn list_notifications(
    query: web::Query<PaginationQuery>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    notification::list_notifications(query.into_inner(), data).await
}

/// Retrieves details of a specific notification by ID.
#[get("/notifications/{notification_id}")]
async fn get_notification(
    notification_id: web::Path<String>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    notification::get_notification(notification_id.into_inner(), data).await
}

/// Creates a new notification.
#[post("/notifications")]
async fn create_notification(
    request: web::Json<NotificationCreateRequest>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    notification::create_notification(request.into_inner(), data).await
}

/// Updates an existing notification.
#[patch("/notifications/{notification_id}")]
async fn update_notification(
    notification_id: web::Path<String>,
    request: web::Json<NotificationUpdateRequest>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    notification::update_notification(notification_id.into_inner(), request.into_inner(), data)
        .await
}

/// Deletes a notification by ID.
#[delete("/notifications/{notification_id}")]
async fn delete_notification(
    notification_id: web::Path<String>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    notification::delete_notification(notification_id.into_inner(), data).await
}

/// Configures the notification routes.
pub fn init(cfg: &mut web::ServiceConfig) {
    cfg.service(list_notifications)
        .service(get_notification)
        .service(create_notification)
        .service(update_notification)
        .service(delete_notification);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::mocks::mockutils::create_mock_app_state;
    use actix_web::{http::StatusCode, test, web, App};

    #[actix_web::test]
    async fn test_notification_routes_are_registered() {
        // Arrange - Create app with notification routes
        let app_state = create_mock_app_state(None, None, None, None, None).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(app_state))
                .configure(init),
        )
        .await;

        // Test GET /notifications - should not return 404 (route exists)
        let req = test::TestRequest::get().uri("/notifications").to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "GET /notifications route not registered"
        );

        // Test GET /notifications/{id} - should not return 404
        let req = test::TestRequest::get()
            .uri("/notifications/test-id")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "GET /notifications/{{id}} route not registered"
        );

        // Test POST /notifications - should not return 404
        let req = test::TestRequest::post()
            .uri("/notifications")
            .set_json(serde_json::json!({
                "id": "test",
                "type": "webhook",
                "url": "https://example.com"
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "POST /notifications route not registered"
        );

        // Test PATCH /notifications/{id} - should not return 404
        let req = test::TestRequest::patch()
            .uri("/notifications/test-id")
            .set_json(serde_json::json!({"url": "https://updated.com"}))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "PATCH /notifications/{{id}} route not registered"
        );

        // Test DELETE /notifications/{id} - should not return 404
        let req = test::TestRequest::delete()
            .uri("/notifications/test-id")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "DELETE /notifications/{{id}} route not registered"
        );
    }
}
