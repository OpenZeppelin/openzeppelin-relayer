//! # Relayer Documentation
//!
//! This module contains the OpenAPI documentation for the relayer API endpoints.
//!
//! ## Endpoints
//!
//! - `GET /api/v1/relayers`: List all relayers
//! - `GET /api/v1/relayers/{id}`: Get a relayer by ID
//! - `POST /api/v1/relayers`: Create a new relayer
//! - `PATCH /api/v1/relayers/{id}`: Update a relayer
//! - `DELETE /api/v1/relayers/{id}`: Delete a relayer

use crate::{
    domain::{
        BalanceResponse, SignDataRequest, SignDataResponse, SignTransactionExternalResponse,
        SignTransactionRequest, SignTypedDataRequest,
    },
    models::{
        ApiResponse, CreateRelayerRequest, DeletePendingTransactionsResponse, JsonRpcRequest,
        JsonRpcResponse, NetworkRpcRequest, NetworkRpcResult, NetworkTransactionRequest,
        RelayerResponse, RelayerStatus, TransactionResponse, UpdateRelayerRequest,
    },
};

/// Relayer routes implementation
///
/// Note: OpenAPI documentation for these endpoints can be found in the `openapi.rs` file
///
/// Lists all relayers with pagination support.
#[utoipa::path(
    get,
    path = "/api/v1/relayers",
    tag = "Relayers",
    operation_id = "listRelayers",
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
            description = "Relayer list retrieved successfully",
            body = ApiResponse<Vec<RelayerResponse>>
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
fn doc_list_relayers() {}

