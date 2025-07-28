//! This module defines the HTTP routes for relayer operations.
//! It includes handlers for listing, retrieving, updating, and managing relayer transactions.
//! The routes are integrated with the Actix-web framework and interact with the relayer controller.
use crate::{
    api::controllers::relayer,
    domain::{SignDataRequest, SignTypedDataRequest},
    models::{CreateRelayerRequest, DefaultAppState, PaginationQuery},
};
use actix_web::{delete, get, patch, post, put, web, Responder};
use serde::Deserialize;
use utoipa::ToSchema;

/// Lists all relayers with pagination support.
#[get("/relayers")]
async fn list_relayers(
    query: web::Query<PaginationQuery>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    relayer::list_relayers(query.into_inner(), data).await
}

/// Retrieves details of a specific relayer by ID.
#[get("/relayers/{relayer_id}")]
async fn get_relayer(
    relayer_id: web::Path<String>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    relayer::get_relayer(relayer_id.into_inner(), data).await
}

/// Creates a new relayer.
#[post("/relayers")]
async fn create_relayer(
    request: web::Json<CreateRelayerRequest>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    relayer::create_relayer(request.into_inner(), data).await
}

/// Updates a relayer's information using JSON Merge Patch (RFC 7396).
#[patch("/relayers/{relayer_id}")]
async fn update_relayer(
    relayer_id: web::Path<String>,
    patch: web::Json<serde_json::Value>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    relayer::update_relayer(relayer_id.into_inner(), patch.into_inner(), data).await
}

/// Deletes a relayer by ID.
#[delete("/relayers/{relayer_id}")]
async fn delete_relayer(
    relayer_id: web::Path<String>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    relayer::delete_relayer(relayer_id.into_inner(), data).await
}

/// Fetches the current status of a specific relayer.
#[get("/relayers/{relayer_id}/status")]
async fn get_relayer_status(
    relayer_id: web::Path<String>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    relayer::get_relayer_status(relayer_id.into_inner(), data).await
}

/// Retrieves the balance of a specific relayer.
#[get("/relayers/{relayer_id}/balance")]
async fn get_relayer_balance(
    relayer_id: web::Path<String>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    relayer::get_relayer_balance(relayer_id.into_inner(), data).await
}

/// Sends a transaction through the specified relayer.
#[post("/relayers/{relayer_id}/transactions")]
async fn send_transaction(
    relayer_id: web::Path<String>,
    req: web::Json<serde_json::Value>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    relayer::send_transaction(relayer_id.into_inner(), req.into_inner(), data).await
}

#[derive(Deserialize, ToSchema)]
pub struct TransactionPath {
    relayer_id: String,
    transaction_id: String,
}

/// Retrieves a specific transaction by its ID.
#[get("/relayers/{relayer_id}/transactions/{transaction_id}")]
async fn get_transaction_by_id(
    path: web::Path<TransactionPath>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    let path = path.into_inner();
    relayer::get_transaction_by_id(path.relayer_id, path.transaction_id, data).await
}

/// Retrieves a transaction by its nonce value.
#[get("/relayers/{relayer_id}/transactions/by-nonce/{nonce}")]
async fn get_transaction_by_nonce(
    params: web::Path<(String, u64)>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    let params = params.into_inner();
    relayer::get_transaction_by_nonce(params.0, params.1, data).await
}

/// Lists all transactions for a specific relayer with pagination.
#[get("/relayers/{relayer_id}/transactions")]
async fn list_transactions(
    relayer_id: web::Path<String>,
    query: web::Query<PaginationQuery>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    relayer::list_transactions(relayer_id.into_inner(), query.into_inner(), data).await
}

/// Deletes all pending transactions for a specific relayer.
#[delete("/relayers/{relayer_id}/transactions/pending")]
async fn delete_pending_transactions(
    relayer_id: web::Path<String>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    relayer::delete_pending_transactions(relayer_id.into_inner(), data).await
}

/// Cancels a specific transaction by its ID.
#[delete("/relayers/{relayer_id}/transactions/{transaction_id}")]
async fn cancel_transaction(
    path: web::Path<TransactionPath>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    let path = path.into_inner();
    relayer::cancel_transaction(path.relayer_id, path.transaction_id, data).await
}

/// Replaces a specific transaction with a new one.
#[put("/relayers/{relayer_id}/transactions/{transaction_id}")]
async fn replace_transaction(
    path: web::Path<TransactionPath>,
    req: web::Json<serde_json::Value>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    let path = path.into_inner();
    relayer::replace_transaction(path.relayer_id, path.transaction_id, req.into_inner(), data).await
}

/// Signs data using the specified relayer.
#[post("/relayers/{relayer_id}/sign")]
async fn sign(
    relayer_id: web::Path<String>,
    req: web::Json<SignDataRequest>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    relayer::sign_data(relayer_id.into_inner(), req.into_inner(), data).await
}

/// Signs typed data using the specified relayer.
#[post("/relayers/{relayer_id}/sign-typed-data")]
async fn sign_typed_data(
    relayer_id: web::Path<String>,
    req: web::Json<SignTypedDataRequest>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    relayer::sign_typed_data(relayer_id.into_inner(), req.into_inner(), data).await
}

