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
    path = "/api/v1/relayers",
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
    path = "/api/v1/relayers/{relayer_id}",
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
    path = "/api/v1/relayers/{relayer_id}",
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
    path = "/api/v1/relayers/{relayer_id}/status",
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
    path = "/api/v1/relayers/{relayer_id}/balance",
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
    path = "/api/v1/relayers/{relayer_id}/transactions",
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
    path = "/api/v1/relayers/{relayer_id}/transactions/{transaction_id}",
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
    path = "/api/v1/relayers/{relayer_id}/transactions/by-nonce/{nonce}",
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
    path = "/api/v1/relayers/{relayer_id}/transactions/",
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
    path = "/api/v1/relayers/{relayer_id}/transactions/pending",
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
    path = "/api/v1/relayers/{relayer_id}/transactions/{transaction_id}",
    tag = "Relayers",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The ID of the relayer"),
        ("transaction_id" = String, Path, description = "The ID of the transaction")
    ),
    responses(
        (status = 200, description = "Cancel transaction", body = ApiResponse<TransactionResponse>),
        (status = 401, description = "Unauthorized"),
    )
)]
#[delete("/relayers/{relayer_id}/transactions/{transaction_id}")]
async fn cancel_transaction(
    path: web::Path<TransactionPath>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    let path = path.into_inner();
    relayer::cancel_transaction(path.relayer_id, path.transaction_id, data).await
}

/// Replaces a specific transaction with a new one.
#[utoipa::path(
    put,
    path = "/api/v1/relayers/{relayer_id}/transactions/{transaction_id}",
    tag = "Relayers",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("relayer_id" = String, Path, description = "The ID of the relayer"),
        ("transaction_id" = String, Path, description = "The ID of the transaction")
    ),
    responses(
        (status = 200, description = "Cancel transaction", body = ApiResponse<TransactionResponse>),
        (status = 401, description = "Unauthorized"),
    )
)]
#[put("/relayers/{relayer_id}/transactions/{transaction_id}")]
async fn replace_transaction(
    path: web::Path<TransactionPath>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    let path = path.into_inner();
    relayer::replace_transaction(path.relayer_id, path.transaction_id, data).await
}

/// Signs data using the specified relayer.
#[utoipa::path(
    post,
    path = "/api/v1/relayers/{relayer_id}/sign",
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
    path = "/api/v1/relayers/{relayer_id}/sign-typed-data",
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
    path = "/api/v1/relayers/{relayer_id}/rpc",
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
    // Register routes with literal segments before routes with path parameters
    cfg.service(delete_pending_transactions); // /relayers/{id}/transactions/pending

    // Then register other routes
    cfg.service(cancel_transaction); // /relayers/{id}/transactions/{tx_id}
    cfg.service(replace_transaction); // /relayers/{id}/transactions/{tx_id}
    cfg.service(get_transaction_by_id); // /relayers/{id}/transactions/{tx_id}
    cfg.service(get_transaction_by_nonce); // /relayers/{id}/transactions/by-nonce/{nonce}
    cfg.service(send_transaction); // /relayers/{id}/transactions
    cfg.service(list_transactions); // /relayers/{id}/transactions
    cfg.service(get_relayer_status); // /relayers/{id}/status
    cfg.service(get_relayer_balance); // /relayers/{id}/balance
    cfg.service(sign); // /relayers/{id}/sign
    cfg.service(sign_typed_data); // /relayers/{id}/sign-typed-data
    cfg.service(rpc); // /relayers/{id}/rpc
    cfg.service(get_relayer); // /relayers/{id}
    cfg.service(update_relayer); // /relayers/{id}
    cfg.service(list_relayers); // /relayers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        jobs::{JobProducer, Queue},
        repositories::{
            InMemoryNotificationRepository, InMemoryRelayerRepository, InMemorySignerRepository,
            InMemoryTransactionCounter, InMemoryTransactionRepository, RelayerRepositoryStorage,
        },
    };
    use actix_web::{http::StatusCode, test, App};
    use std::sync::Arc;

    // Simple mock for AppState
    async fn get_test_app_state() -> AppState {
        AppState {
            relayer_repository: Arc::new(RelayerRepositoryStorage::in_memory(
                InMemoryRelayerRepository::new(),
            )),
            transaction_repository: Arc::new(InMemoryTransactionRepository::new()),
            signer_repository: Arc::new(InMemorySignerRepository::new()),
            notification_repository: Arc::new(InMemoryNotificationRepository::new()),
            transaction_counter_store: Arc::new(InMemoryTransactionCounter::new()),
            job_producer: Arc::new(JobProducer::new(Queue::setup().await)),
        }
    }

    #[actix_web::test]
    async fn test_routes_are_registered() {
        // Create a test app with our routes
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(get_test_app_state()))
                .configure(init),
        )
        .await;

        // Test that routes are registered by checking they return 500 (not 404)

        // Test GET /relayers
        let req = test::TestRequest::get().uri("/relayers").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // Test GET /relayers/{id}
        let req = test::TestRequest::get()
            .uri("/relayers/test-id")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // Test PATCH /relayers/{id}
        let req = test::TestRequest::patch()
            .uri("/relayers/test-id")
            .set_json(serde_json::json!({}))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // Test GET /relayers/{id}/status
        let req = test::TestRequest::get()
            .uri("/relayers/test-id/status")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // Test GET /relayers/{id}/balance
        let req = test::TestRequest::get()
            .uri("/relayers/test-id/balance")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // Test POST /relayers/{id}/transactions
        let req = test::TestRequest::post()
            .uri("/relayers/test-id/transactions")
            .set_json(serde_json::json!({}))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // Test GET /relayers/{id}/transactions/{tx_id}
        let req = test::TestRequest::get()
            .uri("/relayers/test-id/transactions/tx-123")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // Test GET /relayers/{id}/transactions/by-nonce/{nonce}
        let req = test::TestRequest::get()
            .uri("/relayers/test-id/transactions/by-nonce/123")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // Test GET /relayers/{id}/transactions
        let req = test::TestRequest::get()
            .uri("/relayers/test-id/transactions")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // Test DELETE /relayers/{id}/transactions/pending
        let req = test::TestRequest::delete()
            .uri("/relayers/test-id/transactions/pending")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // Test DELETE /relayers/{id}/transactions/{tx_id}
        let req = test::TestRequest::delete()
            .uri("/relayers/test-id/transactions/tx-123")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // Test PUT /relayers/{id}/transactions/{tx_id}
        let req = test::TestRequest::put()
            .uri("/relayers/test-id/transactions/tx-123")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // Test POST /relayers/{id}/sign
        let req = test::TestRequest::post()
            .uri("/relayers/test-id/sign")
            .set_json(serde_json::json!({
                "message": "0x1234567890abcdef"
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // Test POST /relayers/{id}/sign-typed-data
        let req = test::TestRequest::post()
            .uri("/relayers/test-id/sign-typed-data")
            .set_json(serde_json::json!({
                "domain_separator": "0x1234567890abcdef",
                "hash_struct_message": "0x1234567890abcdef"
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // // Test POST /relayers/{id}/rpc
        let req = test::TestRequest::post()
            .uri("/relayers/test-id/rpc")
            .set_json(serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_getBlockByNumber",
                "params": ["0x1", true],
                "id": 1
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