/// Retrieves details of a specific relayer by ID.
#[utoipa::path(
    get,
    path = "/api/v1/relayers/{relayer_id}",
    tag = "Relayers",
    operation_id = "getRelayer",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The unique identifier of the relayer")
    ),
    responses(
        (
            status = 200,
            description = "Relayer details retrieved successfully",
            body = ApiResponse<RelayerResponse>
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
                "message": "Relayer with ID relayer_id not found",
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
fn doc_get_relayer() {}

/// Creates a new relayer.
#[utoipa::path(
    post,
    path = "/api/v1/relayers",
    tag = "Relayers",
    operation_id = "createRelayer",
    security(
        ("bearer_auth" = [])
    ),
    request_body = CreateRelayerRequest,
    responses(
        (
            status = 201,
            description = "Relayer created successfully",
            body = ApiResponse<RelayerResponse>
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
            description = "Relayer with this ID already exists",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Relayer with this ID already exists",
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
fn doc_create_relayer() {}

/// Updates a relayer's information based on the provided update request.
#[utoipa::path(
    patch,
    path = "/api/v1/relayers/{relayer_id}",
    tag = "Relayers",
    operation_id = "updateRelayer",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The unique identifier of the relayer")
    ),
    request_body = UpdateRelayerRequest,
    responses(
        (status = 200, description = "Relayer updated successfully", body = ApiResponse<RelayerResponse>),
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
                "message": "Relayer with ID relayer_id not found",
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
fn doc_update_relayer() {}

/// Deletes a relayer by ID.
#[utoipa::path(
    delete,
    path = "/api/v1/relayers/{relayer_id}",
    tag = "Relayers",
    operation_id = "deleteRelayer",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The unique identifier of the relayer")
    ),
    responses(
        (
            status = 200,
            description = "Relayer deleted successfully",
            body = ApiResponse<String>
        ),
        (
            status = 400,
            description = "Bad Request - Cannot delete relayer with active transactions",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Cannot delete relayer 'relayer_id' because it has N transaction(s). Please wait for all transactions to complete or cancel them before deleting the relayer.",
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
                "message": "Relayer with ID relayer_id not found",
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
fn doc_delete_relayer() {}

/// Fetches the current status of a specific relayer.
#[utoipa::path(
    get,
    path = "/api/v1/relayers/{relayer_id}/status",
    tag = "Relayers",
    operation_id = "getRelayerStatus",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The unique identifier of the relayer")
    ),
    responses(
        (status = 200, description = "Relayer status retrieved successfully", body = ApiResponse<RelayerStatus>),
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
                "message": "Relayer with ID relayer_id not found",
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
fn doc_get_relayer_status() {}

/// Retrieves the balance of a specific relayer.
#[utoipa::path(
    get,
    path = "/api/v1/relayers/{relayer_id}/balance",
    tag = "Relayers",
    operation_id = "getRelayerBalance",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The unique identifier of the relayer")
    ),
    responses(
        (status = 200, description = "Relayer balance retrieved successfully", body = ApiResponse<BalanceResponse>),
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
                "message": "Relayer with ID relayer_id not found",
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
fn doc_get_relayer_balance() {}

/// Sends a transaction through the specified relayer.
#[utoipa::path(
    post,
    path = "/api/v1/relayers/{relayer_id}/transactions",
    tag = "Relayers",
    operation_id = "sendTransaction",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The unique identifier of the relayer")
    ),
    request_body = NetworkTransactionRequest,
    responses(
        (status = 200, description = "Relayer transactions sent successfully", body = ApiResponse<TransactionResponse>),
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
                "message": "Relayer with ID relayer_id not found",
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
fn doc_send_transaction() {}

/// Retrieves a specific transaction by its ID.
#[utoipa::path(
    get,
    path = "/api/v1/relayers/{relayer_id}/transactions/{transaction_id}",
    operation_id = "getTransactionById",
    tag = "Relayers",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The unique identifier of the relayer"),
        ("transaction_id" = String, Path, description = "The unique identifier of the transaction")
    ),
    responses(
        (status = 200, description = "Relayer transaction retrieved successfully", body = ApiResponse<TransactionResponse>),
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
            status = 404,
            description = "Not Found",
            body = ApiResponse<String>,
            example = json!({
                "success": false,
                "message": "Not Found",
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
fn doc_get_transaction_by_id() {}

/// Retrieves a transaction by its nonce value.
#[utoipa::path(
    get,
    path = "/api/v1/relayers/{relayer_id}/transactions/by-nonce/{nonce}",
    tag = "Relayers",
    operation_id = "getTransactionByNonce",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The unique identifier of the relayer"),
        ("nonce" = usize, Path, description = "The nonce of the transaction")
    ),
    responses(
        (status = 200, description = "Relayer transaction retrieved successfully", body = ApiResponse<TransactionResponse>),
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
                "message": "Not found",
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
fn doc_get_transaction_by_nonce() {}

/// Lists all transactions for a specific relayer with pagination.
#[utoipa::path(
    get,
    path = "/api/v1/relayers/{relayer_id}/transactions/",
    tag = "Relayers",
    operation_id = "listTransactions",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The unique identifier of the relayer"),
        ("page" = Option<usize>, Query, description = "Page number for pagination (starts at 1)"),
        ("per_page" = Option<usize>, Query, description = "Number of items per page (default: 10)")
    ),
    responses(
        (status = 200, description = "Relayer transactions retrieved successfully", body = ApiResponse<Vec<TransactionResponse>>),
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
                "message": "Relayer with ID relayer_id not found",
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
fn doc_list_transactions() {}

/// Deletes all pending transactions for a specific relayer.
#[utoipa::path(
    delete,
    path = "/api/v1/relayers/{relayer_id}/transactions/pending",
    tag = "Relayers",
    operation_id = "deletePendingTransactions",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The unique identifier of the relayer")
    ),
    responses(
        (status = 200, description = "Relayer pending transactions successfully", body = ApiResponse<DeletePendingTransactionsResponse>),
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
                "message": "Relayer with ID relayer_id not found",
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
fn doc_delete_pending_transactions() {}

/// Cancels a specific transaction by its ID.
#[utoipa::path(
    delete,
    path = "/api/v1/relayers/{relayer_id}/transactions/{transaction_id}",
    tag = "Relayers",
    operation_id = "cancelTransaction",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The unique identifier of the relayer"),
        ("transaction_id" = String, Path, description = "The unique identifier of the transaction")
    ),
    responses(
        (status = 200, description = "Relayer transaction canceled successfully", body = ApiResponse<TransactionResponse>),
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
                "message": "Not found",
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
fn doc_cancel_transaction() {}

/// Replaces a specific transaction with a new one.
#[utoipa::path(
    put,
    path = "/api/v1/relayers/{relayer_id}/transactions/{transaction_id}",
    tag = "Relayers",
    operation_id = "replaceTransaction",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The unique identifier of the relayer"),
        ("transaction_id" = String, Path, description = "The unique identifier of the transaction")
    ),
    request_body = NetworkTransactionRequest,
    responses(
        (status = 200, description = "Relayer transaction replaced successfully", body = ApiResponse<TransactionResponse>),
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
                "message": "Not found",
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
fn doc_replace_transaction() {}

/// Signs data using the specified relayer.
#[utoipa::path(
    post,
    path = "/api/v1/relayers/{relayer_id}/sign",
    operation_id = "sign",
    tag = "Relayers",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The unique identifier of the relayer"),
    ),
    request_body = SignDataRequest,
    responses(
        (status = 200, description = "Relayer signed data successfully", body = ApiResponse<SignDataResponse>),
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
                "message": "Not found",
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
fn doc_sign() {}

/// Signs typed data using the specified relayer.
#[utoipa::path(
    post,
    path = "/api/v1/relayers/{relayer_id}/sign-typed-data",
    tag = "Relayers",
    operation_id = "signTypedData",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The unique identifier of the relayer"),
    ),
    request_body = SignTypedDataRequest,
    responses(
        (status = 200, description = "Relayer signed typed data successfully", body = ApiResponse<SignDataResponse>),
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
                "message": "Relayer with ID relayer_id not found",
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
fn doc_sign_typed_data() {}

/// Signs a transaction using the specified relayer (Stellar only).
#[utoipa::path(
    post,
    path = "/api/v1/relayers/{relayer_id}/sign-transaction",
    tag = "Relayers",
    operation_id = "signTransaction",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The unique identifier of the relayer"),
    ),
    request_body = SignTransactionRequest,
    responses(
        (status = 200, description = "Transaction signed successfully", body = ApiResponse<SignTransactionExternalResponse>),
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
                "message": "Relayer with ID relayer_id not found",
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
fn doc_sign_transaction() {}

/// Performs a JSON-RPC call using the specified relayer.
#[utoipa::path(
    post,
    path = "/api/v1/relayers/{relayer_id}/rpc",
    tag = "Relayers",
    operation_id = "rpc",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The unique identifier of the relayer"),
    ),
    request_body(content = JsonRpcRequest<NetworkRpcRequest>,
        description = "JSON-RPC request with method and parameters", content_type = "application/json", example = json!({
        "jsonrpc": "2.0",
        "method": "feeEstimate",
        "params": {
            "network": "solana",
            "transaction": "base64_encoded_transaction",
            "fee_token": "SOL"
        },
        "id": 1
    })),
    responses(
        (status = 200, description = "RPC method executed successfully", body = JsonRpcResponse<NetworkRpcResult>),
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
                "message": "Relayer with ID relayer_id not found",
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
fn doc_rpc() {}
