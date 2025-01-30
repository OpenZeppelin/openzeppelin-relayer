use actix_web::web::ThinData;

use crate::{
    domain::get_relayer_by_id,
    models::{RelayerRepoModel, TransactionRepoModel},
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

    RelayerTransactionFactory::create_transaction(
        relayer_model,
        state.relayer_repository(),
        state.transaction_repository(),
        state.job_producer(),
    )
    .map_err(|e| e.into())
}

pub async fn get_relayer_transaction_by_model(
    relayer_model: RelayerRepoModel,
    state: &ThinData<AppState>,
) -> Result<NetworkTransaction, ApiError> {
    RelayerTransactionFactory::create_transaction(
        relayer_model,
        state.relayer_repository(),
        state.transaction_repository(),
        state.job_producer(),
    )
    .map_err(|e| e.into())
}
