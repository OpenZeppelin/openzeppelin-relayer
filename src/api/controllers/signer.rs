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
/// An error indicating that updates are not allowed.
///
/// # Note
///
/// Signer updates are not supported for security reasons. To modify a signer,
/// delete the existing one and create a new signer with the desired configuration.
pub async fn update_signer<J, RR, TR, NR, NFR, SR, TCR, PR>(
    _signer_id: String,
    _request: SignerUpdateRequest,
    _state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
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
    Err(ApiError::BadRequest(
        "Signer updates are not allowed for security reasons. Please delete the existing signer and create a new one with the desired configuration.".to_string()
    ))
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
/// # Security
///
/// This endpoint ensures that signers cannot be deleted if they are still being
/// used by any relayers. This prevents breaking existing relayer configurations
/// and maintains system integrity.
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
    // First check if the signer exists
    let _signer = state.signer_repository.get_by_id(signer_id.clone()).await?;

    // Check if any relayers are using this signer
    let connected_relayers = state
        .relayer_repository
        .list_by_signer_id(&signer_id)
        .await?;

    if !connected_relayers.is_empty() {
        let relayer_names: Vec<String> =
            connected_relayers.iter().map(|r| r.name.clone()).collect();
        return Err(ApiError::BadRequest(format!(
            "Cannot delete signer '{}' because it is being used by {} relayer(s): {}. Please remove or reconfigure these relayers before deleting the signer.",
            signer_id,
            connected_relayers.len(),
            relayer_names.join(", ")
        )));
    }

    // Safe to delete - no relayers are using this signer
    state.signer_repository.delete_by_id(signer_id).await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success("Signer deleted successfully")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::{
            AwsKmsSignerConfigStorage, AwsKmsSignerRequestConfig,
            GoogleCloudKmsSignerKeyRequestConfig, GoogleCloudKmsSignerRequestConfig,
            GoogleCloudKmsSignerServiceAccountRequestConfig, LocalSignerConfigStorage,
            LocalSignerRequestConfig, SignerConfigRequest, SignerConfigStorage, SignerType,
            SignerTypeRequest, TurnkeySignerRequestConfig, VaultSignerRequestConfig,
        },
        utils::mocks::mockutils::create_mock_app_state,
    };
    use secrets::SecretVec;

    /// Helper function to create a test signer model
    fn create_test_signer_model(id: &str, signer_type: SignerType) -> SignerRepoModel {
        let config = match signer_type {
            SignerType::Local => SignerConfigStorage::Local(LocalSignerConfigStorage {
                raw_key: SecretVec::new(32, |v| v.copy_from_slice(&[1; 32])),
            }),
            SignerType::AwsKms => SignerConfigStorage::AwsKms(AwsKmsSignerConfigStorage {
                region: Some("us-east-1".to_string()),
                key_id: "test-key-id".to_string(),
            }),
            _ => SignerConfigStorage::Local(LocalSignerConfigStorage {
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
        use crate::models::{
            AwsKmsSignerRequestConfig, LocalSignerRequestConfig, SignerConfigRequest,
            SignerTypeRequest,
        };

        let (signer_type_req, config) = match signer_type {
            SignerType::Local => (
                SignerTypeRequest::Local,
                SignerConfigRequest::Local(LocalSignerRequestConfig {
                    key: "1111111111111111111111111111111111111111111111111111111111111111"
                        .to_string(), // Valid 32-byte hex key
                }),
            ),
            SignerType::AwsKms => (
                SignerTypeRequest::AwsKms,
                SignerConfigRequest::AwsKms(AwsKmsSignerRequestConfig {
                    region: "us-east-1".to_string(),
                    key_id: "test-key-id".to_string(),
                }),
            ),
            _ => (
                SignerTypeRequest::Local,
                SignerConfigRequest::Local(LocalSignerRequestConfig {
                    key: "placeholder-key".to_string(),
                }),
            ), // Use Local for other types in helper
        };

        SignerCreateRequest {
            id,
            signer_type: signer_type_req,
            config,
        }
    }

    /// Helper function to create a test signer update request
    fn create_test_signer_update_request() -> SignerUpdateRequest {
        SignerUpdateRequest {}
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

        // Check that both signers are present
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
    async fn test_create_signer_with_valid_configs() {
        // Test Local signer with valid key
        let app_state1 = create_mock_app_state(None, None, None, None, None).await;
        let request =
            create_test_signer_create_request(Some("local-test".to_string()), SignerType::Local);
        let result = create_signer(request, actix_web::web::ThinData(app_state1)).await;
        assert!(result.is_ok(), "Local signer with valid key should succeed");

        // Test AWS KMS signer with valid config
        let app_state2 = create_mock_app_state(None, None, None, None, None).await;
        let request =
            create_test_signer_create_request(Some("aws-test".to_string()), SignerType::AwsKms);
        let result = create_signer(request, actix_web::web::ThinData(app_state2)).await;
        assert!(
            result.is_ok(),
            "AWS KMS signer with valid config should succeed"
        );
    }

    #[actix_web::test]
    async fn test_create_signer_local_with_valid_key() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let request = SignerCreateRequest {
            id: Some("local-signer-test".to_string()),
            signer_type: SignerTypeRequest::Local,
            config: SignerConfigRequest::Local(LocalSignerRequestConfig {
                key: "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890".to_string(), // 32 bytes as hex
            }),
        };

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
        assert_eq!(data.id, "local-signer-test");
        assert_eq!(data.r#type, SignerType::Local);
    }

    #[actix_web::test]
    async fn test_create_signer_aws_kms_comprehensive() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let request = SignerCreateRequest {
            id: Some("aws-kms-signer".to_string()),
            signer_type: SignerTypeRequest::AwsKms,
            config: SignerConfigRequest::AwsKms(AwsKmsSignerRequestConfig {
                region: "us-west-2".to_string(),
                key_id:
                    "arn:aws:kms:us-west-2:123456789012:key/12345678-1234-1234-1234-123456789012"
                        .to_string(),
            }),
        };

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
        assert_eq!(data.id, "aws-kms-signer");
        assert_eq!(data.r#type, SignerType::AwsKms);
    }

    #[actix_web::test]
    async fn test_create_signer_vault() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let request = SignerCreateRequest {
            id: Some("vault-signer".to_string()),
            signer_type: SignerTypeRequest::Vault,
            config: SignerConfigRequest::Vault(VaultSignerRequestConfig {
                address: "https://vault.example.com:8200".to_string(),
                namespace: Some("development".to_string()),
                role_id: "test-role-id-12345".to_string(),
                secret_id: "test-secret-id-67890".to_string(),
                key_name: "ethereum-key".to_string(),
                mount_point: Some("secret".to_string()),
            }),
        };

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
        assert_eq!(data.id, "vault-signer");
        assert_eq!(data.r#type, SignerType::Vault);
    }

    #[actix_web::test]
    async fn test_create_signer_vault_transit() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        use crate::models::{
            SignerConfigRequest, SignerTypeRequest, VaultTransitSignerRequestConfig,
        };
        let request = SignerCreateRequest {
            id: Some("vault-transit-signer".to_string()),
            signer_type: SignerTypeRequest::VaultTransit,
            config: SignerConfigRequest::VaultTransit(VaultTransitSignerRequestConfig {
                key_name: "ethereum-transit-key".to_string(),
                address: "https://vault.example.com:8200".to_string(),
                namespace: None,
                role_id: "transit-role-id-12345".to_string(),
                secret_id: "transit-secret-id-67890".to_string(),
                pubkey: "0x04a34b99f22c790c4e36b2b3c2c35a36db06226e41c692fc82b8b56ac1c540c5bd5b8dec5235a0fa8722476c7709c02559e3aa73aa03918ba2d492eea75abea235".to_string(),
                mount_point: Some("transit".to_string()),
            }),
        };

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
        assert_eq!(data.id, "vault-transit-signer");
        assert_eq!(data.r#type, SignerType::VaultTransit);
    }

    #[actix_web::test]
    async fn test_create_signer_turnkey() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let request = SignerCreateRequest {
            id: Some("turnkey-signer".to_string()),
            signer_type: SignerTypeRequest::Turnkey,
            config: SignerConfigRequest::Turnkey(TurnkeySignerRequestConfig {
                api_public_key: "turnkey-api-public-key-example".to_string(),
                api_private_key: "turnkey-api-private-key-example".to_string(),
                organization_id: "turnkey-org-12345".to_string(),
                private_key_id: "turnkey-private-key-67890".to_string(),
                public_key: "0x04a34b99f22c790c4e36b2b3c2c35a36db06226e41c692fc82b8b56ac1c540c5bd5b8dec5235a0fa8722476c7709c02559e3aa73aa03918ba2d492eea75abea235".to_string(),
            }),
        };

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
        assert_eq!(data.id, "turnkey-signer");
        assert_eq!(data.r#type, SignerType::Turnkey);
    }

    #[actix_web::test]
    async fn test_create_signer_google_cloud_kms() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let request = SignerCreateRequest {
            id: Some("gcp-kms-signer".to_string()),
            signer_type: SignerTypeRequest::GoogleCloudKms,
            config: SignerConfigRequest::GoogleCloudKms(GoogleCloudKmsSignerRequestConfig {
                service_account: GoogleCloudKmsSignerServiceAccountRequestConfig {
                    private_key: "-----BEGIN PRIVATE KEY-----\nSDFGSDFGSDGSDFGSDFGSDFGSDFGSDFGSAFAS...\n-----END PRIVATE KEY-----\n".to_string(), // noboost
                    private_key_id: "gcp-private-key-id-12345".to_string(),
                    project_id: "my-gcp-project".to_string(),
                    client_email: "service-account@my-gcp-project.iam.gserviceaccount.com".to_string(),
                    client_id: "123456789012345678901".to_string(),
                    auth_uri: "https://accounts.google.com/o/oauth2/auth".to_string(),
                    token_uri: "https://oauth2.googleapis.com/token".to_string(),
                    auth_provider_x509_cert_url: "https://www.googleapis.com/oauth2/v1/certs".to_string(),
                    client_x509_cert_url: "https://www.googleapis.com/robot/v1/metadata/x509/service-account%40my-gcp-project.iam.gserviceaccount.com".to_string(),
                    universe_domain: "googleapis.com".to_string(),
                },
                key: GoogleCloudKmsSignerKeyRequestConfig {
                    location: "global".to_string(),
                    key_ring_id: "ethereum-keyring".to_string(),
                    key_id: "ethereum-signing-key".to_string(),
                    key_version: 1,
                },
            }),
        };

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
        assert_eq!(data.id, "gcp-kms-signer");
        assert_eq!(data.r#type, SignerType::GoogleCloudKms);
    }

    #[actix_web::test]
    async fn test_create_signer_auto_generated_id() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let request = SignerCreateRequest {
            id: None, // Let the system generate an ID
            signer_type: SignerTypeRequest::Local,
            config: SignerConfigRequest::Local(LocalSignerRequestConfig {
                key: "fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321".to_string(),
            }),
        };

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
        assert!(!data.id.is_empty());
        assert!(uuid::Uuid::parse_str(&data.id).is_ok()); // Should be a valid UUID
        assert_eq!(data.r#type, SignerType::Local);
    }

    #[actix_web::test]
    async fn test_create_signer_invalid_local_key() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let request = SignerCreateRequest {
            id: Some("invalid-key-signer".to_string()),
            signer_type: SignerTypeRequest::Local,
            config: SignerConfigRequest::Local(LocalSignerRequestConfig {
                key: "invalid-hex-key".to_string(), // Invalid hex
            }),
        };

        let result = create_signer(request, actix_web::web::ThinData(app_state)).await;

        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Invalid hex key format"));
        } else {
            panic!("Expected BadRequest error for invalid hex key");
        }
    }

    #[actix_web::test]
    async fn test_create_signer_invalid_vault_address() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let request = SignerCreateRequest {
            id: Some("invalid-vault-signer".to_string()),
            signer_type: SignerTypeRequest::Vault,
            config: SignerConfigRequest::Vault(VaultSignerRequestConfig {
                address: "not-a-valid-url".to_string(), // Invalid URL
                namespace: None,
                role_id: "test-role".to_string(),
                secret_id: "test-secret".to_string(),
                key_name: "test-key".to_string(),
                mount_point: None,
            }),
        };

        let result = create_signer(request, actix_web::web::ThinData(app_state)).await;

        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Address must be a valid URL"));
        } else {
            panic!("Expected BadRequest error for invalid Vault address");
        }
    }

    #[actix_web::test]
    async fn test_create_signer_empty_aws_kms_key_id() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let request = SignerCreateRequest {
            id: Some("empty-key-id-signer".to_string()),
            signer_type: SignerTypeRequest::AwsKms,
            config: SignerConfigRequest::AwsKms(AwsKmsSignerRequestConfig {
                region: "us-east-1".to_string(),
                key_id: "".to_string(), // Empty key ID
            }),
        };

        let result = create_signer(request, actix_web::web::ThinData(app_state)).await;

        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Key ID cannot be empty"));
        } else {
            panic!("Expected BadRequest error for empty AWS KMS key ID");
        }
    }

    #[actix_web::test]
    async fn test_update_signer_not_allowed() {
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

        assert!(result.is_err());
        let error = result.unwrap_err();
        if let ApiError::BadRequest(msg) = error {
            assert!(msg.contains("Signer updates are not allowed"));
            assert!(msg.contains("delete the existing signer and create a new one"));
        } else {
            panic!("Expected BadRequest error");
        }
    }

    #[actix_web::test]
    async fn test_update_signer_always_fails() {
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
        if let ApiError::BadRequest(msg) = error {
            assert!(msg.contains("Signer updates are not allowed"));
        } else {
            panic!("Expected BadRequest error");
        }
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
    async fn test_delete_signer_blocked_by_connected_relayers() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        // Create a test signer
        let signer = create_test_signer_model("connected-signer", SignerType::Local);
        app_state.signer_repository.create(signer).await.unwrap();

        // Create a relayer that uses this signer
        let relayer = crate::models::RelayerRepoModel {
            id: "test-relayer".to_string(),
            name: "Test Relayer".to_string(),
            network: "ethereum".to_string(),
            paused: false,
            network_type: crate::models::NetworkType::Evm,
            signer_id: "connected-signer".to_string(), // References our signer
            policies: crate::models::RelayerNetworkPolicy::Evm(
                crate::models::RelayerEvmPolicy::default(),
            ),
            address: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            notification_id: None,
            system_disabled: false,
            custom_rpc_urls: None,
        };
        app_state.relayer_repository.create(relayer).await.unwrap();

        // Try to delete the signer - should fail
        let result = delete_signer(
            "connected-signer".to_string(),
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        if let ApiError::BadRequest(msg) = error {
            assert!(msg.contains("Cannot delete signer"));
            assert!(msg.contains("being used by"));
            assert!(msg.contains("Test Relayer"));
            assert!(msg.contains("remove or reconfigure"));
        } else {
            panic!("Expected BadRequest error");
        }
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
    async fn test_delete_signer_after_relayer_removed() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        // Create a test signer
        let signer = create_test_signer_model("cleanup-signer", SignerType::Local);
        app_state.signer_repository.create(signer).await.unwrap();

        // Create a relayer that uses this signer
        let relayer = crate::models::RelayerRepoModel {
            id: "temp-relayer".to_string(),
            name: "Temporary Relayer".to_string(),
            network: "ethereum".to_string(),
            paused: false,
            network_type: crate::models::NetworkType::Evm,
            signer_id: "cleanup-signer".to_string(),
            policies: crate::models::RelayerNetworkPolicy::Evm(
                crate::models::RelayerEvmPolicy::default(),
            ),
            address: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            notification_id: None,
            system_disabled: false,
            custom_rpc_urls: None,
        };
        app_state.relayer_repository.create(relayer).await.unwrap();

        // First deletion attempt should fail
        let result = delete_signer(
            "cleanup-signer".to_string(),
            actix_web::web::ThinData(app_state),
        )
        .await;
        assert!(result.is_err());

        // Create new app state for second test (since app_state was consumed)
        let app_state2 = create_mock_app_state(None, None, None, None, None).await;

        // Re-create the signer in the new state
        let signer2 = create_test_signer_model("cleanup-signer", SignerType::Local);
        app_state2.signer_repository.create(signer2).await.unwrap();

        // Now signer deletion should succeed (no relayers in new state)
        let result = delete_signer(
            "cleanup-signer".to_string(),
            actix_web::web::ThinData(app_state2),
        )
        .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[actix_web::test]
    async fn test_signer_response_conversion() {
        let signer_model = SignerRepoModel {
            id: "test-id".to_string(),
            config: SignerConfigStorage::Local(LocalSignerConfigStorage {
                raw_key: SecretVec::new(32, |v| v.copy_from_slice(&[1; 32])),
            }),
        };

        let response = SignerResponse::from(signer_model);

        assert_eq!(response.id, "test-id");
        assert_eq!(response.r#type, SignerType::Local);
    }

    #[actix_web::test]
    async fn test_delete_signer_with_multiple_relayers() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        // Create a test signer
        let signer = create_test_signer_model("multi-relayer-signer", SignerType::AwsKms);
        app_state.signer_repository.create(signer).await.unwrap();

        // Create multiple relayers that use this signer
        let relayers = vec![
            crate::models::RelayerRepoModel {
                id: "relayer-1".to_string(),
                name: "EVM Relayer".to_string(),
                network: "ethereum".to_string(),
                paused: false,
                network_type: crate::models::NetworkType::Evm,
                signer_id: "multi-relayer-signer".to_string(),
                policies: crate::models::RelayerNetworkPolicy::Evm(
                    crate::models::RelayerEvmPolicy::default(),
                ),
                address: "0x1111111111111111111111111111111111111111".to_string(),
                notification_id: None,
                system_disabled: false,
                custom_rpc_urls: None,
            },
            crate::models::RelayerRepoModel {
                id: "relayer-2".to_string(),
                name: "Solana Relayer".to_string(),
                network: "solana".to_string(),
                paused: true, // Even paused relayers should block deletion
                network_type: crate::models::NetworkType::Solana,
                signer_id: "multi-relayer-signer".to_string(),
                policies: crate::models::RelayerNetworkPolicy::Solana(
                    crate::models::RelayerSolanaPolicy::default(),
                ),
                address: "solana-address".to_string(),
                notification_id: None,
                system_disabled: false,
                custom_rpc_urls: None,
            },
            crate::models::RelayerRepoModel {
                id: "relayer-3".to_string(),
                name: "Stellar Relayer".to_string(),
                network: "stellar".to_string(),
                paused: false,
                network_type: crate::models::NetworkType::Stellar,
                signer_id: "multi-relayer-signer".to_string(),
                policies: crate::models::RelayerNetworkPolicy::Stellar(
                    crate::models::RelayerStellarPolicy::default(),
                ),
                address: "stellar-address".to_string(),
                notification_id: None,
                system_disabled: true, // Even disabled relayers should block deletion
                custom_rpc_urls: None,
            },
        ];

        // Create all relayers
        for relayer in relayers {
            app_state.relayer_repository.create(relayer).await.unwrap();
        }

        // Try to delete the signer - should fail with detailed error
        let result = delete_signer(
            "multi-relayer-signer".to_string(),
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        if let ApiError::BadRequest(msg) = error {
            assert!(msg.contains("Cannot delete signer 'multi-relayer-signer'"));
            assert!(msg.contains("being used by 3 relayer(s)"));
            assert!(msg.contains("EVM Relayer"));
            assert!(msg.contains("Solana Relayer"));
            assert!(msg.contains("Stellar Relayer"));
            assert!(msg.contains("remove or reconfigure"));
        } else {
            panic!("Expected BadRequest error, got: {:?}", error);
        }
    }

    #[actix_web::test]
    async fn test_delete_signer_with_some_relayers_using_different_signer() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        // Create two test signers
        let signer1 = create_test_signer_model("signer-to-delete", SignerType::Local);
        let signer2 = create_test_signer_model("other-signer", SignerType::AwsKms);
        app_state.signer_repository.create(signer1).await.unwrap();
        app_state.signer_repository.create(signer2).await.unwrap();

        // Create relayers - only one uses the signer we want to delete
        let relayer1 = crate::models::RelayerRepoModel {
            id: "blocking-relayer".to_string(),
            name: "Blocking Relayer".to_string(),
            network: "ethereum".to_string(),
            paused: false,
            network_type: crate::models::NetworkType::Evm,
            signer_id: "signer-to-delete".to_string(), // This one blocks deletion
            policies: crate::models::RelayerNetworkPolicy::Evm(
                crate::models::RelayerEvmPolicy::default(),
            ),
            address: "0x1111111111111111111111111111111111111111".to_string(),
            notification_id: None,
            system_disabled: false,
            custom_rpc_urls: None,
        };

        let relayer2 = crate::models::RelayerRepoModel {
            id: "non-blocking-relayer".to_string(),
            name: "Non-blocking Relayer".to_string(),
            network: "polygon".to_string(),
            paused: false,
            network_type: crate::models::NetworkType::Evm,
            signer_id: "other-signer".to_string(), // This one uses different signer
            policies: crate::models::RelayerNetworkPolicy::Evm(
                crate::models::RelayerEvmPolicy::default(),
            ),
            address: "0x2222222222222222222222222222222222222222".to_string(),
            notification_id: None,
            system_disabled: false,
            custom_rpc_urls: None,
        };

        app_state.relayer_repository.create(relayer1).await.unwrap();
        app_state.relayer_repository.create(relayer2).await.unwrap();

        // Try to delete the first signer - should fail because of one relayer
        let result = delete_signer(
            "signer-to-delete".to_string(),
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        if let ApiError::BadRequest(msg) = error {
            assert!(msg.contains("being used by 1 relayer(s)"));
            assert!(msg.contains("Blocking Relayer"));
            assert!(!msg.contains("Non-blocking Relayer")); // Should not mention the other relayer
        } else {
            panic!("Expected BadRequest error");
        }

        // Try to delete the second signer - should succeed (no relayers using it in our test)
        let app_state2 = create_mock_app_state(None, None, None, None, None).await;
        let signer2_recreated = create_test_signer_model("other-signer", SignerType::AwsKms);
        app_state2
            .signer_repository
            .create(signer2_recreated)
            .await
            .unwrap();

        let result = delete_signer(
            "other-signer".to_string(),
            actix_web::web::ThinData(app_state2),
        )
        .await;

        assert!(result.is_ok());
    }
}
