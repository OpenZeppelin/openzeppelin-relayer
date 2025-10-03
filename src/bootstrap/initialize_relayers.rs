//! Relayer initialization
//!
//! This module contains functions for initializing relayers, ensuring they are
//! properly configured and ready for operation.
use crate::{
    domain::{get_network_relayer, Relayer},
    jobs::JobProducerTrait,
    models::{
        NetworkRepoModel, NotificationRepoModel, RelayerRepoModel, SignerRepoModel,
        ThinDataAppState, TransactionRepoModel,
    },
    repositories::{
        ApiKeyRepositoryTrait, NetworkRepository, PluginRepositoryTrait, RelayerRepository,
        Repository, TransactionCounterTrait, TransactionRepository,
    },
};
use color_eyre::{eyre::WrapErr, Report, Result};
use futures::future::try_join_all;
use tracing::debug;

pub async fn initialize_relayer<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    relayer_id: String,
    app_state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
) -> Result<()>
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
    let relayer_service = get_network_relayer(relayer_id.clone(), &app_state).await?;

    debug!(relayer_id = %relayer_id, "initializing relayer");

    relayer_service.initialize_relayer().await?;

    Ok::<(), Report>(())
}

pub async fn initialize_relayers<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    app_state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
) -> Result<()>
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
    let relayers = app_state.relayer_repository.list_all().await?;

    let relayer_futures = relayers.iter().map(|relayer| {
        let app_state = app_state.clone();
        async move { initialize_relayer(relayer.id.clone(), app_state).await }
    });

    try_join_all(relayer_futures)
        .await
        .wrap_err("Failed to initialize relayers")?;
    Ok(())
}
