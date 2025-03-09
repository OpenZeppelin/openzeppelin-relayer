//! This module provides functionality for processing configuration files and populating
//! repositories.
use std::path::Path;

use crate::{
    config::{Config, SignerConfig as ConfigFileSignerConfig},
    models::{
        AppState, AwsKmsSignerConfig, LocalSignerConfig, NotificationRepoModel, RelayerRepoModel,
        SignerConfig, SignerRepoModel, VaultTransitSignerConfig,
    },
    repositories::Repository,
    services::{Signer, SignerFactory, VaultConfig, VaultService, VaultServiceTrait},
    utils::unsafe_generate_random_private_key,
};
use actix_web::web::ThinData;
use color_eyre::{eyre::WrapErr, Report, Result};
use futures::future::try_join_all;
use oz_keystore::{HashicorpCloudClient, LocalClient};
use reqwest::Client;

async fn process_signers(config_file: &Config, app_state: &ThinData<AppState>) -> Result<()> {
    let signer_futures = config_file.signers.iter().map(|signer| async {
        let signer_repo_model = match &signer.config {
            ConfigFileSignerConfig::Test(_) => SignerRepoModel {
                id: signer.id.clone(),
                config: SignerConfig::Test(LocalSignerConfig {
                    raw_key: unsafe_generate_random_private_key(),
                }),
            },
            ConfigFileSignerConfig::Local(local_signer) => {
                let passphrase = local_signer.passphrase.get_value()?;
                let raw_key =
                    LocalClient::load(Path::new(&local_signer.path).to_path_buf(), passphrase);
                SignerRepoModel {
                    id: signer.id.clone(),
                    config: SignerConfig::Local(LocalSignerConfig { raw_key }),
                }
            }
            ConfigFileSignerConfig::AwsKms(_) => SignerRepoModel {
                id: signer.id.clone(),
                config: SignerConfig::AwsKms(AwsKmsSignerConfig {}),
            },
            ConfigFileSignerConfig::Vault(vault_config) => {
                let config = VaultConfig {
                    address: vault_config.address.clone(),
                    namespace: vault_config.namespace.clone(),
                    role_id: vault_config.role_id.clone(),
                    secret_id: vault_config.secret_id.clone(),
                };

                let client = Client::new();
                let vault_service = VaultService { config, client };
                let secret = vault_service
                    .retrieve_secret(&vault_config.key_name)
                    .await?;
                let raw_key = hex::decode(&secret)?;

                SignerRepoModel {
                    id: signer.id.clone(),
                    config: SignerConfig::Vault(LocalSignerConfig { raw_key }),
                }
            }
            ConfigFileSignerConfig::VaultCloud(vault_cloud_config) => {
                let client = HashicorpCloudClient::new(
                    vault_cloud_config.client_id.clone(),
                    vault_cloud_config.client_secret.clone(),
                    vault_cloud_config.org_id.clone(),
                    vault_cloud_config.project_id.clone(),
                    vault_cloud_config.app_name.clone(),
                );

                let response = client.get_secret(&vault_cloud_config.key_name).await?;
                let raw_key = hex::decode(&response.secret.static_version.value)?;

                SignerRepoModel {
                    id: signer.id.clone(),
                    config: SignerConfig::Vault(LocalSignerConfig { raw_key }),
                }
            }
            ConfigFileSignerConfig::VaultTransit(vault_transit_config) => SignerRepoModel {
                id: signer.id.clone(),
                config: SignerConfig::VaultTransit(VaultTransitSignerConfig {
                    key_name: vault_transit_config.key_name.clone(),
                    address: vault_transit_config.address.clone(),
                    namespace: vault_transit_config.namespace.clone(),
                    token: vault_transit_config.token.clone(),
                    pubkey: vault_transit_config.pubkey.clone(),
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
