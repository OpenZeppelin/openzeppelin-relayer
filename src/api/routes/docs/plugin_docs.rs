use crate::{
    models::{ApiResponse, PluginCallRequest, PluginModel},
    repositories::PaginatedResult,
    services::plugins::PluginHandlerError,
};

/// Calls a plugin method.
///
/// Logs and traces are only returned when the plugin is configured with `emit_logs` / `emit_traces`.
/// Plugin-provided errors are normalized into a consistent payload (`code`, `details`) and a derived
/// message so downstream clients receive a stable shape regardless of how the handler threw.
#[utoipa::path(
    post,
    path = "/api/v1/plugins/{plugin_id}/call",
    tag = "Plugins",
    operation_id = "callPlugin",
    summary = "Execute a plugin and receive the sanitized result",
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
            body = ApiResponse<serde_json::Value>,
            example = json!({
                "success": true,
                "data": "done!",
                "metadata": {
                    "logs": [
                        {
                            "level": "info",
                            "message": "Plugin started..."
                        }
                    ],
                    "traces": [
                        {
                            "method": "sendTransaction",
                            "relayerId": "sepolia-example",
                            "requestId": "6c1f336f-3030-4f90-bd99-ada190a1235b"
                        }
                    ]
                },
                "error": null
            })
        ),
        (
            status = 400,
            description = "BadRequest (plugin-provided)",
            body = ApiResponse<PluginHandlerError>,
            example = json!({
                "success": false,
                "error": "Validation failed",
                "data": { "code": "VALIDATION_FAILED", "details": { "field": "email" } },
                "metadata": {
                    "logs": [
                        {
                            "level": "error",
                            "message": "Validation failed for field: email"
                        }
                    ]
                }
            })
        ),
        (
            status = 401,
            description = "Unauthorized",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "error": "Unauthorized",
                "data": null
            })
        ),
        (
            status = 404,
            description = "Not Found",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "error": "Plugin with ID plugin_id not found",
                "data": null
            })
        ),
        (
            status = 429,
            description = "Too Many Requests",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "error": "Too Many Requests",
                "data": null
            })
        ),
        (
            status = 500,
            description = "Internal server error",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "error": "Internal Server Error",
                "data": null
            })
        ),
    )
)]
#[allow(dead_code)]
fn doc_call_plugin() {}

/// List plugins.
#[utoipa::path(
    get,
    path = "/api/v1/plugins",
    tag = "Plugins",
    operation_id = "listPlugins",
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
            description = "Plugins listed successfully",
            body = ApiResponse<PaginatedResult<PluginModel>>
        ),
        (
            status = 400,
            description = "BadRequest",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "error": "Bad Request",
                "data": null
            })
        ),
        (
            status = 401,
            description = "Unauthorized",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "error": "Unauthorized",
                "data": null
            })
        ),
        (
            status = 404,
            description = "Not Found",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "error": "Plugin with ID plugin_id not found",
                "data": null
            })
        ),
        (
            status = 429,
            description = "Too Many Requests",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "error": "Too Many Requests",
                "data": null
            })
        ),
        (
            status = 500,
            description = "Internal server error",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "error": "Internal Server Error",
                "data": null
            })
        ),
    )
)]
#[allow(dead_code)]
fn doc_list_plugins() {}
