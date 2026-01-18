use crate::{
    models::{ApiResponse, PluginCallRequest, PluginModel, UpdatePluginRequest},
    repositories::PaginatedResult,
    services::plugins::PluginHandlerError,
};

/// Calls a plugin method.
///
/// Logs and traces are only returned when the plugin is configured with `emit_logs` / `emit_traces`.
/// Plugin-provided errors are normalized into a consistent payload (`code`, `details`) and a derived
/// message so downstream clients receive a stable shape regardless of how the handler threw.
///
/// The endpoint supports wildcard route routing, allowing plugins to implement custom routing logic:
/// - `/api/v1/plugins/{plugin_id}/call` - Default endpoint (route = "")
/// - `/api/v1/plugins/{plugin_id}/call?route=/verify` - Custom route via query parameter
/// - `/api/v1/plugins/{plugin_id}/call/verify` - Custom route via URL path (route = "/verify")
///
/// The route is passed to the plugin handler via the `context.route` field.
/// You can specify a custom route either by appending it to the URL path or by using the `route` query parameter.
#[utoipa::path(
    post,
    path = "/api/v1/plugins/{plugin_id}/call",
    tag = "Plugins",
    operation_id = "callPlugin",
    summary = "Execute a plugin with optional wildcard route routing",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("plugin_id" = String, Path, description = "The unique identifier of the plugin"),
        ("route" = Option<String>, Query, description = "Optional route suffix for custom routing (e.g., '/verify'). Alternative to appending the route to the URL path.")
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

/// Calls a plugin method via GET request.
///
/// This endpoint is disabled by default. To enable it for a given plugin, set
/// `allow_get_invocation: true` in the plugin configuration.
///
/// When invoked via GET:
/// - `params` is an empty object (`{}`)
/// - query parameters are passed to the plugin handler via `context.query`
/// - wildcard route routing is supported the same way as POST (see `doc_call_plugin`)
/// - Use the `route` query parameter or append the route to the URL path
#[utoipa::path(
    get,
    path = "/api/v1/plugins/{plugin_id}/call",
    tag = "Plugins",
    operation_id = "callPluginGet",
    summary = "Execute a plugin via GET (must be enabled per plugin)",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("plugin_id" = String, Path, description = "The unique identifier of the plugin"),
        ("route" = Option<String>, Query, description = "Optional route suffix for custom routing (e.g., '/verify'). Alternative to appending the route to the URL path.")
    ),
    responses(
        (
            status = 200,
            description = "Plugin call successful",
            body = ApiResponse<serde_json::Value>
        ),
        (
            status = 405,
            description = "Method Not Allowed (GET invocation disabled for this plugin)",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "error": "GET requests are not enabled for this plugin. Set 'allow_get_invocation: true' in plugin configuration to enable.",
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
        )
    )
)]
#[allow(dead_code)]
fn doc_call_plugin_get() {}

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

/// Get plugin by ID.
#[utoipa::path(
    get,
    path = "/api/v1/plugins/{plugin_id}",
    tag = "Plugins",
    operation_id = "getPlugin",
    summary = "Get plugin by ID",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("plugin_id" = String, Path, description = "The unique identifier of the plugin")
    ),
    responses(
        (
            status = 200,
            description = "Plugin retrieved successfully",
            body = ApiResponse<PluginModel>,
            example = json!({
                "success": true,
                "data": {
                    "id": "my-plugin",
                    "path": "plugins/my-plugin.ts",
                    "timeout": 30,
                    "emit_logs": false,
                    "emit_traces": false,
                    "raw_response": false,
                    "allow_get_invocation": false,
                    "config": {
                        "featureFlag": true
                    },
                    "forward_logs": false
                },
                "error": null
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
            description = "Plugin not found",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "error": "Plugin with id my-plugin not found",
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
        )
    )
)]
#[allow(dead_code)]
fn doc_get_plugin() {}

/// Update plugin configuration.
///
/// Updates mutable plugin fields such as timeout, emit_logs, emit_traces,
/// raw_response, allow_get_invocation, config, and forward_logs.
/// The plugin id and path cannot be changed after creation.
///
/// All fields are optional - only the provided fields will be updated.
/// To clear the `config` field, pass `"config": null`.
#[utoipa::path(
    patch,
    path = "/api/v1/plugins/{plugin_id}",
    tag = "Plugins",
    operation_id = "updatePlugin",
    summary = "Update plugin configuration",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("plugin_id" = String, Path, description = "The unique identifier of the plugin")
    ),
    request_body(
        content = UpdatePluginRequest,
        description = "Plugin configuration update. All fields are optional.",
        example = json!({
            "timeout": 60,
            "emit_logs": true,
            "forward_logs": true,
            "config": {
                "featureFlag": true,
                "apiKey": "xyz123"
            }
        })
    ),
    responses(
        (
            status = 200,
            description = "Plugin updated successfully",
            body = ApiResponse<PluginModel>,
            example = json!({
                "success": true,
                "data": {
                    "id": "my-plugin",
                    "path": "plugins/my-plugin.ts",
                    "timeout": 60,
                    "emit_logs": true,
                    "emit_traces": false,
                    "raw_response": false,
                    "allow_get_invocation": false,
                    "config": {
                        "featureFlag": true,
                        "apiKey": "xyz123"
                    },
                    "forward_logs": true
                },
                "error": null
            })
        ),
        (
            status = 400,
            description = "Bad Request (invalid timeout or other validation error)",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "error": "Timeout must be greater than 0",
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
            description = "Plugin not found",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "error": "Plugin with id my-plugin not found",
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
        )
    )
)]
#[allow(dead_code)]
fn doc_update_plugin() {}
