//! # Network Controller
//!
//! Handles HTTP endpoints for network operations including:
//! - Listing networks
//! - Getting network details
//! - Updating network RPC URLs

use crate::{
    config::{EvmNetworkConfig, SolanaNetworkConfig, StellarNetworkConfig},
    jobs::JobProducerTrait,
    models::{
        ApiError, ApiResponse, NetworkConfigData, NetworkRepoModel, NetworkResponse,
        PaginationMeta, PaginationQuery, ThinDataAppState, UpdateNetworkRequest,
    },
    repositories::{
        ApiKeyRepositoryTrait, NetworkRepository, PluginRepositoryTrait, RelayerRepository,
        Repository, TransactionCounterTrait, TransactionRepository,
    },
};
use actix_web::HttpResponse;
use eyre::Result;

/// Lists all networks with pagination support.
///
/// # Arguments
///
/// * `query` - The pagination query parameters.
/// * `state` - The application state containing the network repository.
///
/// # Returns
///
/// A paginated list of networks.
pub async fn list_networks<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    query: PaginationQuery,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository
        + Repository<crate::models::RelayerRepoModel, String>
        + Send
        + Sync
        + 'static,
    TR: TransactionRepository
        + Repository<crate::models::TransactionRepoModel, String>
        + Send
        + Sync
        + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<crate::models::NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<crate::models::SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
    AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
{
    let networks = state.network_repository.list_paginated(query).await?;

    let mapped_networks: Vec<NetworkResponse> =
        networks.items.into_iter().map(|n| n.into()).collect();

    Ok(HttpResponse::Ok().json(ApiResponse::paginated(
        mapped_networks,
        PaginationMeta {
            total_items: networks.total,
            current_page: networks.page,
            per_page: networks.per_page,
        },
    )))
}

/// Retrieves details of a specific network by ID.
///
/// # Arguments
///
/// * `network_id` - The ID of the network (e.g., "evm:sepolia", "solana:mainnet").
/// * `state` - The application state containing the network repository.
///
/// # Returns
///
/// The details of the specified network or 404 if not found.
pub async fn get_network<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    network_id: String,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository
        + Repository<crate::models::RelayerRepoModel, String>
        + Send
        + Sync
        + 'static,
    TR: TransactionRepository
        + Repository<crate::models::TransactionRepoModel, String>
        + Send
        + Sync
        + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<crate::models::NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<crate::models::SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
    AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
{
    let network = state
        .network_repository
        .get_by_id(network_id.clone())
        .await
        .map_err(|e| match e {
            crate::models::RepositoryError::NotFound(_) => {
                ApiError::NotFound(format!("Network with ID '{}' not found", network_id))
            }
            _ => ApiError::InternalError(format!("Failed to retrieve network: {}", e)),
        })?;

    let network_response: NetworkResponse = network.into();

    Ok(HttpResponse::Ok().json(ApiResponse::success(network_response)))
}

/// Updates a network's configuration.
/// Currently supports updating RPC URLs only. Can be extended to support other fields.
///
/// # Arguments
///
/// * `network_id` - The ID of the network (e.g., "evm:sepolia", "solana:mainnet").
/// * `request` - The update request containing fields to update.
/// * `state` - The application state containing the network repository.
///
/// # Returns
///
/// The updated network or an error if update fails.
pub async fn update_network<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    network_id: String,
    request: UpdateNetworkRequest,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository
        + Repository<crate::models::RelayerRepoModel, String>
        + Send
        + Sync
        + 'static,
    TR: TransactionRepository
        + Repository<crate::models::TransactionRepoModel, String>
        + Send
        + Sync
        + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<crate::models::NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<crate::models::SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
    AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
{
    // Validate request
    request.validate()?;

    // Ensure at least one field is provided for update
    if request.rpc_urls.is_none() {
        return Err(ApiError::BadRequest(
            "At least one field must be provided for update".to_string(),
        ));
    }

    // Get existing network
    let mut network = state
        .network_repository
        .get_by_id(network_id.clone())
        .await
        .map_err(|e| match e {
            crate::models::RepositoryError::NotFound(_) => {
                ApiError::NotFound(format!("Network with ID '{}' not found", network_id))
            }
            _ => ApiError::InternalError(format!("Failed to retrieve network: {}", e)),
        })?;

    // Update fields in the network config
    let common = network.common();
    let mut updated_common = common.clone();

    // Update RPC URLs if provided
    if let Some(rpc_urls) = request.rpc_urls {
        updated_common.rpc_urls = Some(rpc_urls);
    }

    // Update the network config based on type
    let updated_config = match network.config {
        NetworkConfigData::Evm(evm_config) => NetworkConfigData::Evm(EvmNetworkConfig {
            common: updated_common,
            chain_id: evm_config.chain_id,
            required_confirmations: evm_config.required_confirmations,
            features: evm_config.features,
            symbol: evm_config.symbol,
            gas_price_cache: evm_config.gas_price_cache,
        }),
        NetworkConfigData::Solana(_) => NetworkConfigData::Solana(SolanaNetworkConfig {
            common: updated_common,
        }),
        NetworkConfigData::Stellar(stellar_config) => {
            NetworkConfigData::Stellar(StellarNetworkConfig {
                common: updated_common,
                passphrase: stellar_config.passphrase,
                horizon_url: stellar_config.horizon_url,
            })
        }
    };

    // Update the network model
    network.config = updated_config;

    // Save the updated network
    let saved_network = state
        .network_repository
        .update(network.id.clone(), network)
        .await?;

    let network_response: NetworkResponse = saved_network.into();

    Ok(HttpResponse::Ok().json(ApiResponse::success(network_response)))
}
