// integration tests for the relayer server, main.rs
use actix_web::{dev::Service, test, web, App, HttpResponse};
use std::sync::Arc;

use openzeppelin_relayer::{
    config::{RepositoryStorageType, ServerConfig},
    constants::{AUTHORIZATION_HEADER_NAME, AUTHORIZATION_HEADER_VALUE_PREFIX},
    models::SecretString,
    utils::check_authorization_header,
};

fn test_server_config() -> ServerConfig {
    ServerConfig {
        api_key: SecretString::new("test_key"),
        host: "localhost".to_string(),
        port: 8080,
        metrics_port: 8081,
        redis_url: "redis://localhost:6237".to_string(),
        redis_reader_url: None,
        config_file_path: "./config/config.json".to_string(),
        rate_limit_requests_per_second: 10,
        rate_limit_burst_size: 10,
        enable_swagger: false,
        redis_connection_timeout_ms: 5000,
        redis_key_prefix: "test".to_string(),
        redis_pool_max_size: 10,
        redis_reader_pool_max_size: 10,
        redis_pool_timeout_ms: 5000,
        rpc_timeout_ms: 5000,
        provider_max_retries: 3,
        provider_retry_base_delay_ms: 100,
        provider_retry_max_delay_ms: 2000,
        provider_max_failovers: 3,
        repository_storage_type: RepositoryStorageType::InMemory,
        reset_storage_on_start: false,
        storage_encryption_key: None,
        transaction_expiration_hours: 4.0,
        provider_failure_expiration_secs: 60,
        provider_failure_threshold: 3,
        provider_pause_duration_secs: 60,
        rpc_allowed_hosts: vec![],
        rpc_block_private_ips: false,
        relayer_concurrency_limit: 100,
        max_connections: 256,
        connection_backlog: 511,
        request_timeout_seconds: 30,
        stellar_mainnet_fee_forwarder_address: None,
        stellar_testnet_fee_forwarder_address: None,
        stellar_mainnet_soroswap_router_address: None,
        stellar_testnet_soroswap_router_address: None,
        stellar_mainnet_soroswap_factory_address: None,
        stellar_testnet_soroswap_factory_address: None,
        stellar_mainnet_soroswap_native_wrapper_address: None,
        stellar_testnet_soroswap_native_wrapper_address: None,
    }
}

#[actix_web::test]
async fn test_authorization_middleware_success() {
    let config = Arc::new(test_server_config());

    let app = test::init_service(
        App::new()
            .wrap_fn({
                let config = Arc::clone(&config);
                move |req, srv| {
                    if check_authorization_header(&req, &config.api_key) {
                        return srv.call(req);
                    }
                    Box::pin(async move {
                        Ok(req.into_response(
                            HttpResponse::Unauthorized().body(
                                r#"{"success": false, "code":401, "error": "Unauthorized", "message": "Unauthorized"}"#.to_string(),
                            ),
                        ))
                    })
                }
            })
            .service(web::resource("/test").to(|| async { HttpResponse::Ok().body("Success") })),
    )
    .await;

    let req = test::TestRequest::get()
        .uri("/test")
        .insert_header((
            AUTHORIZATION_HEADER_NAME,
            format!("{}{}", AUTHORIZATION_HEADER_VALUE_PREFIX, "test_key"),
        ))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
}

#[actix_web::test]
async fn test_authorization_middleware_failure() {
    let config = Arc::new(test_server_config());

    let app = test::init_service(
        App::new()
            .wrap_fn({
                let config = Arc::clone(&config);
                move |req, srv| {
                    if check_authorization_header(&req, &config.api_key) {
                        return srv.call(req);
                    }
                    Box::pin(async move {
                        Ok(req.into_response(
                            HttpResponse::Unauthorized().body(
                                r#"{"success": false, "code":401, "error": "Unauthorized", "message": "Unauthorized"}"#.to_string(),
                            ),
                        ))
                    })
                }
            })
            .service(web::resource("/test").to(|| async { HttpResponse::Ok().body("Success") })),
    )
    .await;

    let req = test::TestRequest::get().uri("/test").to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 401);
}
