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
        NotificationRepoModel, PaginationMeta, PaginationQuery, RelayerRepoModel, SecretString,
        SignerRepoModel, ThinDataAppState, TransactionRepoModel,
    },
    repositories::{
        ApiKeyRepositoryTrait, NetworkRepository, PluginRepositoryTrait, RelayerRepository,
        Repository, TransactionCounterTrait, TransactionRepository,
    },
};
use actix_web::HttpResponse;
use chrono::Utc;
use eyre::Result;
use uuid::Uuid;

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
    let api_key = ApiKeyRepoModel {
        id: Uuid::new_v4().to_string(),
        value: SecretString::new(&Uuid::new_v4().to_string()),
        name: api_key_request.name,
        allowed_origins: api_key_request
            .allowed_origins
            .unwrap_or(vec!["*".to_string()]),
        permissions: api_key_request.permissions,
        created_at: Utc::now().to_string(),
    };

    let api_key = state.api_key_repository.create(api_key).await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success(api_key)))
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
        .map(|api_key| ApiKeyResponse {
            id: api_key.id,
            name: api_key.name,
            allowed_origins: api_key.allowed_origins,
            created_at: api_key.created_at,
            permissions: api_key.permissions,
        })
        .collect();

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
/// If the API key is the last Admin API key in the system, it will return an error.
///
pub async fn delete_api_key<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
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
    state.api_key_repository.delete_by_id(&api_key_id).await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success(api_key_id)))
}

#[cfg(test)]
mod tests {}
