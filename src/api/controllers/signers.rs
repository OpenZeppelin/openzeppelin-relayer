//! # Signers Controller
//!
//! Handles HTTP endpoints for signer operations including:
//! - Listing signers
//! - Getting signer details
//! - Creating signers  
//! - Updating signers
//! - Deleting signers

use crate::{
    jobs::JobProducerTrait,
    models::{
        ApiError, ApiResponse, NetworkRepoModel, NotificationRepoModel, PaginationMeta,
        PaginationQuery, RelayerRepoModel, Signer, SignerCreateRequest, SignerRepoModel,
        SignerResponse, SignerUpdateRequest, ThinDataAppState, TransactionRepoModel,
    },
    repositories::{
        NetworkRepository, PluginRepositoryTrait, RelayerRepository, Repository,
        TransactionCounterTrait, TransactionRepository,
    },
};
use actix_web::HttpResponse;
use eyre::Result;

/// Lists all signers with pagination support.
///
/// # Arguments
///
/// * `query` - The pagination query parameters.
/// * `state` - The application state containing the signer repository.
///
/// # Returns
///
/// A paginated list of signers.
pub async fn list_signers<J, RR, TR, NR, NFR, SR, TCR, PR>(
    query: PaginationQuery,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
    let signers = state.signer_repository.list_paginated(query).await?;

    let mapped_signers: Vec<SignerResponse> = signers.items.into_iter().map(|s| s.into()).collect();

    Ok(HttpResponse::Ok().json(ApiResponse::paginated(
        mapped_signers,
        PaginationMeta {
            total_items: signers.total,
            current_page: signers.page,
            per_page: signers.per_page,
        },
    )))
}

/// Retrieves details of a specific signer by ID.
///
/// # Arguments
///
/// * `signer_id` - The ID of the signer to retrieve.
/// * `state` - The application state containing the signer repository.
///
/// # Returns
///
/// The signer details or an error if not found.
pub async fn get_signer<J, RR, TR, NR, NFR, SR, TCR, PR>(
    signer_id: String,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
    let signer = state.signer_repository.get_by_id(signer_id).await?;

    let response = SignerResponse::from(signer);
    Ok(HttpResponse::Ok().json(ApiResponse::success(response)))
}

/// Creates a new signer.
///
/// # Arguments
///
/// * `request` - The signer creation request.
/// * `state` - The application state containing the signer repository.
///
/// # Returns
///
/// The created signer or an error if creation fails.
///
/// # Note
///
/// This endpoint only creates the signer metadata. The actual signer configuration
/// (keys, credentials, etc.) should be provided through configuration files or
/// other secure channels. This is a security measure to prevent sensitive data
/// from being transmitted through API requests.
pub async fn create_signer<J, RR, TR, NR, NFR, SR, TCR, PR>(
    request: SignerCreateRequest,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
    // Convert request to domain model (validates automatically and includes placeholder config)
    let signer = Signer::try_from(request)?;

    // Convert domain model to repository model
    let signer_model = SignerRepoModel::from(signer);

    let created_signer = state.signer_repository.create(signer_model).await?;

    let response = SignerResponse::from(created_signer);
    Ok(HttpResponse::Created().json(ApiResponse::success(response)))
}

/// Updates an existing signer.
///
/// # Arguments
///
/// * `signer_id` - The ID of the signer to update.
/// * `request` - The signer update request.
/// * `state` - The application state containing the signer repository.
///
/// # Returns
///
/// The updated signer or an error if update fails.
///
/// # Note
///
/// Only metadata fields (name, description) can be updated through this endpoint.
/// Signer configuration changes require secure configuration channels for security reasons.
pub async fn update_signer<J, RR, TR, NR, NFR, SR, TCR, PR>(
    signer_id: String,
    request: SignerUpdateRequest,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
    // Get the existing signer from repository
    let existing_repo_model = state.signer_repository.get_by_id(signer_id.clone()).await?;

    // Convert to domain model and apply update (only metadata changes)
    let existing_signer = Signer::from(existing_repo_model.clone());
    let updated_signer = existing_signer.apply_update(&request)?;

    // Create updated repository model (preserving original config)
    let updated_repo_model = SignerRepoModel {
        id: updated_signer.id,
        config: existing_repo_model.config, // Keep original config unchanged
    };

    let saved_signer = state
        .signer_repository
        .update(signer_id, updated_repo_model)
        .await?;

    let response = SignerResponse::from(saved_signer);
    Ok(HttpResponse::Ok().json(ApiResponse::success(response)))
}

