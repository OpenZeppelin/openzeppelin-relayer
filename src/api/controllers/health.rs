//! Health check controller.
//!
//! This module handles HTTP endpoints for health checks, including basic health
//! and readiness endpoints.

use actix_web::HttpResponse;

use crate::jobs::JobProducerTrait;
use crate::models::ThinDataAppState;
use crate::models::{
    NetworkRepoModel, NotificationRepoModel, RelayerRepoModel, SignerRepoModel,
    TransactionRepoModel,
};
use crate::repositories::{
    ApiKeyRepositoryTrait, NetworkRepository, PluginRepositoryTrait, RelayerRepository, Repository,
    TransactionCounterTrait, TransactionRepository,
};
use crate::services::health::get_readiness;

/// Handles the health check endpoint.
///
/// Returns an `HttpResponse` with a status of `200 OK` and a body of `"OK"`.
pub async fn health() -> Result<HttpResponse, actix_web::Error> {
    Ok(HttpResponse::Ok().body("OK"))
}

/// Handles the readiness check endpoint.
///
/// Returns 200 OK if the service is ready to accept traffic, or 503 Service Unavailable if not.
///
/// Health check results are cached for 10 seconds to prevent excessive load from frequent
/// health checks (e.g., from AWS ECS or Kubernetes).
pub async fn readiness<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    data: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
) -> Result<HttpResponse, actix_web::Error>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
    AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
{
    let response = get_readiness(data).await;

    if response.ready {
        Ok(HttpResponse::Ok().json(response))
    } else {
        Ok(HttpResponse::ServiceUnavailable().json(response))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::health::ComponentStatus;
    use crate::utils::mocks::mockutils::create_mock_app_state;
    use actix_web::body::to_bytes;
    use actix_web::web::ThinData;
    use std::sync::Arc;

    // =========================================================================
    // Health Endpoint Tests
    // =========================================================================

    #[actix_web::test]
    async fn test_health_returns_ok() {
        let result = health().await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status().as_u16(), 200);
    }

    #[actix_web::test]
    async fn test_health_response_body() {
        let result = health().await;

        let response = result.unwrap();
        let body = to_bytes(response.into_body()).await.unwrap();
        assert_eq!(body, "OK");
    }

    #[actix_web::test]
    async fn test_health_is_idempotent() {
        // Health endpoint should always return the same result
        for _ in 0..3 {
            let result = health().await;
            assert!(result.is_ok());
            let response = result.unwrap();
            assert_eq!(response.status().as_u16(), 200);
        }
    }

    // =========================================================================
    // Readiness Endpoint Tests - Unhealthy Path (Queue Unavailable)
    // =========================================================================

    #[actix_web::test]
    async fn test_readiness_returns_503_when_queue_unavailable() {
        let mut app_state = create_mock_app_state(None, None, None, None, None, None).await;

        Arc::get_mut(&mut app_state.job_producer)
            .unwrap()
            .expect_get_queue_backend()
            .return_const(None);

        let result = readiness(ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status().as_u16(), 503);
    }

    #[actix_web::test]
    async fn test_readiness_returns_json_when_unhealthy() {
        let mut app_state = create_mock_app_state(None, None, None, None, None, None).await;

        Arc::get_mut(&mut app_state.job_producer)
            .unwrap()
            .expect_get_queue_backend()
            .return_const(None);

        let result = readiness(ThinData(app_state)).await;
        let response = result.unwrap();
        let body = to_bytes(response.into_body()).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Verify response structure
        assert_eq!(json["ready"], false);
        assert_eq!(json["status"], "unhealthy");
        assert!(json.get("components").is_some());
        assert!(json.get("timestamp").is_some());
    }

    #[actix_web::test]
    async fn test_readiness_includes_reason_when_unhealthy() {
        let mut app_state = create_mock_app_state(None, None, None, None, None, None).await;

        Arc::get_mut(&mut app_state.job_producer)
            .unwrap()
            .expect_get_queue_backend()
            .return_const(None);

        let result = readiness(ThinData(app_state)).await;
        let response = result.unwrap();
        let body = to_bytes(response.into_body()).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // When unhealthy, reason should explain why
        assert!(json.get("reason").is_some());
        let reason = json["reason"].as_str().unwrap();
        assert!(!reason.is_empty());
    }

    #[actix_web::test]
    async fn test_readiness_components_show_unhealthy_state() {
        let mut app_state = create_mock_app_state(None, None, None, None, None, None).await;

        Arc::get_mut(&mut app_state.job_producer)
            .unwrap()
            .expect_get_queue_backend()
            .return_const(None);

        let result = readiness(ThinData(app_state)).await;
        let response = result.unwrap();
        let body = to_bytes(response.into_body()).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Queue should be unhealthy when queue backend is unavailable.
        // Redis is neutral in in-memory repository-backed tests.
        let redis_status = json["components"]["redis"]["status"].as_str().unwrap();
        let queue_status = json["components"]["queue"]["status"].as_str().unwrap();

        assert_eq!(redis_status, "healthy");
        assert_eq!(queue_status, "unhealthy");
    }

    #[actix_web::test]
    async fn test_readiness_system_health_still_checked_when_queue_unavailable() {
        let mut app_state = create_mock_app_state(None, None, None, None, None, None).await;

        Arc::get_mut(&mut app_state.job_producer)
            .unwrap()
            .expect_get_queue_backend()
            .return_const(None);

        let result = readiness(ThinData(app_state)).await;
        let response = result.unwrap();
        let body = to_bytes(response.into_body()).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // System health should still be checked even when queue unavailable
        let system = &json["components"]["system"];
        assert!(system.get("status").is_some());
        assert!(system.get("fd_count").is_some());
        assert!(system.get("fd_limit").is_some());
        assert!(system.get("fd_usage_percent").is_some());
        assert!(system.get("close_wait_count").is_some());

        // System status should be valid (likely healthy since system checks don't depend on queue)
        let system_status = system["status"].as_str().unwrap();
        assert!(
            system_status == "healthy"
                || system_status == "degraded"
                || system_status == "unhealthy"
        );
    }

    // =========================================================================
    // Response Correlation Tests
    // =========================================================================

    #[actix_web::test]
    async fn test_readiness_status_code_matches_ready_field() {
        let mut app_state = create_mock_app_state(None, None, None, None, None, None).await;

        // Test unhealthy path
        Arc::get_mut(&mut app_state.job_producer)
            .unwrap()
            .expect_get_queue_backend()
            .return_const(None);

        let result = readiness(ThinData(app_state)).await;
        let response = result.unwrap();
        let status_code = response.status().as_u16();
        let body = to_bytes(response.into_body()).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        let ready = json["ready"].as_bool().unwrap();

        // Verify correlation: ready=false should give 503
        assert!(!ready);
        assert_eq!(status_code, 503);
    }

    #[actix_web::test]
    async fn test_readiness_timestamp_is_valid_rfc3339() {
        let mut app_state = create_mock_app_state(None, None, None, None, None, None).await;

        Arc::get_mut(&mut app_state.job_producer)
            .unwrap()
            .expect_get_queue_backend()
            .return_const(None);

        let result = readiness(ThinData(app_state)).await;
        let response = result.unwrap();
        let body = to_bytes(response.into_body()).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        let timestamp = json["timestamp"].as_str().unwrap();
        // Should be parseable as RFC3339
        chrono::DateTime::parse_from_rfc3339(timestamp)
            .expect("Timestamp should be valid RFC3339 format");
    }

    // =========================================================================
    // Serialization Tests
    // =========================================================================

    #[test]
    fn test_component_status_serializes_to_lowercase() {
        assert_eq!(
            serde_json::to_string(&ComponentStatus::Healthy).unwrap(),
            "\"healthy\""
        );
        assert_eq!(
            serde_json::to_string(&ComponentStatus::Degraded).unwrap(),
            "\"degraded\""
        );
        assert_eq!(
            serde_json::to_string(&ComponentStatus::Unhealthy).unwrap(),
            "\"unhealthy\""
        );
    }
}
