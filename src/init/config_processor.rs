//! This module provides functionality for processing configuration files and populating
//! repositories.
use crate::{
    config::{Config, KeyLoaderTrait, SignerConfig as ConfigFileSignerConfig},
    models::{
        AppState, AwsKmsSignerConfig, LocalSignerConfig, NotificationRepoModel, RelayerRepoModel,
        SignerConfig, SignerRepoModel, TestSignerConfig, VaultSignerConfig,
    },
    repositories::Repository,
    services::{Signer, SignerFactory},
    utils::unsafe_generate_random_private_key,
};

use actix_web::web::ThinData;
use color_eyre::{eyre::WrapErr, Report, Result};
use futures::future::try_join_all;

async fn process_signers(config_file: &Config, app_state: &ThinData<AppState>) -> Result<()> {
    let signer_futures = config_file.signers.iter().map(|signer| async {
        let signer_repo_model = match &signer.config {
            ConfigFileSignerConfig::Test(_) => SignerRepoModel {
                id: signer.id.clone(),
                config: SignerConfig::Test(TestSignerConfig {
                    raw_key: unsafe_generate_random_private_key(),
                }),
            },
            ConfigFileSignerConfig::Local(local_signer) => SignerRepoModel {
                id: signer.id.clone(),
                config: SignerConfig::Local(LocalSignerConfig {
                    raw_key: local_signer.load_key().await?,
                }),
            },
            ConfigFileSignerConfig::AwsKms(_) => SignerRepoModel {
                id: signer.id.clone(),
                config: SignerConfig::AwsKms(AwsKmsSignerConfig {}),
            },
            ConfigFileSignerConfig::Vault(vault_config) => SignerRepoModel {
                id: signer.id.clone(),
                config: SignerConfig::Vault(VaultSignerConfig {
                    address: vault_config.address.clone(),
                    namespace: vault_config.namespace.clone(),
                    role_id: vault_config.role_id.clone(),
                    secret_id: vault_config.secret_id.clone(),
                    key_name: vault_config.key_name.clone(),
                }),
            },
        };

        app_state
            .signer_repository
            .create(signer_repo_model)
            .await
            .wrap_err("Failed to create signer repository entry")?;
        Ok::<(), Report>(())
    });

    try_join_all(signer_futures)
        .await
        .wrap_err("Failed to initialize signer repository")?;
    Ok(())
}

async fn process_notifications(config_file: &Config, app_state: &ThinData<AppState>) -> Result<()> {
    let notification_futures = config_file.notifications.iter().map(|notification| async {
        let notification_repo_model = NotificationRepoModel::try_from(notification.clone())
            .wrap_err("Failed to convert notification config")?;

        app_state
            .notification_repository
            .create(notification_repo_model)
            .await
            .wrap_err("Failed to create notification repository entry")?;
        Ok::<(), Report>(())
    });

    try_join_all(notification_futures)
        .await
        .wrap_err("Failed to initialize notification repository")?;
    Ok(())
}

async fn process_relayers(config_file: &Config, app_state: &ThinData<AppState>) -> Result<()> {
    let signers = app_state.signer_repository.list_all().await?;

    let relayer_futures = config_file.relayers.iter().map(|relayer| async {
        let mut repo_model = RelayerRepoModel::try_from(relayer.clone())
            .wrap_err("Failed to convert relayer config")?;
        let signer_model = signers
            .iter()
            .find(|s| s.id == repo_model.signer_id)
            .ok_or_else(|| eyre::eyre!("Signer not found"))?;
        let network_type = repo_model.network_type;
        let signer_service = SignerFactory::create_signer(&network_type, signer_model)
            .wrap_err("Failed to create signer service")?;

        let address = signer_service.address().await?;
        repo_model.address = address.to_string();

        app_state
            .relayer_repository
            .create(repo_model)
            .await
            .wrap_err("Failed to create relayer repository entry")?;
        Ok::<(), Report>(())
    });

    try_join_all(relayer_futures)
        .await
        .wrap_err("Failed to initialize relayer repository")?;
    Ok(())
}

pub async fn process_config_file(config_file: Config, app_state: ThinData<AppState>) -> Result<()> {
    process_signers(&config_file, &app_state).await?;
    process_notifications(&config_file, &app_state).await?;
    process_relayers(&config_file, &app_state).await?;
    Ok(())
}
