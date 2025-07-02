//! Relayer initialization
//!
//! This module contains functions for initializing relayers, ensuring they are
//! properly configured and ready for operation.
use crate::{
    domain::{get_network_relayer, Relayer},
    models::DefaultAppState,
    repositories::Repository,
};
use actix_web::web::ThinData;

use color_eyre::{eyre::WrapErr, Report, Result};
use futures::future::try_join_all;
use log::info;

async fn initialize_relayer(
    relayer_id: String,
    app_state: ThinData<DefaultAppState>,
) -> Result<()> {
    let relayer_service = get_network_relayer(relayer_id.clone(), &app_state).await?;

    info!("Initializing relayer: {}", relayer_id.clone());

    relayer_service.initialize_relayer().await?;

    Ok::<(), Report>(())
}

pub async fn initialize_relayers(app_state: ThinData<DefaultAppState>) -> Result<()> {
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
