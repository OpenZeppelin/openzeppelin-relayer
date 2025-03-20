//! This module defines the HTTP routes for relayer operations.
//! It includes handlers for listing, retrieving, updating, and managing relayer transactions.
//! The routes are integrated with the Actix-web framework and interact with the relayer controller.
use crate::{
    api::controllers::relayer,
    domain::{
        BalanceResponse, JsonRpcRequest, JsonRpcResponse, RelayerUpdateRequest, SignDataRequest,
        SignDataResponse, SignTypedDataRequest,
    },
    models::{
        ApiResponse, AppState, NetworkRpcRequest, NetworkRpcResult, NetworkTransactionRequest,
        PaginationQuery, RelayerResponse, TransactionResponse,
    },
};
use actix_web::{delete, get, patch, post, put, web, Responder};
use serde::Deserialize;
use utoipa::ToSchema;

/// Lists all relayers with pagination support.
#[utoipa::path(
    get,
    path = "/relayers",
    tag = "Relayers",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("page" = Option<usize>, Query, description = "Page number for pagination (starts at 1)"),
        ("per_page" = Option<usize>, Query, description = "Number of items per page (default: 10)")
    ),
    responses(
        (status = 200, description = "Relayers list", body = ApiResponse<Vec<RelayerResponse>>),
        (status = 401, description = "Unauthorized"),
    )
)]
#[get("/relayers")]
async fn list_relayers(
    query: web::Query<PaginationQuery>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::list_relayers(query.into_inner(), data).await
}

/// Retrieves details of a specific relayer by ID.
#[utoipa::path(
    get,
    path = "/relayers/{relayer_id}",
    tag = "Relayers",
    security(
        ("bearer_auth" = []) 
    ),
    responses(
        (status = 200, description = "success", body = ApiResponse<RelayerResponse>),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "internal server error", body = String),
    )
)]
#[get("/relayers/{relayer_id}")]
async fn get_relayer(
    relayer_id: web::Path<String>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::get_relayer(relayer_id.into_inner(), data).await
}

/// Updates a relayer's information based on the provided update request.
#[utoipa::path(
    patch,
    path = "/relayers/{relayer_id}",
    tag = "Relayers",
    security(
        ("bearer_auth" = [])
    ),
    responses(
        (status = 200, description = "Update relayer", body = ApiResponse<RelayerResponse>),
        (status = 401, description = "Unauthorized"),
    )
)]
#[patch("/relayers/{relayer_id}")]
async fn update_relayer(
    relayer_id: web::Path<String>,
    update_req: web::Json<RelayerUpdateRequest>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::update_relayer(relayer_id.into_inner(), update_req.into_inner(), data).await
}

/// Fetches the current status of a specific relayer.
#[utoipa::path(
    get,
    path = "/relayers/{relayer_id}/status",
    tag = "Relayers",
    security(
        ("bearer_auth" = [])
    ),
    responses(
        (status = 200, description = "Relayer status", body = ApiResponse<bool>),
        (status = 401, description = "Unauthorized"),
    )
)]
#[get("/relayers/{relayer_id}/status")]
async fn get_relayer_status(
    relayer_id: web::Path<String>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::get_relayer_status(relayer_id.into_inner(), data).await
}

/// Retrieves the balance of a specific relayer.
#[utoipa::path(
    get,
    path = "/relayers/{relayer_id}/balance",
    tag = "Relayers",
    security(
        ("bearer_auth" = [])
    ),
    responses(
        (status = 200, description = "Relayer balance", body = ApiResponse<BalanceResponse>),
        (status = 401, description = "Unauthorized"),
    )
)]
#[get("/relayers/{relayer_id}/balance")]
async fn get_relayer_balance(
    relayer_id: web::Path<String>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::get_relayer_balance(relayer_id.into_inner(), data).await
}

