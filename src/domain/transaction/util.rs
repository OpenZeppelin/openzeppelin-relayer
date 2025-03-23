//! This module provides utility functions for handling transactions within the application.
//!
//! It includes functions to retrieve transactions by ID, create relayer transactions, and
//! handle unsupported operations for specific relayers. The module interacts with various
//! repositories and factories to perform these operations.
use actix_web::web::ThinData;

use crate::{
    domain::get_relayer_by_id,
    jobs::JobProducer,
    models::{ApiError, AppState, RelayerRepoModel, TransactionError, TransactionRepoModel},
    repositories::Repository,
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
pub async fn get_transaction_by_id(
    transaction_id: String,
    state: &ThinData<AppState<JobProducer>>,
) -> Result<TransactionRepoModel, ApiError> {
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
    state: &ThinData<AppState<JobProducer>>,
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
        state.transaction_repository(),
        state.transaction_counter_store(),
        state.job_producer(),
    )
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
    state: &ThinData<AppState<JobProducer>>,
) -> Result<NetworkTransaction, ApiError> {
    let signer_model = state
        .signer_repository
        .get_by_id(relayer_model.signer_id.clone())
        .await?;

    RelayerTransactionFactory::create_transaction(
        relayer_model,
        signer_model,
        state.relayer_repository(),
        state.transaction_repository(),
        state.transaction_counter_store(),
        state.job_producer(),
    )
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
