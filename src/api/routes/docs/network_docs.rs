//! # Network Documentation
//!
//! This module contains the OpenAPI documentation for the network API endpoints.
//!
//! ## Endpoints
//!
//! - `GET /api/v1/networks`: List all networks
//! - `GET /api/v1/networks/{network_id}`: Get a network by ID (e.g., evm:sepolia)
//! - `PATCH /api/v1/networks/{network_id}`: Update a network configuration

use crate::models::{ApiResponse, NetworkResponse, UpdateNetworkRequest};

/// Network routes implementation
///
/// Note: OpenAPI documentation for these endpoints can be found in the `openapi.rs` file
///
/// Lists all networks with pagination support.
#[utoipa::path(
    get,
    path = "/api/v1/networks",
    tag = "Networks",
    operation_id = "listNetworks",
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
            description = "Network list retrieved successfully",
            body = ApiResponse<Vec<NetworkResponse>>
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
fn doc_list_networks() {}

/// Retrieves details of a specific network by ID.
#[utoipa::path(
    get,
    path = "/api/v1/networks/{network_id}",
    tag = "Networks",
    operation_id = "getNetwork",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("network_id" = String, Path, description = "Network ID (e.g., evm:sepolia, solana:mainnet)")
    ),
    responses(
        (
            status = 200,
            description = "Network retrieved successfully",
            body = ApiResponse<NetworkResponse>
        ),
        (
            status = 400,
            description = "Bad Request - invalid network type",
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
                "message": "Network with ID 'evm:sepolia' not found",
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
fn doc_get_network() {}

/// Updates a network's configuration.
/// Currently supports updating RPC URLs only. Can be extended to support other fields.
#[utoipa::path(
    patch,
    path = "/api/v1/networks/{network_id}",
    tag = "Networks",
    operation_id = "updateNetwork",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("network_id" = String, Path, description = "Network ID (e.g., evm:sepolia, solana:mainnet)")
    ),
    request_body = UpdateNetworkRequest,
    responses(
        (
            status = 200,
            description = "Network updated successfully",
            body = ApiResponse<NetworkResponse>
        ),
        (
            status = 400,
            description = "Bad Request - invalid network type or request data",
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
                "message": "Network with ID 'evm:sepolia' not found",
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
fn doc_update_network() {}
