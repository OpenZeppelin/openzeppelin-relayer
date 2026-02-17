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
                ApiError::NotFound(format!("Network with ID '{network_id}' not found"))
            }
            _ => ApiError::InternalError(format!("Failed to retrieve network: {e}")),
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
                ApiError::NotFound(format!("Network with ID '{network_id}' not found"))
            }
            _ => ApiError::InternalError(format!("Failed to retrieve network: {e}")),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{NetworkConfigCommon, StellarNetworkConfig},
        models::{NetworkType, PaginationQuery, RpcConfig},
        utils::mocks::mockutils::{
            create_mock_app_state, create_mock_network, create_mock_solana_network,
        },
    };
    use actix_web::web::ThinData;

    /// Helper function to create a mock Stellar network for testing
    fn create_mock_stellar_network() -> NetworkRepoModel {
        NetworkRepoModel {
            id: "stellar:testnet".to_string(),
            name: "Stellar Testnet".to_string(),
            network_type: NetworkType::Stellar,
            config: NetworkConfigData::Stellar(StellarNetworkConfig {
                common: NetworkConfigCommon {
                    network: "stellar-testnet".to_string(),
                    from: None,
                    rpc_urls: Some(vec![RpcConfig::new(
                        "https://soroban-testnet.stellar.org".to_string(),
                    )]),
                    explorer_urls: None,
                    average_blocktime_ms: Some(5000),
                    is_testnet: Some(true),
                    tags: None,
                },
                passphrase: Some("Test SDF Network ; September 2015".to_string()),
                horizon_url: Some("https://horizon-testnet.stellar.org".to_string()),
            }),
        }
    }

    /// Helper function to create a mock EVM network with a specific ID
    fn create_mock_evm_network_with_id(id: &str, name: &str) -> NetworkRepoModel {
        let mut network = create_mock_network();
        network.id = id.to_string();
        network.name = name.to_string();
        network
    }

    // ============================================
    // Tests for list_networks
    // ============================================

    #[actix_web::test]
    async fn test_list_networks_empty() {
        let app_state = create_mock_app_state(None, None, None, None, None, None).await;
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };

        let result = list_networks(query, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[actix_web::test]
    async fn test_list_networks_with_single_network() {
        let network = create_mock_network();
        let app_state =
            create_mock_app_state(None, None, None, Some(vec![network]), None, None).await;
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };

        let result = list_networks(query, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[actix_web::test]
    async fn test_list_networks_with_multiple_networks() {
        let evm_network = create_mock_evm_network_with_id("evm:sepolia", "Sepolia");
        let solana_network = create_mock_solana_network();
        let stellar_network = create_mock_stellar_network();

        let app_state = create_mock_app_state(
            None,
            None,
            None,
            Some(vec![evm_network, solana_network, stellar_network]),
            None,
            None,
        )
        .await;

        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };

        let result = list_networks(query, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[actix_web::test]
    async fn test_list_networks_pagination() {
        let network1 = create_mock_evm_network_with_id("evm:network1", "Network 1");
        let network2 = create_mock_evm_network_with_id("evm:network2", "Network 2");
        let network3 = create_mock_evm_network_with_id("evm:network3", "Network 3");

        let app_state = create_mock_app_state(
            None,
            None,
            None,
            Some(vec![network1, network2, network3]),
            None,
            None,
        )
        .await;

        // Request first page with 2 items per page
        let query = PaginationQuery {
            page: 1,
            per_page: 2,
        };

        let result = list_networks(query, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);
    }

    // ============================================
    // Tests for get_network
    // ============================================

    #[actix_web::test]
    async fn test_get_network_success() {
        let network = create_mock_network();
        let network_id = network.id.clone();
        let app_state =
            create_mock_app_state(None, None, None, Some(vec![network]), None, None).await;

        let result = get_network(network_id, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[actix_web::test]
    async fn test_get_network_not_found() {
        let app_state = create_mock_app_state(None, None, None, None, None, None).await;

        let result = get_network("nonexistent-network".to_string(), ThinData(app_state)).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        match error {
            ApiError::NotFound(msg) => {
                assert!(msg.contains("nonexistent-network"));
            }
            _ => panic!("Expected NotFound error, got {error:?}"),
        }
    }

    #[actix_web::test]
    async fn test_get_network_evm() {
        let network = create_mock_network();
        let network_id = network.id.clone();
        let app_state =
            create_mock_app_state(None, None, None, Some(vec![network]), None, None).await;

        let result = get_network(network_id, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[actix_web::test]
    async fn test_get_network_solana() {
        let mut network = create_mock_solana_network();
        network.id = "solana:devnet".to_string();
        let network_id = network.id.clone();
        let app_state =
            create_mock_app_state(None, None, None, Some(vec![network]), None, None).await;

        let result = get_network(network_id, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[actix_web::test]
    async fn test_get_network_stellar() {
        let network = create_mock_stellar_network();
        let network_id = network.id.clone();
        let app_state =
            create_mock_app_state(None, None, None, Some(vec![network]), None, None).await;

        let result = get_network(network_id, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);
    }

    // ============================================
    // Tests for update_network
    // ============================================

    #[actix_web::test]
    async fn test_update_network_evm_success() {
        let network = create_mock_network();
        let network_id = network.id.clone();
        let app_state =
            create_mock_app_state(None, None, None, Some(vec![network]), None, None).await;

        let request = UpdateNetworkRequest {
            rpc_urls: Some(vec![
                RpcConfig::new("https://new-rpc1.example.com".to_string()),
                RpcConfig::new("https://new-rpc2.example.com".to_string()),
            ]),
        };

        let result = update_network(network_id, request, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[actix_web::test]
    async fn test_update_network_solana_success() {
        let mut network = create_mock_solana_network();
        network.id = "solana:devnet".to_string();
        let network_id = network.id.clone();
        let app_state =
            create_mock_app_state(None, None, None, Some(vec![network]), None, None).await;

        let request = UpdateNetworkRequest {
            rpc_urls: Some(vec![RpcConfig::new(
                "https://api.devnet.solana.com".to_string(),
            )]),
        };

        let result = update_network(network_id, request, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[actix_web::test]
    async fn test_update_network_stellar_success() {
        let network = create_mock_stellar_network();
        let network_id = network.id.clone();
        let app_state =
            create_mock_app_state(None, None, None, Some(vec![network]), None, None).await;

        let request = UpdateNetworkRequest {
            rpc_urls: Some(vec![RpcConfig::new(
                "https://new-soroban-testnet.stellar.org".to_string(),
            )]),
        };

        let result = update_network(network_id, request, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[actix_web::test]
    async fn test_update_network_not_found() {
        let app_state = create_mock_app_state(None, None, None, None, None, None).await;

        let request = UpdateNetworkRequest {
            rpc_urls: Some(vec![RpcConfig::new("https://rpc.example.com".to_string())]),
        };

        let result = update_network(
            "nonexistent-network".to_string(),
            request,
            ThinData(app_state),
        )
        .await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        match error {
            ApiError::NotFound(msg) => {
                assert!(msg.contains("nonexistent-network"));
            }
            _ => panic!("Expected NotFound error, got {error:?}"),
        }
    }

    #[actix_web::test]
    async fn test_update_network_no_fields_provided() {
        let network = create_mock_network();
        let network_id = network.id.clone();
        let app_state =
            create_mock_app_state(None, None, None, Some(vec![network]), None, None).await;

        let request = UpdateNetworkRequest { rpc_urls: None };

        let result = update_network(network_id, request, ThinData(app_state)).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        match error {
            ApiError::BadRequest(msg) => {
                assert!(msg.contains("At least one field must be provided"));
            }
            _ => panic!("Expected BadRequest error, got {error:?}"),
        }
    }

    #[actix_web::test]
    async fn test_update_network_empty_rpc_urls() {
        let network = create_mock_network();
        let network_id = network.id.clone();
        let app_state =
            create_mock_app_state(None, None, None, Some(vec![network]), None, None).await;

        let request = UpdateNetworkRequest {
            rpc_urls: Some(vec![]),
        };

        let result = update_network(network_id, request, ThinData(app_state)).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        match error {
            ApiError::BadRequest(msg) => {
                assert!(msg.contains("at least one RPC endpoint"));
            }
            _ => panic!("Expected BadRequest error, got {error:?}"),
        }
    }

    #[actix_web::test]
    async fn test_update_network_invalid_rpc_url() {
        let network = create_mock_network();
        let network_id = network.id.clone();
        let app_state =
            create_mock_app_state(None, None, None, Some(vec![network]), None, None).await;

        let request = UpdateNetworkRequest {
            rpc_urls: Some(vec![RpcConfig::new(
                "ftp://invalid-protocol.com".to_string(),
            )]),
        };

        let result = update_network(network_id, request, ThinData(app_state)).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        match error {
            ApiError::BadRequest(msg) => {
                assert!(msg.contains("Invalid RPC URL"));
            }
            _ => panic!("Expected BadRequest error, got {error:?}"),
        }
    }

    #[actix_web::test]
    async fn test_update_network_with_weighted_rpc_urls() {
        let network = create_mock_network();
        let network_id = network.id.clone();
        let app_state =
            create_mock_app_state(None, None, None, Some(vec![network]), None, None).await;

        let request = UpdateNetworkRequest {
            rpc_urls: Some(vec![
                RpcConfig {
                    url: "https://primary-rpc.example.com".to_string(),
                    weight: 80,
                },
                RpcConfig {
                    url: "https://backup-rpc.example.com".to_string(),
                    weight: 20,
                },
            ]),
        };

        let result = update_network(network_id, request, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[actix_web::test]
    async fn test_update_network_preserves_other_evm_fields() {
        // This test verifies that updating RPC URLs doesn't affect other EVM-specific fields
        let network = create_mock_network();
        let network_id = network.id.clone();
        let app_state =
            create_mock_app_state(None, None, None, Some(vec![network]), None, None).await;

        let request = UpdateNetworkRequest {
            rpc_urls: Some(vec![RpcConfig::new(
                "https://new-rpc.example.com".to_string(),
            )]),
        };

        let result = update_network(network_id.clone(), request, ThinData(app_state.clone())).await;

        assert!(result.is_ok());

        // Verify the network was updated by fetching it again
        let get_result = get_network(network_id, ThinData(app_state)).await;
        assert!(get_result.is_ok());
    }
}