/// Sends a transaction through the specified relayer.
#[utoipa::path(
    post,
    path = "/relayers/{relayer_id}/transactions",
    tag = "Relayers",
    security(
        ("bearer_auth" = ["read"])
    ),
    request_body = NetworkTransactionRequest,
    responses(
        (status = 200, description = "Transaction response", body = ApiResponse<TransactionResponse>),
        (status = 401, description = "Unauthorized"),
    )
)]
#[post("/relayers/{relayer_id}/transactions")]
async fn send_transaction(
    relayer_id: web::Path<String>,
    req: web::Json<serde_json::Value>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::send_transaction(relayer_id.into_inner(), req.into_inner(), data).await
}

#[derive(Deserialize, ToSchema)]
pub struct TransactionPath {
    relayer_id: String,
    transaction_id: String,
}

/// Retrieves a specific transaction by its ID.
#[utoipa::path(
    get,
    path = "/relayers/{relayer_id}/transactions/{transaction_id}",
    tag = "Relayers",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The ID of the relayer"),
        ("transaction_id" = String, Path, description = "The ID of the transaction")
    ),
    responses(
        (status = 200, description = "Transaction response", body = ApiResponse<TransactionResponse>),
        (status = 401, description = "Unauthorized"),
    )
)]
#[get("/relayers/{relayer_id}/transactions/{transaction_id}")]
async fn get_transaction_by_id(
    path: web::Path<TransactionPath>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    let path = path.into_inner();
    relayer::get_transaction_by_id(path.relayer_id, path.transaction_id, data).await
}

/// Retrieves a transaction by its nonce value.
#[utoipa::path(
    get,
    path = "/relayers/{relayer_id}/transactions/by-nonce/{nonce}",
    tag = "Relayers",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The ID of the relayer"),
        ("nonce" = usize, Path, description = "The nonce of the transaction")
    ),
    responses(
        (status = 200, description = "Transaction response", body = ApiResponse<TransactionResponse>),
        (status = 401, description = "Unauthorized"),
    )
)]
#[get("/relayers/{relayer_id}/transactions/by-nonce/{nonce}")]
async fn get_transaction_by_nonce(
    params: web::Path<(String, u64)>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    let params = params.into_inner();
    relayer::get_transaction_by_nonce(params.0, params.1, data).await
}

/// Lists all transactions for a specific relayer with pagination.
#[utoipa::path(
    get,
    path = "/relayers/{relayer_id}/transactions/",
    tag = "Relayers",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The ID of the relayer"),
        ("page" = Option<usize>, Query, description = "Page number for pagination (starts at 1)"),
        ("per_page" = Option<usize>, Query, description = "Number of items per page (default: 10)")
    ),
    responses(
        (status = 200, description = "Transaction list response", body = ApiResponse<Vec<TransactionResponse>>),
        (status = 401, description = "Unauthorized"),
    )
)]
#[get("/relayers/{relayer_id}/transactions")]
async fn list_transactions(
    relayer_id: web::Path<String>,
    query: web::Query<PaginationQuery>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::list_transactions(relayer_id.into_inner(), query.into_inner(), data).await
}

/// Deletes all pending transactions for a specific relayer.
#[utoipa::path(
    delete,
    path = "/relayers/{relayer_id}/transactions/pending",
    tag = "Relayers",
    security(
        ("bearer_auth" = [])
    ),
    responses(
        (status = 200, description = "Delete Pending transactions", body = ApiResponse<String>),
        (status = 401, description = "Unauthorized"),
    )
)]
#[delete("/relayers/{relayer_id}/transactions/pending")]
async fn delete_pending_transactions(
    relayer_id: web::Path<String>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::delete_pending_transactions(relayer_id.into_inner(), data).await
}