/// Performs a JSON-RPC call using the specified relayer.
#[post("/relayers/{relayer_id}/rpc")]
async fn rpc(
    relayer_id: web::Path<String>,
    req: web::Json<serde_json::Value>,
    data: web::ThinData<DefaultAppState>,
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
    cfg.service(create_relayer); // /relayers
    cfg.service(update_relayer); // /relayers/{id}
    cfg.service(delete_relayer); // /relayers/{id}
    cfg.service(list_relayers); // /relayers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{EvmNetworkConfig, NetworkConfigCommon},
        jobs::MockJobProducerTrait,
        models::{
            AppState, EvmTransactionData, LocalSignerConfigStorage, NetworkConfigData,
            NetworkRepoModel, NetworkTransactionData, NetworkType, RelayerEvmPolicy,
            RelayerNetworkPolicy, RelayerRepoModel, SignerConfigStorage, SignerRepoModel,
            TransactionRepoModel, TransactionStatus, U256,
        },
        repositories::{
            NetworkRepositoryStorage, NotificationRepositoryStorage, PluginRepositoryStorage,
            RelayerRepositoryStorage, Repository, SignerRepositoryStorage,
            TransactionCounterRepositoryStorage, TransactionRepositoryStorage,
        },
    };
    use actix_web::{http::StatusCode, test, App};
    use std::sync::Arc;

    // Simple mock for AppState
    async fn get_test_app_state() -> AppState<
        MockJobProducerTrait,
        RelayerRepositoryStorage,
        TransactionRepositoryStorage,
        NetworkRepositoryStorage,
        NotificationRepositoryStorage,
        SignerRepositoryStorage,
        TransactionCounterRepositoryStorage,
        PluginRepositoryStorage,
    > {
        let relayer_repo = Arc::new(RelayerRepositoryStorage::new_in_memory());
        let transaction_repo = Arc::new(TransactionRepositoryStorage::new_in_memory());
        let signer_repo = Arc::new(SignerRepositoryStorage::new_in_memory());
        let network_repo = Arc::new(NetworkRepositoryStorage::new_in_memory());

        // Create test entities so routes don't return 404

        // Create test network configuration first
        let test_network = NetworkRepoModel {
            id: "evm:ethereum".to_string(),
            name: "ethereum".to_string(),
            network_type: NetworkType::Evm,
            config: NetworkConfigData::Evm(EvmNetworkConfig {
                common: NetworkConfigCommon {
                    network: "ethereum".to_string(),
                    from: None,
                    rpc_urls: Some(vec!["https://rpc.example.com".to_string()]),
                    explorer_urls: None,
                    average_blocktime_ms: Some(12000),
                    is_testnet: Some(false),
                    tags: None,
                },
                chain_id: Some(1),
                required_confirmations: Some(12),
                features: None,
                symbol: Some("ETH".to_string()),
            }),
        };
        network_repo.create(test_network).await.unwrap();

        // Create local signer first
        let test_signer = SignerRepoModel {
            id: "test-signer".to_string(),
            config: SignerConfigStorage::Local(LocalSignerConfigStorage {
                raw_key: secrets::SecretVec::new(32, |v| v.copy_from_slice(&[0u8; 32])),
            }),
        };
        signer_repo.create(test_signer).await.unwrap();

        // Create test relayer
        let test_relayer = RelayerRepoModel {
            id: "test-id".to_string(),
            name: "Test Relayer".to_string(),
            network: "ethereum".to_string(),
            network_type: NetworkType::Evm,
            signer_id: "test-signer".to_string(),
            address: "0x1234567890123456789012345678901234567890".to_string(),
            paused: false,
            system_disabled: false,
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
            notification_id: None,
            custom_rpc_urls: None,
        };
        relayer_repo.create(test_relayer).await.unwrap();

        // Create test transaction
        let test_transaction = TransactionRepoModel {
            id: "tx-123".to_string(),
            relayer_id: "test-id".to_string(),
            status: TransactionStatus::Pending,
            status_reason: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            sent_at: None,
            confirmed_at: None,
            valid_until: None,
            network_data: NetworkTransactionData::Evm(EvmTransactionData {
                gas_price: Some(20000000000u128),
                gas_limit: Some(21000u64),
                nonce: Some(1u64),
                value: U256::from(0u64),
                data: Some("0x".to_string()),
                from: "0x1234567890123456789012345678901234567890".to_string(),
                to: Some("0x9876543210987654321098765432109876543210".to_string()),
                chain_id: 1u64,
                hash: Some("0xabcdef".to_string()),
                signature: None,
                speed: None,
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                raw: None,
            }),
            priced_at: None,
            hashes: vec!["0xabcdef".to_string()],
            network_type: NetworkType::Evm,
            noop_count: None,
            is_canceled: Some(false),
            delete_at: None,
        };
        transaction_repo.create(test_transaction).await.unwrap();

        AppState {
            relayer_repository: relayer_repo,
            transaction_repository: transaction_repo,
            signer_repository: signer_repo,
            notification_repository: Arc::new(NotificationRepositoryStorage::new_in_memory()),
            network_repository: network_repo,
            transaction_counter_store: Arc::new(
                TransactionCounterRepositoryStorage::new_in_memory(),
            ),
            job_producer: Arc::new(MockJobProducerTrait::new()),
            plugin_repository: Arc::new(PluginRepositoryStorage::new_in_memory()),
        }
    }

    #[actix_web::test]
    async fn test_routes_are_registered() -> Result<(), color_eyre::eyre::Error> {
        // Create a test app with our routes
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(get_test_app_state().await))
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
            .set_json(serde_json::json!({"paused": false}))
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
            .set_json(serde_json::json!({}))
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

        // Test POST /relayers/{id}/rpc
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
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        Ok(())
    }
}
