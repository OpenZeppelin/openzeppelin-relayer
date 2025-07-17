use crate::models::{
    ApiResponse, NotificationCreateRequest, NotificationResponse, NotificationUpdateRequest,
};

/// Notification routes implementation
///
/// Note: OpenAPI documentation for these endpoints can be found in the `openapi.rs` file
///
/// Lists all notifications with pagination support.
#[utoipa::path(
    get,
    path = "/api/v1/notifications",
    tag = "Notifications",
    operation_id = "listNotifications",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("page" = Option<usize>, Query, description = "Page number for pagination (starts at 1)"),
        ("per_page" = Option<usize>, Query, description = "Number of items per page (default: 10)")
    ),
    responses(
        (
            status = 200,
            description = "Notification list retrieved successfully",
            body = ApiResponse<Vec<NotificationResponse>>
        ),
        (
            status = 400,
            description = "Bad Request",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Bad Request",
                "data": null
            })
        ),
        (
            status = 401,
            description = "Unauthorized",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Unauthorized",
                "data": null
            })
        ),
        (
            status = 500,
            description = "Internal Server Error",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Internal Server Error",
                "data": null
            })
        )
    )
)]
#[allow(dead_code)]
fn doc_list_notifications() {}

/// Retrieves details of a specific notification by ID.
#[utoipa::path(
    get,
    path = "/api/v1/notifications/{notification_id}",
    tag = "Notifications",
    operation_id = "getNotification",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("notification_id" = String, Path, description = "Notification ID")
    ),
    responses(
        (
            status = 200,
            description = "Notification retrieved successfully",
            body = ApiResponse<NotificationResponse>
        ),
        (
            status = 400,
            description = "Bad Request",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Bad Request",
                "data": null
            })
        ),
        (
            status = 401,
            description = "Unauthorized",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Unauthorized",
                "data": null
            })
        ),
        (
            status = 404,
            description = "Notification not found",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Notification not found",
                "data": null
            })
        ),
        (
            status = 500,
            description = "Internal Server Error",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Internal Server Error",
                "data": null
            })
        )
    )
)]
#[allow(dead_code)]
fn doc_get_notification() {}

/// Creates a new notification.
#[utoipa::path(
    post,
    path = "/api/v1/notifications",
    tag = "Notifications",
    operation_id = "createNotification",
    security(
        ("bearer_auth" = [])
    ),
    request_body = NotificationCreateRequest,
    responses(
        (
            status = 201,
            description = "Notification created successfully",
            body = ApiResponse<NotificationResponse>
        ),
        (
            status = 400,
            description = "Bad Request",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Bad Request",
                "data": null
            })
        ),
        (
            status = 401,
            description = "Unauthorized",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Unauthorized",
                "data": null
            })
        ),
        (
            status = 409,
            description = "Notification with this ID already exists",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Notification with this ID already exists",
                "data": null
            })
        ),
        (
            status = 500,
            description = "Internal Server Error",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Internal Server Error",
                "data": null
            })
        )
    )
)]
#[allow(dead_code)]
fn doc_create_notification() {}

/// Updates an existing notification.
#[utoipa::path(
    patch,
    path = "/api/v1/notifications/{notification_id}",
    tag = "Notifications",
    operation_id = "updateNotification",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("notification_id" = String, Path, description = "Notification ID")
    ),
    request_body = NotificationUpdateRequest,
    responses(
        (
            status = 200,
            description = "Notification updated successfully",
            body = ApiResponse<NotificationResponse>
        ),
        (
            status = 400,
            description = "Bad Request",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Bad Request",
                "data": null
            })
        ),
        (
            status = 401,
            description = "Unauthorized",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Unauthorized",
                "data": null
            })
        ),
        (
            status = 404,
            description = "Notification not found",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Notification not found",
                "data": null
            })
        ),
        (
            status = 500,
            description = "Internal Server Error",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Internal Server Error",
                "data": null
            })
        )
    )
)]
#[allow(dead_code)]
fn doc_update_notification() {}

/// Deletes a notification by ID.
#[utoipa::path(
    delete,
    path = "/api/v1/notifications/{notification_id}",
    tag = "Notifications",
    operation_id = "deleteNotification",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("notification_id" = String, Path, description = "Notification ID")
    ),
    responses(
        (
            status = 200,
            description = "Notification deleted successfully",
            body = ApiResponse<String>,
            example = json!({
                "success": true,
                "message": "Notification deleted successfully",
                "data": "Notification deleted successfully"
            })
        ),
        (
            status = 400,
            description = "Bad Request",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Bad Request",
                "data": null
            })
        ),
        (
            status = 401,
            description = "Unauthorized",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Unauthorized",
                "data": null
            })
        ),
        (
            status = 404,
            description = "Notification not found",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Notification not found",
                "data": null
            })
        ),
        (
            status = 500,
            description = "Internal Server Error",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Internal Server Error",
                "data": null
            })
        )
    )
)]
#[allow(dead_code)]
fn doc_delete_notification() {}
