/// This module provides utility functions and structures for managing and interacting
/// with relayers within the application. It includes functions to retrieve relayers
/// by ID, construct network relayers, and handle unsupported operations for specific
/// relayer types.
///
/// The primary components of this module are:
/// - `get_relayer_by_id`: Retrieves a relayer from the repository using its ID.
/// - `get_network_relayer`: Constructs a network relayer using a relayer ID.
/// - `get_network_relayer_by_model`: Constructs a network relayer using a relayer model.
/// - `solana_not_supported`: Returns an error for unsupported Solana relayer operations.
///
/// These utilities are essential for the application's relayer management and
/// interaction with the underlying repositories and factories.
use actix_web::web::ThinData;

use crate::{
    domain::{RelayerFactory, RelayerFactoryTrait},
    jobs::{JobProducer, JobProducerTrait},
    models::{ApiError, AppState, RelayerError, RelayerRepoModel},
    repositories::Repository,
};

use super::NetworkRelayer;

/// Retrieves a relayer by its ID from the repository.
///
/// # Arguments
///
/// * `relayer_id` - A string slice that holds the ID of the relayer.
/// * `state` - A reference to the application state.
///
/// # Returns
///
/// * `Result<RelayerRepoModel, ApiError>` - Returns a `RelayerRepoModel` on success, or an
///   `ApiError` on failure.
pub async fn get_relayer_by_id<P>(
    relayer_id: String,
    state: &ThinData<AppState<P>>,
) -> Result<RelayerRepoModel, ApiError>
where
    P: JobProducerTrait + 'static,
{
    state
        .relayer_repository
        .get_by_id(relayer_id)
        .await
        .map_err(|e| e.into())
}

/// Retrieves a network relayer by its ID, constructing it using the relayer and signer models.
///
/// # Arguments
///
/// * `relayer_id` - A string slice that holds the ID of the relayer.
/// * `state` - A reference to the application state.
///
/// # Returns
///
/// * `Result<NetworkRelayer, ApiError>` - Returns a `NetworkRelayer` on success, or an `ApiError`
///   on failure.
pub async fn get_network_relayer(
    relayer_id: String,
    state: &ThinData<AppState<JobProducer>>,
) -> Result<NetworkRelayer, ApiError> {
    let relayer_model = get_relayer_by_id(relayer_id, state).await?;
    let signer_model = state
        .signer_repository
        .get_by_id(relayer_model.signer_id.clone())
        .await?;

    RelayerFactory::create_relayer(
        relayer_model,
        signer_model,
        state.relayer_repository(),
        state.transaction_repository(),
        state.transaction_counter_store(),
        state.job_producer(),
    )
    .map_err(|e| e.into())
}

/// Constructs a network relayer using a given relayer model.
///
/// # Arguments
///
/// * `relayer_model` - A `RelayerRepoModel` that holds the relayer data.
/// * `state` - A reference to the application state.
///
/// # Returns
///
/// * `Result<NetworkRelayer, ApiError>` - Returns a `NetworkRelayer` on success, or an `ApiError`
///   on failure.
pub async fn get_network_relayer_by_model(
    relayer_model: RelayerRepoModel,
    state: &ThinData<AppState<JobProducer>>,
) -> Result<NetworkRelayer, ApiError> {
    let signer_model = state
        .signer_repository
        .get_by_id(relayer_model.signer_id.clone())
        .await?;

    RelayerFactory::create_relayer(
        relayer_model,
        signer_model,
        state.relayer_repository(),
        state.transaction_repository(),
        state.transaction_counter_store(),
        state.job_producer(),
    )
    .map_err(|e| e.into())
}

/// Returns an error indicating that the endpoint is not supported for Solana relayers.
///
/// # Returns
///
/// * `Result<T, RelayerError>` - Always returns a `RelayerError::NotSupported`.
pub fn solana_not_supported_relayer<T>() -> Result<T, RelayerError> {
    Err(RelayerError::NotSupported(
        "Endpoint is not supported for Solana relayers".to_string(),
    ))
}
