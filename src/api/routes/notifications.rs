//! This module defines the HTTP routes for notification operations.
//! It includes handlers for listing, retrieving, creating, updating, and deleting notifications.
//! The routes are integrated with the Actix-web framework and interact with the notification controller.

use crate::{
    api::controllers::notifications,
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
    notifications::list_notifications(query.into_inner(), data).await
}

/// Retrieves details of a specific notification by ID.
#[get("/notifications/{notification_id}")]
async fn get_notification(
    notification_id: web::Path<String>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    notifications::get_notification(notification_id.into_inner(), data).await
}

/// Creates a new notification.
#[post("/notifications")]
async fn create_notification(
    request: web::Json<NotificationCreateRequest>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    notifications::create_notification(request.into_inner(), data).await
}

/// Updates an existing notification.
#[patch("/notifications/{notification_id}")]
async fn update_notification(
    notification_id: web::Path<String>,
    request: web::Json<NotificationUpdateRequest>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    notifications::update_notification(notification_id.into_inner(), request.into_inner(), data)
        .await
}

/// Deletes a notification by ID.
#[delete("/notifications/{notification_id}")]
async fn delete_notification(
    notification_id: web::Path<String>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    notifications::delete_notification(notification_id.into_inner(), data).await
}

/// Configures the notification routes.
pub fn init(cfg: &mut web::ServiceConfig) {
    cfg.service(list_notifications)
        .service(get_notification)
        .service(create_notification)
        .service(update_notification)
        .service(delete_notification);
}