/// Cancels a specific transaction by its ID.
#[utoipa::path(
    delete,
    path = "/relayers/{relayer_id}/transactions/{transaction_id}",
    tag = "Relayers",
    security(
        ("bearer_auth" = [])
    ),
    responses(
        (status = 200, description = "Cancel transaction", body = ApiResponse<TransactionResponse>),
        (status = 401, description = "Unauthorized"),
    )
)]
#[delete("/relayers/{relayer_id}/transactions/{transaction_id}")]
async fn cancel_transaction(
    relayer_id: web::Path<String>,
    transaction_id: web::Path<String>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::cancel_transaction(relayer_id.into_inner(), transaction_id.into_inner(), data).await
}

/// Replaces a specific transaction with a new one.
#[utoipa::path(
    put,
    path = "/relayers/{relayer_id}/transactions/{transaction_id}",
    tag = "Relayers",
    security(
        ("bearer_auth" = [])
    ),
    responses(
        (status = 200, description = "Cancel transaction", body = ApiResponse<TransactionResponse>),
        (status = 401, description = "Unauthorized"),
    )
)]
#[put("/relayers/{relayer_id}/transactions/{transaction_id}")]
async fn replace_transaction(
    relayer_id: web::Path<String>,
    transaction_id: web::Path<String>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::replace_transaction(relayer_id.into_inner(), transaction_id.into_inner(), data).await
}

/// Signs data using the specified relayer.
#[utoipa::path(
    post,
    path = "/relayers/{relayer_id}/sign",
    tag = "Relayers",
    security(
        ("bearer_auth" = [])
    ),
    responses(
        (status = 200, description = "Sign transaction", body = ApiResponse<SignDataResponse>),
        (status = 401, description = "Unauthorized"),
    )
)]
#[post("/relayers/{relayer_id}/sign")]
async fn sign(
    relayer_id: web::Path<String>,
    req: web::Json<SignDataRequest>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::sign_data(relayer_id.into_inner(), req.into_inner(), data).await
}

/// Signs typed data using the specified relayer.
#[utoipa::path(
    post,
    path = "/relayers/{relayer_id}/sign-typed-data",
    tag = "Relayers",
    security(
        ("bearer_auth" = [])
    ),
    responses(
        (status = 200, description = "Sign transaction", body = ApiResponse<SignDataResponse>),
        (status = 401, description = "Unauthorized"),
    )
)]
#[post("/relayers/{relayer_id}/sign-typed-data")]
async fn sign_typed_data(
    relayer_id: web::Path<String>,
    req: web::Json<SignTypedDataRequest>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::sign_typed_data(relayer_id.into_inner(), req.into_inner(), data).await
}

/// Performs a JSON-RPC call using the specified relayer.
#[utoipa::path(
    post,
    path = "/relayers/{relayer_id}/rpc",
    tag = "Relayers",
    security(
        ("bearer_auth" = [])
    ),
    request_body(content = JsonRpcRequest<NetworkRpcRequest>, description = "JSON-RPC request with method and parameters", content_type = "application/json", example = json!({
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
        (status = 200, description = "RPC", body = JsonRpcResponse<NetworkRpcResult>),
        (status = 401, description = "Unauthorized"),
    )
)]
#[post("/relayers/{relayer_id}/rpc")]
async fn rpc(
    relayer_id: web::Path<String>,
    req: web::Json<JsonRpcRequest<NetworkRpcRequest>>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::relayer_rpc(relayer_id.into_inner(), req.into_inner(), data).await
}

/// Initializes the routes for the relayer module.
pub fn init(cfg: &mut web::ServiceConfig) {
    cfg.service(list_relayers);
    cfg.service(get_relayer);
    cfg.service(get_relayer_balance);
    cfg.service(update_relayer);
    cfg.service(get_transaction_by_nonce);
    cfg.service(get_transaction_by_id);
    cfg.service(send_transaction);
    cfg.service(list_transactions);
    cfg.service(get_relayer_status);
    cfg.service(sign_typed_data);
    cfg.service(sign);
    cfg.service(cancel_transaction);
    cfg.service(delete_pending_transactions);
    cfg.service(rpc);
}
