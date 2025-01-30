use actix_web::web::ThinData;

use crate::{
    domain::{RelayerFactory, RelayerFactoryTrait},
    models::RelayerRepoModel,
    repositories::Repository,
    ApiError, AppState,
};

use super::NetworkRelayer;
pub async fn get_relayer_by_id(
    relayer_id: String,
    state: &ThinData<AppState>,
) -> Result<RelayerRepoModel, ApiError> {
    state
        .relayer_repository
        .get_by_id(relayer_id)
        .await
        .map_err(|e| e.into())
}

pub async fn get_network_relayer(
    relayer_id: String,
    state: &ThinData<AppState>,
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
        state.job_producer(),
    )
    .map_err(|e| e.into())
}

pub async fn get_network_relayer_by_model(
    relayer_model: RelayerRepoModel,
    state: &ThinData<AppState>,
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
        state.job_producer(),
    )
    .map_err(|e| e.into())
}
