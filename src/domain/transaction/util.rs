use actix_web::web::ThinData;

use crate::{
    domain::get_relayer_by_id,
    models::{RelayerRepoModel, TransactionError, TransactionRepoModel},
    repositories::Repository,
    ApiError, AppState,
};

use super::{NetworkTransaction, RelayerTransactionFactory};

pub async fn get_transaction_by_id(
    transaction_id: String,
    state: &ThinData<AppState>,
) -> Result<TransactionRepoModel, ApiError> {
    state
        .transaction_repository
        .get_by_id(transaction_id)
        .await
        .map_err(|e| e.into())
}

pub async fn get_relayer_transaction(
    relayer_id: String,
    state: &ThinData<AppState>,
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

pub async fn get_relayer_transaction_by_model(
    relayer_model: RelayerRepoModel,
    state: &ThinData<AppState>,
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

pub fn solana_not_supported<T>() -> Result<T, TransactionError> {
    Err(TransactionError::NotSupported(
        "Endpoint is not supported for Solana relayers".to_string(),
    ))
}
