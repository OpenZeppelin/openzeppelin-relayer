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
use crate::{
    domain::{RelayerFactory, RelayerFactoryTrait},
    jobs::JobProducerTrait,
    models::{
        ApiError, NetworkRepoModel, NotificationRepoModel, RelayerError, RelayerRepoModel,
        SignerRepoModel, ThinDataAppState, TransactionRepoModel,
    },
    repositories::{
        NetworkRepository, PluginRepositoryTrait, RelayerRepository, Repository,
        TransactionCounterTrait, TransactionRepository,
    },
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
pub async fn get_relayer_by_id<J, RR, TR, NR, NFR, SR, TCR, PR>(
    relayer_id: String,
    state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<RelayerRepoModel, ApiError>
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
pub async fn get_network_relayer<J, RR, TR, NR, NFR, SR, TCR, PR>(
    relayer_id: String,
    state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<NetworkRelayer<J, TR, RR, NR, TCR>, ApiError>
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
    let relayer_model = get_relayer_by_id(relayer_id.clone(), state).await?;
    let signer_model = state
        .signer_repository
        .get_by_id(relayer_model.signer_id.clone())
        .await?;

    RelayerFactory::create_relayer(relayer_model, signer_model, state)
        .await
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
pub async fn get_network_relayer_by_model<J, RR, TR, NR, NFR, SR, TCR, PR>(
    relayer_model: RelayerRepoModel,
    state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<NetworkRelayer<J, TR, RR, NR, TCR>, ApiError>
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
    let signer_model = state
        .signer_repository
        .get_by_id(relayer_model.signer_id.clone())
        .await?;

    RelayerFactory::create_relayer(relayer_model, signer_model, state)
        .await
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
