//! This module provides utility functions for handling transactions within the application.
//!
//! It includes functions to retrieve transactions by ID, create relayer transactions, and
//! handle unsupported operations for specific relayers. The module interacts with various
//! repositories and factories to perform these operations.
use actix_web::web::ThinData;

use crate::{
    domain::get_relayer_by_id,
    jobs::JobProducerTrait,
    models::{
        ApiError, DefaultAppState, NetworkRepoModel, NotificationRepoModel, RelayerRepoModel,
        SignerRepoModel, ThinDataAppState, TransactionError, TransactionRepoModel,
    },
    repositories::{
        NetworkRepository, PluginRepositoryTrait, RelayerRepository, Repository,
        TransactionCounterTrait, TransactionRepository,
    },
};

use super::{NetworkTransaction, RelayerTransactionFactory};

/// Retrieves a transaction by its ID.
///
/// # Arguments
///
/// * `transaction_id` - A `String` representing the ID of the transaction to retrieve.
/// * `state` - A reference to the application state, wrapped in `ThinData`.
///
/// # Returns
///
/// A `Result` containing a `TransactionRepoModel` if successful, or an `ApiError` if an error
/// occurs.
pub async fn get_transaction_by_id<J, RR, TR, NR, NFR, SR, TCR, PR>(
    transaction_id: String,
    state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<TransactionRepoModel, ApiError>
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
        .transaction_repository
        .get_by_id(transaction_id)
        .await
        .map_err(|e| e.into())
}

/// Creates a relayer network transaction instance based on the relayer ID.
///
/// # Arguments
///
/// * `relayer_id` - A `String` representing the ID of the relayer.
/// * `state` - A reference to the application state, wrapped in `ThinData`.
///
/// # Returns
///
/// A `Result` containing a `NetworkTransaction` if successful, or an `ApiError` if an error occurs.
pub async fn get_relayer_transaction(
    relayer_id: String,
    state: &ThinData<DefaultAppState>,
) -> Result<NetworkTransaction, ApiError> {
    let relayer_model = get_relayer_by_id(relayer_id, state).await?;
    let signer_model = state
        .signer_repository
        .get_by_id(relayer_model.signer_id.clone())
        .await?;

    RelayerTransactionFactory::create_transaction(
        relayer_model,
        signer_model,
        state.relayer_repository(),
        state.network_repository(),
        state.transaction_repository(),
        state.transaction_counter_store(),
        state.job_producer(),
    )
    .await
    .map_err(|e| e.into())
}

/// Creates a relayer network transaction using a relayer model.
///
/// # Arguments
///
/// * `relayer_model` - A `RelayerRepoModel` representing the relayer.
/// * `state` - A reference to the application state, wrapped in `ThinData`.
///
/// # Returns
///
/// A `Result` containing a `NetworkTransaction` if successful, or an `ApiError` if an error occurs.
pub async fn get_relayer_transaction_by_model(
    relayer_model: RelayerRepoModel,
    state: &ThinData<DefaultAppState>,
) -> Result<NetworkTransaction, ApiError> {
    let signer_model = state
        .signer_repository
        .get_by_id(relayer_model.signer_id.clone())
        .await?;

    RelayerTransactionFactory::create_transaction(
        relayer_model,
        signer_model,
        state.relayer_repository(),
        state.network_repository(),
        state.transaction_repository(),
        state.transaction_counter_store(),
        state.job_producer(),
    )
    .await
    .map_err(|e| e.into())
}

/// Returns an error indicating that Solana relayers are not supported.
///
/// # Returns
///
/// A `Result` that always contains a `TransactionError::NotSupported` error.
pub fn solana_not_supported_transaction<T>() -> Result<T, TransactionError> {
    Err(TransactionError::NotSupported(
        "Endpoint is not supported for Solana relayers".to_string(),
    ))
}
