use crate::{
    models::{ApiResponse, PluginCallRequest},
    services::plugins::PluginCallResponse,
};

/// Calls a plugin method.
#[utoipa::path(
    post,
    path = "/api/v1/plugins/{plugin_id}/call",
    tag = "Plugins",
    operation_id = "callPlugin",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("plugin_id" = String, Path, description = "The unique identifier of the plugin")
    ),
    request_body = PluginCallRequest,
    responses(
        (
            status = 200,
            description = "Plugin call successful",
            body = ApiResponse<PluginCallResponse>
        ),
        (
            status = 400,
            description = "BadRequest",
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
            description = "Not Found",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Plugin with ID plugin_id not found",
                "data": null
            })
        ),
        (
            status = 429,
            description = "Too Many Requests",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Too Many Requests",
                "data": null
            })
        ),
        (
            status = 500,
            description = "Internal server error",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Internal Server Error",
                "data": null
            })
        ),
    )
)]
#[allow(dead_code)]
fn doc_call_plugin() {}