/// Deletes a signer by ID.
///
/// # Arguments
///
/// * `signer_id` - The ID of the signer to delete.
/// * `state` - The application state containing the signer repository.
///
/// # Returns
///
/// A success response or an error if deletion fails.
///
/// # Warning
///
/// Deleting a signer will prevent any relayers or services that depend on it
/// from functioning properly. Ensure the signer is not in use before deletion.
pub async fn delete_signer<J, RR, TR, NR, NFR, SR, TCR, PR>(
    signer_id: String,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
    state.signer_repository.delete_by_id(signer_id).await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success("Signer deleted successfully")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::{LocalSignerConfig, SignerConfig, SignerType},
        utils::mocks::mockutils::create_mock_app_state,
    };
    use secrets::SecretVec;

    /// Helper function to create a test signer model
    fn create_test_signer_model(id: &str, signer_type: SignerType) -> SignerRepoModel {
        let config = match signer_type {
            SignerType::Local => SignerConfig::Local(LocalSignerConfig {
                raw_key: SecretVec::new(32, |v| v.copy_from_slice(&[1; 32])),
            }),
            SignerType::AwsKms => SignerConfig::AwsKms(crate::models::AwsKmsSignerConfig {
                region: Some("us-east-1".to_string()),
                key_id: "test-key-id".to_string(),
            }),
            _ => SignerConfig::Local(LocalSignerConfig {
                raw_key: SecretVec::new(32, |v| v.copy_from_slice(&[1; 32])),
            }),
        };

        SignerRepoModel {
            id: id.to_string(),
            config,
        }
    }

    /// Helper function to create a test signer create request
    fn create_test_signer_create_request(
        id: Option<String>,
        signer_type: SignerType,
    ) -> SignerCreateRequest {
        use crate::models::{AwsKmsSignerRequestConfig, PlainSignerRequestConfig, SignerConfigRequest};

        let config = match signer_type {
            SignerType::Local => SignerConfigRequest::Local { 
                config: PlainSignerRequestConfig { key: "placeholder-key".to_string() }
            },
            SignerType::AwsKms => SignerConfigRequest::AwsKms {
                config: AwsKmsSignerRequestConfig {
                    region: "us-east-1".to_string(),
                    key_id: "test-key-id".to_string(),
                },
            },
            _ => SignerConfigRequest::Local { 
                config: PlainSignerRequestConfig { key: "placeholder-key".to_string() }
            }, // Use Local for other types in helper
        };

        SignerCreateRequest {
            id,
            config,
            name: Some("Test Signer".to_string()),
            description: Some("A test signer for development".to_string()),
        }
    }

    /// Helper function to create a test signer update request
    fn create_test_signer_update_request() -> SignerUpdateRequest {
        SignerUpdateRequest {
            name: Some("Updated Signer Name".to_string()),
            description: Some("Updated signer description".to_string()),
        }
    }

    #[actix_web::test]
    async fn test_list_signers_empty() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };

        let result = list_signers(query, actix_web::web::ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        let api_response: ApiResponse<Vec<SignerResponse>> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.len(), 0);
    }

    #[actix_web::test]
    async fn test_list_signers_with_data() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        // Create test signers
        let signer1 = create_test_signer_model("test-1", SignerType::Local);
        let signer2 = create_test_signer_model("test-2", SignerType::AwsKms);

        app_state.signer_repository.create(signer1).await.unwrap();
        app_state.signer_repository.create(signer2).await.unwrap();

        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };

        let result = list_signers(query, actix_web::web::ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        let api_response: ApiResponse<Vec<SignerResponse>> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.len(), 2);

        // Check that both signers are present (order not guaranteed)
        let ids: Vec<&String> = data.iter().map(|s| &s.id).collect();
        assert!(ids.contains(&&"test-1".to_string()));
        assert!(ids.contains(&&"test-2".to_string()));
    }

    #[actix_web::test]
    async fn test_get_signer_success() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        // Create a test signer
        let signer = create_test_signer_model("test-signer", SignerType::Local);
        app_state
            .signer_repository
            .create(signer.clone())
            .await
            .unwrap();

        let result = get_signer(
            "test-signer".to_string(),
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        let api_response: ApiResponse<SignerResponse> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.id, "test-signer");
        assert_eq!(data.r#type, SignerType::Local);
    }

    #[actix_web::test]
    async fn test_get_signer_not_found() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let result = get_signer(
            "non-existent".to_string(),
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(matches!(error, ApiError::NotFound(_)));
    }

    #[actix_web::test]
    async fn test_create_signer_test_type_success() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let request = create_test_signer_create_request(
            Some("new-test-signer".to_string()),
            SignerType::Local,
        );

        let result = create_signer(request, actix_web::web::ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 201);

        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        let api_response: ApiResponse<SignerResponse> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.id, "new-test-signer");
        assert_eq!(data.r#type, SignerType::Local);
    }

    #[actix_web::test]
    async fn test_create_signer_production_types_require_config() {
        let production_types = vec![
            SignerType::Local,
            SignerType::AwsKms,
            SignerType::GoogleCloudKms,
            SignerType::Vault,
            SignerType::VaultTransit,
            SignerType::Turnkey,
        ];

        for signer_type in production_types {
            let app_state = create_mock_app_state(None, None, None, None, None).await;
            let request =
                create_test_signer_create_request(Some("test".to_string()), signer_type.clone());
            let result = create_signer(request, actix_web::web::ThinData(app_state)).await;

            assert!(
                result.is_err(),
                "Should fail for signer type: {:?}",
                signer_type
            );
            if let Err(ApiError::BadRequest(msg)) = result {
                assert!(msg.contains("require secure configuration setup"));
            } else {
                panic!(
                    "Expected BadRequest error for signer type: {:?}",
                    signer_type
                );
            }
        }
    }

    #[actix_web::test]
    async fn test_update_signer_success() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        // Create a test signer
        let signer = create_test_signer_model("test-signer", SignerType::Local);
        app_state.signer_repository.create(signer).await.unwrap();

        let update_request = create_test_signer_update_request();

        let result = update_signer(
            "test-signer".to_string(),
            update_request,
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        let api_response: ApiResponse<SignerResponse> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.id, "test-signer");
        // Note: name and description won't be updated in the response since
        // the repository model doesn't store metadata currently
    }

    #[actix_web::test]
    async fn test_update_signer_not_found() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let update_request = create_test_signer_update_request();

        let result = update_signer(
            "non-existent".to_string(),
            update_request,
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(matches!(error, ApiError::NotFound(_)));
    }

    #[actix_web::test]
    async fn test_delete_signer_success() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        // Create a test signer
        let signer = create_test_signer_model("test-signer", SignerType::Local);
        app_state.signer_repository.create(signer).await.unwrap();

        let result = delete_signer(
            "test-signer".to_string(),
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        let api_response: ApiResponse<&str> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        assert_eq!(api_response.data.unwrap(), "Signer deleted successfully");
    }

    #[actix_web::test]
    async fn test_delete_signer_not_found() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let result = delete_signer(
            "non-existent".to_string(),
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(matches!(error, ApiError::NotFound(_)));
    }

    #[actix_web::test]
    async fn test_signer_response_conversion() {
        let signer_model = SignerRepoModel {
            id: "test-id".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: SecretVec::new(32, |v| v.copy_from_slice(&[1; 32])),
            }),
        };

        let response = SignerResponse::from(signer_model);

        assert_eq!(response.id, "test-id");
        assert_eq!(response.r#type, SignerType::Local);
    }
}
