//! # Notifications Controller
//!
//! Handles HTTP endpoints for notification operations including:
//! - Listing notifications
//! - Getting notification details
//! - Creating notifications
//! - Updating notifications
//! - Deleting notifications

use crate::{
    models::{
        ApiError, ApiResponse, DefaultAppState, NotificationCreateRequest, NotificationRepoModel,
        NotificationResponse, NotificationUpdateRequest, PaginationMeta, PaginationQuery,
    },
    repositories::Repository,
};
use actix_web::{web, HttpResponse};
use eyre::Result;

/// Lists all notifications with pagination support.
///
/// # Arguments
///
/// * `query` - The pagination query parameters.
/// * `state` - The application state containing the notification repository.
///
/// # Returns
///
/// A paginated list of notifications.
pub async fn list_notifications(
    query: PaginationQuery,
    state: web::ThinData<DefaultAppState>,
) -> Result<HttpResponse, ApiError> {
    let notifications = state.notification_repository.list_paginated(query).await?;

    let mapped_notifications: Vec<NotificationResponse> =
        notifications.items.into_iter().map(|n| n.into()).collect();

    Ok(HttpResponse::Ok().json(ApiResponse::paginated(
        mapped_notifications,
        PaginationMeta {
            total_items: notifications.total,
            current_page: notifications.page,
            per_page: notifications.per_page,
        },
    )))
}

/// Retrieves details of a specific notification by ID.
///
/// # Arguments
///
/// * `notification_id` - The ID of the notification to retrieve.
/// * `state` - The application state containing the notification repository.
///
/// # Returns
///
/// The notification details or an error if not found.
pub async fn get_notification(
    notification_id: String,
    state: web::ThinData<DefaultAppState>,
) -> Result<HttpResponse, ApiError> {
    let notification = state
        .notification_repository
        .get_by_id(notification_id)
        .await?;

    let response = NotificationResponse::from(notification);
    Ok(HttpResponse::Ok().json(ApiResponse::success(response)))
}

/// Creates a new notification.
///
/// # Arguments
///
/// * `request` - The notification creation request.
/// * `state` - The application state containing the notification repository.
///
/// # Returns
///
/// The created notification or an error if creation fails.
pub async fn create_notification(
    request: NotificationCreateRequest,
    state: web::ThinData<DefaultAppState>,
) -> Result<HttpResponse, ApiError> {
    let notification_model = NotificationRepoModel::from(request);
    let created_notification = state
        .notification_repository
        .create(notification_model)
        .await?;

    let response = NotificationResponse::from(created_notification);
    Ok(HttpResponse::Created().json(ApiResponse::success(response)))
}

/// Updates an existing notification.
///
/// # Arguments
///
/// * `notification_id` - The ID of the notification to update.
/// * `request` - The notification update request.
/// * `state` - The application state containing the notification repository.
///
/// # Returns
///
/// The updated notification or an error if update fails.
pub async fn update_notification(
    notification_id: String,
    request: NotificationUpdateRequest,
    state: web::ThinData<DefaultAppState>,
) -> Result<HttpResponse, ApiError> {
    // Get the existing notification
    let existing_notification = state
        .notification_repository
        .get_by_id(notification_id.clone())
        .await?;

    // Apply the update to the existing notification
    let updated_notification = request.apply_to(existing_notification);

    // Save the updated notification
    let saved_notification = state
        .notification_repository
        .update(notification_id, updated_notification)
        .await?;

    let response = NotificationResponse::from(saved_notification);
    Ok(HttpResponse::Ok().json(ApiResponse::success(response)))
}

/// Deletes a notification by ID.
///
/// # Arguments
///
/// * `notification_id` - The ID of the notification to delete.
/// * `state` - The application state containing the notification repository.
///
/// # Returns
///
/// A success response or an error if deletion fails.
pub async fn delete_notification(
    notification_id: String,
    state: web::ThinData<DefaultAppState>,
) -> Result<HttpResponse, ApiError> {
    state
        .notification_repository
        .delete_by_id(notification_id)
        .await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success("Notification deleted successfully")))
}
