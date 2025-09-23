//! # Api Key Controller
//!
//! Handles HTTP endpoints for api key operations including:
//! - Create api keys
//! - List api keys
//! - Delete api keys
use crate::{
    jobs::JobProducerTrait,
    models::{
        ApiError, ApiKeyRepoModel, ApiKeyRequest, ApiKeyResponse, ApiResponse, NetworkRepoModel,
        NotificationRepoModel, PaginationMeta, PaginationQuery, RelayerRepoModel, SignerRepoModel,
        ThinDataAppState, TransactionRepoModel,
    },
    repositories::{
        ApiKeyRepositoryTrait, NetworkRepository, PluginRepositoryTrait, RelayerRepository,
        Repository, TransactionCounterTrait, TransactionRepository,
    },
};
use actix_web::HttpResponse;
use eyre::Result;

/// Create api key
///
/// # Arguments
///
/// * `api_key_request` - The api key request.
///     * `name` - The name of the api key.
///     * `allowed_origins` - The allowed origins for the api key.
///     * `permissions` - The permissions for the api key.
/// * `state` - The application state containing the api key repository.
///
/// # Returns
///
/// The result of the plugin call.
pub async fn create_api_key<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    api_key_request: ApiKeyRequest,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
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
    AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
{
    let api_key = ApiKeyRepoModel::try_from(api_key_request)?;

    let api_key = state.api_key_repository.create(api_key).await?;

    Ok(HttpResponse::Created().json(ApiResponse::success(api_key)))
}

/// List api keys
///
/// # Arguments
///
/// * `query` - The pagination query parameters.
///     * `page` - The page number.
///     * `per_page` - The number of items per page.
/// * `state` - The application state containing the api key repository.
///
/// # Returns
///
/// The result of the api key list.
pub async fn list_api_keys<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    query: PaginationQuery,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
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
    AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
{
    let api_keys = state.api_key_repository.list_paginated(query).await?;

    let api_key_items: Vec<ApiKeyRepoModel> = api_keys.items.into_iter().collect();

    // Subtract the "value" from the api key to avoid exposing it.
    let api_key_items: Vec<ApiKeyResponse> = api_key_items
        .into_iter()
        .map(ApiKeyResponse::try_from)
        .collect::<Result<Vec<ApiKeyResponse>, ApiError>>()?;

    Ok(HttpResponse::Ok().json(ApiResponse::paginated(
        api_key_items,
        PaginationMeta {
            total_items: api_keys.total,
            current_page: api_keys.page,
            per_page: api_keys.per_page,
        },
    )))
}

/// Get api key permissions
///
/// # Arguments
///
/// * `api_key_id` - The id of the api key.
/// * `state` - The application state containing the api key repository.
///
pub async fn get_api_key_permissions<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    api_key_id: String,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
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
    AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
{
    let permissions = state
        .api_key_repository
        .list_permissions(&api_key_id)
        .await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success(permissions)))
}

/// Delete api key
///
/// # Arguments
///
/// * `api_key_id` - The id of the api key.
/// * `state` - The application state containing the api key repository.
///
pub async fn delete_api_key<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    _api_key_id: String,
    _state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
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
    AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
{
    // state.api_key_repository.delete_by_id(&api_key_id).await?;

    // Ok(HttpResponse::Ok().json(ApiResponse::success(api_key_id)))
    Ok(HttpResponse::Ok().json(ApiResponse::<String>::error("Not implemented".to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::{ApiKeyRepoModel, PaginationQuery, SecretString},
        utils::mocks::mockutils::create_mock_app_state,
    };
    use actix_web::web::ThinData;

    /// Helper function to create a test api key model
    fn create_test_api_key_model(id: &str) -> ApiKeyRepoModel {
        ApiKeyRepoModel {
            id: id.to_string(),
            value: SecretString::new("test-api-key-value"),
            name: "Test API Key".to_string(),
            allowed_origins: vec!["*".to_string()],
            permissions: vec!["relayer:all:execute".to_string()],
            created_at: "2023-01-01T00:00:00Z".to_string(),
        }
    }

    /// Helper function to create a test api key create request
    fn create_test_api_key_create_request(name: &str) -> ApiKeyRequest {
        ApiKeyRequest {
            name: name.to_string(),
            permissions: vec!["relayer:all:execute".to_string()],
            allowed_origins: Some(vec!["*".to_string()]),
        }
    }

    #[actix_web::test]
    async fn test_create_api_key() {
        let app_state = create_mock_app_state(None, None, None, None, None, None).await;
        let api_key_request = create_test_api_key_create_request("Test API Key");

        let result = create_api_key(api_key_request, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 201);
    }

    #[actix_web::test]
    async fn test_list_api_keys_empty() {
        let app_state = create_mock_app_state(None, None, None, None, None, None).await;
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };

        let result = list_api_keys(query, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[actix_web::test]
    async fn test_list_api_keys_with_data() {
        let api_key = create_test_api_key_model("test-api-key-1");
        let app_state =
            create_mock_app_state(Some(vec![api_key]), None, None, None, None, None).await;
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };

        let result = list_api_keys(query, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[actix_web::test]
    async fn test_get_api_key_permissions() {
        let api_key = create_test_api_key_model("test-api-key-1");
        let api_key_id = api_key.id.clone();
        let app_state =
            create_mock_app_state(Some(vec![api_key]), None, None, None, None, None).await;

        let result = get_api_key_permissions(api_key_id, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);
    }

    // #[actix_web::test]
    // async fn test_delete_api_key() {
    //     let api_key = create_test_api_key_model("test-api-key-1");
    //     let api_key_id = api_key.id.clone();
    //     let app_state =
    //         create_mock_app_state(Some(vec![api_key]), None, None, None, None, None).await;

    //     let result = delete_api_key(api_key_id, ThinData(app_state)).await;

    //     assert!(result.is_ok());
    //     let response = result.unwrap();
    //     assert_eq!(response.status(), 200);
    // }

    #[actix_web::test]
    async fn test_get_permissions_nonexistent_api_key() {
        let app_state = create_mock_app_state(None, None, None, None, None, None).await;

        let result =
            get_api_key_permissions("nonexistent-id".to_string(), ThinData(app_state)).await;

        assert!(result.is_err());
    }
}
