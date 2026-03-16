//! This module provides functionality for processing configuration files and populating
//! repositories.
//!
//! ## Distributed Locking for Config Processing
//!
//! When multiple instances of the relayer service start simultaneously with Redis storage
//! and `DISTRIBUTED_MODE` is enabled, this module uses distributed locking to coordinate
//! config processing and prevent race conditions:
//!
//! - **Global lock**: A single lock is used for the entire config processing,
//!   ensuring only one instance processes the config at a time.
//! - **Post-lock population check**: After acquiring the lock, checks if Redis is already
//!   populated (by another instance that held the lock first), and skips if so.
//! - **Single-instance mode**: When `DISTRIBUTED_MODE` is disabled (default) or using
//!   in-memory storage, locking is skipped since coordination is not needed.
use std::sync::Arc;
use std::time::Duration;

use crate::{
    config::{Config, RepositoryStorageType, ServerConfig},
    jobs::JobProducerTrait,
    models::{
        ApiKeyRepoModel, NetworkRepoModel, NotificationRepoModel, PluginModel, Relayer,
        RelayerRepoModel, Signer as SignerDomainModel, SignerFileConfig, SignerRepoModel,
        ThinDataAppState, TransactionRepoModel,
    },
    repositories::{
        ApiKeyRepositoryTrait, NetworkRepository, PluginRepositoryTrait, RelayerRepository,
        Repository, TransactionCounterTrait, TransactionRepository,
    },
    services::signer::{Signer as SignerService, SignerFactory},
    utils::{
        is_config_processing_completed, is_config_processing_in_progress, poll_until,
        set_config_processing_completed, set_config_processing_in_progress, DistributedLock,
        BOOTSTRAP_LOCK_TTL_SECS, LOCK_POLL_INTERVAL_MS, LOCK_WAIT_MAX_SECS,
    },
};
use color_eyre::{eyre::WrapErr, Report, Result};
use deadpool_redis::Pool;
use futures::future::try_join_all;
use tracing::{info, warn};

/// Lock name for config processing lock.
const CONFIG_PROCESSING_LOCK_NAME: &str = "config_processing";

#[derive(Debug, PartialEq, Eq)]
enum ConfigBootstrapState {
    Empty,
    Complete,
    Incomplete { missing: Vec<String> },
}

async fn process_api_key<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    server_config: &ServerConfig,
    app_state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
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
    let api_key_model = ApiKeyRepoModel::new(
        "default".to_string(),
        server_config.api_key.clone(),
        vec!["*".to_string()],
        vec!["*".to_string()],
    );

    app_state
        .api_key_repository
        .create(api_key_model)
        .await
        .wrap_err("Failed to create api key repository entry")?;

    Ok(())
}

/// Process all plugins from the config file and store them in the repository.
async fn process_plugins<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    config_file: &Config,
    app_state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
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
    if let Some(plugins) = &config_file.plugins {
        let plugin_futures = plugins.iter().map(|plugin| async {
            let plugin_model = PluginModel::try_from(plugin.clone())
                .wrap_err("Failed to convert plugin config")?;
            app_state
                .plugin_repository
                .add(plugin_model)
                .await
                .wrap_err("Failed to create plugin repository entry")?;
            Ok::<(), Report>(())
        });

        try_join_all(plugin_futures)
            .await
            .wrap_err("Failed to initialize plugin repository")?;
        Ok(())
    } else {
        Ok(())
    }
}

/// Process a signer configuration from the config file and convert it into a `SignerRepoModel`.
async fn process_signer(signer: &SignerFileConfig) -> Result<SignerRepoModel> {
    // Convert config to domain model (this validates and applies business logic)
    let domain_signer = SignerDomainModel::try_from(signer.clone())
        .wrap_err("Failed to convert signer config to domain model")?;

    // Convert domain model to repository model for storage
    let signer_repo_model = SignerRepoModel::from(domain_signer);

    Ok(signer_repo_model)
}

/// Process all signers from the config file and store them in the repository.
///
/// For each signer in the config file:
/// 1. Process it using `process_signer` (config -> domain -> repository)
/// 2. Store the resulting repository model
///
/// This function processes signers in parallel using futures.
async fn process_signers<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    config_file: &Config,
    app_state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
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
    let signer_futures = config_file.signers.iter().map(|signer| async {
        let signer_repo_model = process_signer(signer).await?;

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

/// Process all notification configurations from the config file and store them in the repository.
///
/// For each notification in the config file:
/// 1. Convert it to a repository model
/// 2. Store the resulting model in the repository
///
/// This function processes notifications in parallel using futures.
async fn process_notifications<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    config_file: &Config,
    app_state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
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

/// Process all network configurations from the config file and store them in the repository.
///
/// For each network in the config file:
/// 1. Convert it to a repository model using TryFrom
/// 2. Store the resulting model in the repository
///
/// This function processes networks in parallel using futures.
async fn process_networks<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    config_file: &Config,
    app_state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
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
    let network_futures = config_file.networks.iter().map(|network| async move {
        let network_repo_model = NetworkRepoModel::try_from(network.clone())?;

        app_state
            .network_repository
            .create(network_repo_model)
            .await
            .wrap_err("Failed to create network repository entry")?;
        Ok::<(), Report>(())
    });

    try_join_all(network_futures)
        .await
        .wrap_err("Failed to initialize network repository")?;
    Ok(())
}

/// Process all relayer configurations from the config file and store them in the repository.
///
/// For each relayer in the config file:
/// 1. Convert it to a repository model
/// 2. Retrieve the associated signer
/// 3. Create a signer service
/// 4. Get the signer's address and add it to the relayer model
/// 5. Store the resulting model in the repository
///
/// This function processes relayers in parallel using futures.
async fn process_relayers<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    config_file: &Config,
    app_state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
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
    let signers = app_state.signer_repository.list_all().await?;

    let relayer_futures = config_file.relayers.iter().map(|relayer| async {
        // Convert config to domain model first, then to repository model
        let domain_relayer = Relayer::try_from(relayer.clone())
            .wrap_err("Failed to convert relayer config to domain model")?;
        let mut repo_model = RelayerRepoModel::from(domain_relayer);
        let signer_model = signers
            .iter()
            .find(|s| s.id == repo_model.signer_id)
            .ok_or_else(|| eyre::eyre!("Signer not found"))?;

        let network_type = repo_model.network_type;
        let signer_service = SignerFactory::create_signer(
            &network_type,
            &SignerDomainModel::from(signer_model.clone()),
        )
        .await
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

async fn validate_config_bootstrap_state<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    config_file: &Config,
    app_state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
) -> Result<ConfigBootstrapState>
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
    let mut missing = Vec::new();
    let relayer_has_entries = app_state.relayer_repository.has_entries().await?;
    let signer_has_entries = app_state.signer_repository.has_entries().await?;
    let notification_has_entries = app_state.notification_repository.has_entries().await?;
    let network_has_entries = app_state.network_repository.has_entries().await?;
    let plugin_has_entries = app_state.plugin_repository.has_entries().await?;
    let api_key_has_entries = app_state.api_key_repository.has_entries().await?;

    let has_any_entries = relayer_has_entries
        || signer_has_entries
        || notification_has_entries
        || network_has_entries
        || plugin_has_entries
        || api_key_has_entries;

    if !has_any_entries {
        return Ok(ConfigBootstrapState::Empty);
    }

    if config_file
        .plugins
        .as_ref()
        .is_some_and(|plugins| !plugins.is_empty())
        && !plugin_has_entries
    {
        missing.push("plugin repository".to_string());
    }

    if !config_file.signers.is_empty() && !signer_has_entries {
        missing.push("signer repository".to_string());
    }

    if !config_file.notifications.is_empty() && !notification_has_entries {
        missing.push("notification repository".to_string());
    }

    if !config_file.networks.is_empty() && !network_has_entries {
        missing.push("network repository".to_string());
    }

    if !config_file.relayers.is_empty() && !relayer_has_entries {
        missing.push("relayer repository".to_string());
    }

    if !api_key_has_entries {
        missing.push("api key repository".to_string());
    }

    if missing.is_empty() {
        Ok(ConfigBootstrapState::Complete)
    } else {
        Ok(ConfigBootstrapState::Incomplete { missing })
    }
}

fn format_incomplete_bootstrap_error(missing: &[String]) -> Report {
    eyre::eyre!(
        "Redis contains incomplete bootstrap-managed config state without completion marker (missing: {})",
        missing.join(", ")
    )
}

/// Process a complete configuration file by initializing all repositories.
///
/// This function processes the entire configuration file in the following order:
/// 1. Process plugins
/// 2. Process signers
/// 3. Process notifications
/// 4. Process networks
/// 5. Process relayers
/// 6. Process API key
///
/// When using Redis storage with `DISTRIBUTED_MODE` enabled, this function uses distributed
/// locking to prevent race conditions when multiple instances start simultaneously
/// (especially with `RESET_STORAGE_ON_START=true`).
pub async fn process_config_file<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    config_file: Config,
    server_config: Arc<ServerConfig>,
    app_state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
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
    match server_config.repository_storage_type {
        RepositoryStorageType::InMemory => {
            // In-memory mode: no locking needed, process directly
            execute_config_processing(&config_file, &server_config, app_state).await
        }
        RepositoryStorageType::Redis => {
            // Check if distributed locking is needed
            let use_lock = ServerConfig::get_distributed_mode();
            let connection_info = app_state.relayer_repository.connection_info();

            match (use_lock, connection_info) {
                (true, Some((conn, prefix))) => {
                    // Distributed mode: use locking to coordinate across instances
                    coordinate_config_with_lock(
                        &config_file,
                        &server_config,
                        app_state,
                        &conn,
                        &prefix,
                    )
                    .await
                }
                _ => {
                    // Single-instance mode or no connection info: validate state directly.
                    if server_config.reset_storage_on_start {
                        return execute_config_processing(&config_file, &server_config, app_state)
                            .await;
                    }

                    match validate_config_bootstrap_state(&config_file, app_state).await? {
                        ConfigBootstrapState::Empty => {
                            execute_config_processing(&config_file, &server_config, app_state).await
                        }
                        ConfigBootstrapState::Complete => {
                            info!(
                                "Skipping config file processing - bootstrap-managed Redis state is complete"
                            );
                            Ok(())
                        }
                        ConfigBootstrapState::Incomplete { missing } => {
                            Err(format_incomplete_bootstrap_error(&missing))
                        }
                    }
                }
            }
        }
    }
}

/// Process config file with distributed locking for Redis storage.
///
/// Flow:
/// 1. Try to acquire global lock for config processing
/// 2. If lock acquired: check for an explicit completion marker, process if needed
/// 3. If lock held: wait for the completion marker to appear
/// 4. If wait times out: recheck state and attempt takeover after lock expiry
async fn coordinate_config_with_lock<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    config_file: &Config,
    server_config: &ServerConfig,
    app_state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    conn: &Arc<Pool>,
    prefix: &str,
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
    let lock_key = format!("{prefix}:lock:{CONFIG_PROCESSING_LOCK_NAME}");
    let lock = DistributedLock::new(
        conn.clone(),
        &lock_key,
        Duration::from_secs(BOOTSTRAP_LOCK_TTL_SECS),
    );

    match lock.try_acquire().await {
        Ok(Some(guard)) => {
            // We got the lock - check if we need to process
            info!("Acquired config processing lock");

            let result =
                process_if_needed_after_lock(config_file, server_config, app_state, conn, prefix)
                    .await;

            drop(guard); // Release lock
            result
        }
        Ok(None) => {
            // Lock held by another instance - wait for it to complete
            info!("Another instance is processing config, waiting for completion");
            let completed = wait_for_config_processing_complete(conn, prefix).await?;

            if completed {
                return Ok(());
            }

            warn!("Timeout waiting for config processing, rechecking state");

            if is_config_processing_completed(conn, prefix)
                .await
                .unwrap_or(false)
            {
                info!("Config processing completed during timeout window");
                return Ok(());
            }

            recover_config_processing_after_timeout(
                config_file,
                server_config,
                app_state,
                conn,
                prefix,
            )
            .await
        }
        Err(e) => Err(eyre::eyre!(
            "Failed to acquire config processing lock in distributed mode: {}",
            e
        )),
    }
}

/// Process config after successfully acquiring the lock.
///
/// Checks if config processing was already completed and only processes if needed.
async fn process_if_needed_after_lock<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    config_file: &Config,
    server_config: &ServerConfig,
    app_state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    conn: &Arc<Pool>,
    prefix: &str,
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
    let already_completed = is_config_processing_completed(conn, prefix).await?;
    let in_progress = is_config_processing_in_progress(conn, prefix).await?;
    let bootstrap_state = validate_config_bootstrap_state(config_file, app_state).await?;

    if server_config.reset_storage_on_start {
        // With reset flag: always reset and process (we have the lock)
        execute_config_processing_with_marker(config_file, server_config, app_state, conn, prefix)
            .await
    } else if already_completed {
        // No reset flag and already completed: skip
        info!("Config processing already completed, skipping config file processing");
        Ok(())
    } else {
        match bootstrap_state {
            ConfigBootstrapState::Empty => {
                execute_config_processing_with_marker(
                    config_file,
                    server_config,
                    app_state,
                    conn,
                    prefix,
                )
                .await
            }
            ConfigBootstrapState::Complete if !in_progress => {
                info!(
                    "Bootstrap-managed Redis state is complete without marker, backfilling completion marker"
                );
                set_config_processing_completed(conn, prefix).await?;
                Ok(())
            }
            ConfigBootstrapState::Complete => {
                info!("Bootstrap-managed Redis state is complete, restoring completion marker");
                set_config_processing_completed(conn, prefix).await?;
                Ok(())
            }
            ConfigBootstrapState::Incomplete { missing } => {
                Err(format_incomplete_bootstrap_error(&missing))
            }
        }
    }
}

/// Waits for another instance to complete config processing.
///
/// Polls periodically until the explicit completion marker is set or timeout is reached.
async fn wait_for_config_processing_complete(conn: &Arc<Pool>, prefix: &str) -> Result<bool> {
    let max_wait = Duration::from_secs(LOCK_WAIT_MAX_SECS);
    let poll_interval = Duration::from_millis(LOCK_POLL_INTERVAL_MS);

    let conn = conn.clone();
    let prefix = prefix.to_string();

    let completed = poll_until(
        || is_config_processing_completed(&conn, &prefix),
        max_wait,
        poll_interval,
        "config processing",
    )
    .await?;

    Ok(completed)
}

/// Attempts to recover config processing after a wait timeout.
///
/// This is the config-processing analogue to relayer initialization recovery:
/// if the original lock holder died after setting the in-progress marker but
/// before completion, a waiting instance should take over once the lock TTL
/// expires instead of failing the whole rollout.
async fn recover_config_processing_after_timeout<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    config_file: &Config,
    server_config: &ServerConfig,
    app_state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    conn: &Arc<Pool>,
    prefix: &str,
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
    let lock_key = format!("{prefix}:lock:{CONFIG_PROCESSING_LOCK_NAME}");
    let recovery_lock = DistributedLock::new(
        conn.clone(),
        &lock_key,
        Duration::from_secs(BOOTSTRAP_LOCK_TTL_SECS),
    );

    match recovery_lock.try_acquire().await {
        Ok(Some(guard)) => {
            warn!("Previous config-processing lock holder appears to have crashed, taking over");
            let result =
                process_if_needed_after_lock(config_file, server_config, app_state, conn, prefix)
                    .await;
            drop(guard);
            result
        }
        Ok(None) => {
            // Another instance may still be processing config.
            // Wait one more bounded period for the explicit completion marker
            // before giving up on this instance.
            warn!("Config-processing lock still held after timeout, waiting again for completion");
            let completed = wait_for_config_processing_complete(conn, prefix).await?;

            if completed {
                info!("Config processing completed by another instance during extended wait");
                Ok(())
            } else {
                Err(eyre::eyre!(
                    "Timed out waiting for config processing and could not acquire recovery lock"
                ))
            }
        }
        Err(e) => {
            warn!(
                error = %e,
                "Failed to acquire recovery lock for config processing"
            );
            Err(e)
        }
    }
}

/// Internal function that performs the actual config processing.
async fn execute_config_processing_with_marker<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    config_file: &Config,
    server_config: &ServerConfig,
    app_state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    conn: &Arc<Pool>,
    prefix: &str,
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
    set_config_processing_in_progress(conn, prefix).await?;

    let result = execute_config_processing(config_file, server_config, app_state).await;

    if result.is_ok() {
        set_config_processing_completed(conn, prefix).await?;
    }

    result
}

/// Internal function that performs the actual config processing work.
async fn execute_config_processing<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    config_file: &Config,
    server_config: &ServerConfig,
    app_state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
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
    if server_config.reset_storage_on_start {
        info!("Resetting storage on start due to server config flag RESET_STORAGE_ON_START = true");
        app_state.relayer_repository.drop_all_entries().await?;
        app_state.transaction_repository.drop_all_entries().await?;
        app_state.signer_repository.drop_all_entries().await?;
        app_state.notification_repository.drop_all_entries().await?;
        app_state.network_repository.drop_all_entries().await?;
        app_state.plugin_repository.drop_all_entries().await?;
        app_state.api_key_repository.drop_all_entries().await?;
    }

    info!("Processing config file");
    process_plugins(config_file, app_state).await?;
    process_signers(config_file, app_state).await?;
    process_notifications(config_file, app_state).await?;
    process_networks(config_file, app_state).await?;
    process_relayers(config_file, app_state).await?;
    process_api_key(server_config, app_state).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{ConfigFileNetworkType, NetworksFileConfig, PluginFileConfig},
        constants::DEFAULT_PLUGIN_TIMEOUT_SECONDS,
        jobs::MockJobProducerTrait,
        models::{
            relayer::RelayerFileConfig, AppState, AwsKmsSignerFileConfig,
            GoogleCloudKmsKeyFileConfig, GoogleCloudKmsServiceAccountFileConfig,
            GoogleCloudKmsSignerFileConfig, LocalSignerFileConfig, NetworkType, NotificationConfig,
            NotificationType, PaginationQuery, PlainOrEnvValue, SecretString, SignerConfigStorage,
            SignerFileConfig, SignerFileConfigEnum, VaultSignerFileConfig,
            VaultTransitSignerFileConfig,
        },
        repositories::{
            ApiKeyRepositoryStorage, InMemoryApiKeyRepository, InMemoryNetworkRepository,
            InMemoryNotificationRepository, InMemoryPluginRepository, InMemorySignerRepository,
            InMemoryTransactionCounter, InMemoryTransactionRepository, NetworkRepositoryStorage,
            NotificationRepositoryStorage, PluginRepositoryStorage, RelayerRepositoryStorage,
            SignerRepositoryStorage, TransactionCounterRepositoryStorage,
            TransactionRepositoryStorage,
        },
        utils::mocks::mockutils::{
            create_mock_relayer, create_mock_signer, create_test_server_config,
        },
    };
    use actix_web::web::ThinData;
    use mockito;
    use serde_json::json;
    use std::{sync::Arc, time::Duration};

    fn create_test_app_state() -> AppState<
        MockJobProducerTrait,
        RelayerRepositoryStorage,
        TransactionRepositoryStorage,
        NetworkRepositoryStorage,
        NotificationRepositoryStorage,
        SignerRepositoryStorage,
        TransactionCounterRepositoryStorage,
        PluginRepositoryStorage,
        ApiKeyRepositoryStorage,
    > {
        // Create a mock job producer
        let mut mock_job_producer = MockJobProducerTrait::new();

        // Set up expectations for the mock
        mock_job_producer
            .expect_produce_transaction_request_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        mock_job_producer
            .expect_produce_submit_transaction_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        mock_job_producer
            .expect_produce_check_transaction_status_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        mock_job_producer
            .expect_produce_send_notification_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        AppState {
            relayer_repository: Arc::new(RelayerRepositoryStorage::new_in_memory()),
            transaction_repository: Arc::new(TransactionRepositoryStorage::new_in_memory()),
            signer_repository: Arc::new(SignerRepositoryStorage::new_in_memory()),
            notification_repository: Arc::new(NotificationRepositoryStorage::new_in_memory()),
            network_repository: Arc::new(NetworkRepositoryStorage::new_in_memory()),
            transaction_counter_store: Arc::new(
                TransactionCounterRepositoryStorage::new_in_memory(),
            ),
            job_producer: Arc::new(mock_job_producer),
            plugin_repository: Arc::new(PluginRepositoryStorage::new_in_memory()),
            api_key_repository: Arc::new(ApiKeyRepositoryStorage::new_in_memory()),
        }
    }

    #[tokio::test]
    async fn test_process_signer_test() {
        let signer = SignerFileConfig {
            id: "test-signer".to_string(),
            config: SignerFileConfigEnum::Local(LocalSignerFileConfig {
                path: "tests/utils/test_keys/unit-test-local-signer.json".to_string(),
                passphrase: PlainOrEnvValue::Plain {
                    value: SecretString::new("test"),
                },
            }),
        };

        let result = process_signer(&signer).await;

        assert!(
            result.is_ok(),
            "Failed to process test signer: {:?}",
            result.err()
        );
        let model = result.unwrap();

        assert_eq!(model.id, "test-signer");

        match model.config {
            SignerConfigStorage::Local(config) => {
                assert!(!config.raw_key.is_empty());
                assert_eq!(config.raw_key.len(), 32);
            }
            _ => panic!("Expected Local config"),
        }
    }

    #[tokio::test]
    async fn test_process_signer_vault_transit() -> Result<()> {
        let signer = SignerFileConfig {
            id: "vault-transit-signer".to_string(),
            config: SignerFileConfigEnum::VaultTransit(VaultTransitSignerFileConfig {
                key_name: "test-transit-key".to_string(),
                address: "https://vault.example.com".to_string(),
                namespace: Some("test-namespace".to_string()),
                role_id: PlainOrEnvValue::Plain {
                    value: SecretString::new("test-role"),
                },
                secret_id: PlainOrEnvValue::Plain {
                    value: SecretString::new("test-secret"),
                },
                pubkey: "test-pubkey".to_string(),
                mount_point: Some("transit".to_string()),
            }),
        };

        let result = process_signer(&signer).await;

        assert!(
            result.is_ok(),
            "Failed to process vault transit signer: {:?}",
            result.err()
        );
        let model = result.unwrap();

        assert_eq!(model.id, "vault-transit-signer");

        match model.config {
            SignerConfigStorage::VaultTransit(config) => {
                assert_eq!(config.key_name, "test-transit-key");
                assert_eq!(config.address, "https://vault.example.com");
                assert_eq!(config.namespace, Some("test-namespace".to_string()));
                assert_eq!(config.role_id.to_str().as_str(), "test-role");
                assert_eq!(config.secret_id.to_str().as_str(), "test-secret");
                assert_eq!(config.pubkey, "test-pubkey");
                assert_eq!(config.mount_point, Some("transit".to_string()));
            }
            _ => panic!("Expected VaultTransit config"),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_process_signer_aws_kms() -> Result<()> {
        let signer = SignerFileConfig {
            id: "aws-kms-signer".to_string(),
            config: SignerFileConfigEnum::AwsKms(AwsKmsSignerFileConfig {
                region: "us-east-1".to_string(),
                key_id: "test-key-id".to_string(),
            }),
        };

        let result = process_signer(&signer).await;

        assert!(
            result.is_ok(),
            "Failed to process AWS KMS signer: {:?}",
            result.err()
        );
        let model = result.unwrap();

        assert_eq!(model.id, "aws-kms-signer");

        match model.config {
            SignerConfigStorage::AwsKms(_) => {}
            _ => panic!("Expected AwsKms config"),
        }

        Ok(())
    }

    // utility function to setup a mock AppRole login response
    async fn setup_mock_approle_login(
        mock_server: &mut mockito::ServerGuard,
        role_id: &str,
        secret_id: &str,
        token: &str,
    ) -> mockito::Mock {
        mock_server
            .mock("POST", "/v1/auth/approle/login")
            .match_body(mockito::Matcher::Json(json!({
                "role_id": role_id,
                "secret_id": secret_id
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::to_string(&json!({
                    "request_id": "test-request-id",
                    "lease_id": "",
                    "renewable": false,
                    "lease_duration": 0,
                    "data": null,
                    "wrap_info": null,
                    "warnings": null,
                    "auth": {
                        "client_token": token,
                        "accessor": "test-accessor",
                        "policies": ["default"],
                        "token_policies": ["default"],
                        "metadata": {
                            "role_name": "test-role"
                        },
                        "lease_duration": 3600,
                        "renewable": true,
                        "entity_id": "test-entity-id",
                        "token_type": "service",
                        "orphan": true
                    }
                }))
                .unwrap(),
            )
            .create_async()
            .await
    }

    #[tokio::test]
    async fn test_process_signer_vault() -> Result<()> {
        let mut mock_server = mockito::Server::new_async().await;

        let _login_mock = setup_mock_approle_login(
            &mut mock_server,
            "test-role-id",
            "test-secret-id",
            "test-token",
        )
        .await;

        let _secret_mock = mock_server
            .mock("GET", "/v1/secret/data/test-key")
            .match_header("X-Vault-Token", "test-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_string(&json!({
                "request_id": "test-request-id",
                "lease_id": "",
                "renewable": false,
                "lease_duration": 0,
                "data": {
                    "data": {
                        "value": "C5ACE14AB163556747F02C1110911537578FBE335FB74D18FBF82990AD70C3B9"
                    },
                    "metadata": {
                        "created_time": "2024-01-01T00:00:00Z",
                        "deletion_time": "",
                        "destroyed": false,
                        "version": 1
                    }
                },
                "wrap_info": null,
                "warnings": null,
                "auth": null
            })).unwrap())
            .create_async()
            .await;

        let signer = SignerFileConfig {
            id: "vault-signer".to_string(),
            config: SignerFileConfigEnum::Vault(VaultSignerFileConfig {
                key_name: "test-key".to_string(),
                address: mock_server.url(),
                namespace: Some("test-namespace".to_string()),
                role_id: PlainOrEnvValue::Plain {
                    value: SecretString::new("test-role-id"),
                },
                secret_id: PlainOrEnvValue::Plain {
                    value: SecretString::new("test-secret-id"),
                },
                mount_point: Some("secret".to_string()),
            }),
        };

        let result = process_signer(&signer).await;

        assert!(
            result.is_ok(),
            "Failed to process Vault signer: {:?}",
            result.err()
        );
        let model = result.unwrap();

        assert_eq!(model.id, "vault-signer");

        match model.config {
            SignerConfigStorage::Vault(_) => {}
            _ => panic!("Expected Vault config"),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_process_signers() -> Result<()> {
        // Create test signers
        let signers = vec![
            SignerFileConfig {
                id: "test-signer-1".to_string(),
                config: SignerFileConfigEnum::Local(LocalSignerFileConfig {
                    path: "tests/utils/test_keys/unit-test-local-signer.json".to_string(),
                    passphrase: PlainOrEnvValue::Plain {
                        value: SecretString::new("test"),
                    },
                }),
            },
            SignerFileConfig {
                id: "test-signer-2".to_string(),
                config: SignerFileConfigEnum::Local(LocalSignerFileConfig {
                    path: "tests/utils/test_keys/unit-test-local-signer.json".to_string(),
                    passphrase: PlainOrEnvValue::Plain {
                        value: SecretString::new("test"),
                    },
                }),
            },
        ];

        // Create config
        let config = Config {
            signers,
            relayers: vec![],
            notifications: vec![],
            networks: NetworksFileConfig::new(vec![]).unwrap(),
            plugins: Some(vec![]),
        };

        // Create app state
        let app_state = ThinData(create_test_app_state());

        // Process signers
        process_signers(&config, &app_state).await?;

        // Verify signers were created
        let stored_signers = app_state.signer_repository.list_all().await?;
        assert_eq!(stored_signers.len(), 2);
        assert!(stored_signers.iter().any(|s| s.id == "test-signer-1"));
        assert!(stored_signers.iter().any(|s| s.id == "test-signer-2"));

        Ok(())
    }

    #[tokio::test]
    async fn test_process_notifications() -> Result<()> {
        // Create test notifications
        let notifications = vec![
            NotificationConfig {
                id: "test-notification-1".to_string(),
                r#type: NotificationType::Webhook,
                url: "https://hooks.slack.com/test1".to_string(),
                signing_key: None,
            },
            NotificationConfig {
                id: "test-notification-2".to_string(),
                r#type: NotificationType::Webhook,
                url: "https://hooks.slack.com/test2".to_string(),
                signing_key: None,
            },
        ];

        // Create config
        let config = Config {
            signers: vec![],
            relayers: vec![],
            notifications,
            networks: NetworksFileConfig::new(vec![]).unwrap(),
            plugins: Some(vec![]),
        };

        // Create app state
        let app_state = ThinData(create_test_app_state());

        // Process notifications
        process_notifications(&config, &app_state).await?;

        // Verify notifications were created
        let stored_notifications = app_state.notification_repository.list_all().await?;
        assert_eq!(stored_notifications.len(), 2);
        assert!(stored_notifications
            .iter()
            .any(|n| n.id == "test-notification-1"));
        assert!(stored_notifications
            .iter()
            .any(|n| n.id == "test-notification-2"));

        Ok(())
    }

    #[tokio::test]
    async fn test_process_networks_empty() -> Result<()> {
        let config = Config {
            signers: vec![],
            relayers: vec![],
            notifications: vec![],
            networks: NetworksFileConfig::new(vec![]).unwrap(),
            plugins: Some(vec![]),
        };

        let app_state = ThinData(create_test_app_state());

        process_networks(&config, &app_state).await?;

        let stored_networks = app_state.network_repository.list_all().await?;
        assert_eq!(stored_networks.len(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn test_process_networks_single_evm() -> Result<()> {
        use crate::config::network::test_utils::*;

        let networks = vec![create_evm_network_wrapped("mainnet")];

        let config = Config {
            signers: vec![],
            relayers: vec![],
            notifications: vec![],
            networks: NetworksFileConfig::new(networks).unwrap(),
            plugins: Some(vec![]),
        };

        let app_state = ThinData(create_test_app_state());

        process_networks(&config, &app_state).await?;

        let stored_networks = app_state.network_repository.list_all().await?;
        assert_eq!(stored_networks.len(), 1);
        assert_eq!(stored_networks[0].name, "mainnet");
        assert_eq!(stored_networks[0].network_type, NetworkType::Evm);

        Ok(())
    }

    #[tokio::test]
    async fn test_process_networks_single_solana() -> Result<()> {
        use crate::config::network::test_utils::*;

        let networks = vec![create_solana_network_wrapped("devnet")];

        let config = Config {
            signers: vec![],
            relayers: vec![],
            notifications: vec![],
            networks: NetworksFileConfig::new(networks).unwrap(),
            plugins: Some(vec![]),
        };

        let app_state = ThinData(create_test_app_state());

        process_networks(&config, &app_state).await?;

        let stored_networks = app_state.network_repository.list_all().await?;
        assert_eq!(stored_networks.len(), 1);
        assert_eq!(stored_networks[0].name, "devnet");
        assert_eq!(stored_networks[0].network_type, NetworkType::Solana);

        Ok(())
    }

    #[tokio::test]
    async fn test_process_networks_multiple_mixed() -> Result<()> {
        use crate::config::network::test_utils::*;

        let networks = vec![
            create_evm_network_wrapped("mainnet"),
            create_solana_network_wrapped("devnet"),
            create_evm_network_wrapped("sepolia"),
            create_solana_network_wrapped("testnet"),
        ];

        let config = Config {
            signers: vec![],
            relayers: vec![],
            notifications: vec![],
            networks: NetworksFileConfig::new(networks).unwrap(),
            plugins: Some(vec![]),
        };

        let app_state = ThinData(create_test_app_state());

        process_networks(&config, &app_state).await?;

        let stored_networks = app_state.network_repository.list_all().await?;
        assert_eq!(stored_networks.len(), 4);

        let evm_networks: Vec<_> = stored_networks
            .iter()
            .filter(|n| n.network_type == NetworkType::Evm)
            .collect();
        assert_eq!(evm_networks.len(), 2);
        assert!(evm_networks.iter().any(|n| n.name == "mainnet"));
        assert!(evm_networks.iter().any(|n| n.name == "sepolia"));

        let solana_networks: Vec<_> = stored_networks
            .iter()
            .filter(|n| n.network_type == NetworkType::Solana)
            .collect();
        assert_eq!(solana_networks.len(), 2);
        assert!(solana_networks.iter().any(|n| n.name == "devnet"));
        assert!(solana_networks.iter().any(|n| n.name == "testnet"));

        Ok(())
    }

    #[tokio::test]
    async fn test_process_networks_many_networks() -> Result<()> {
        use crate::config::network::test_utils::*;

        let networks = (0..10)
            .map(|i| create_evm_network_wrapped(&format!("network-{i}")))
            .collect();

        let config = Config {
            signers: vec![],
            relayers: vec![],
            notifications: vec![],
            networks: NetworksFileConfig::new(networks).unwrap(),
            plugins: Some(vec![]),
        };

        let app_state = ThinData(create_test_app_state());

        process_networks(&config, &app_state).await?;

        let stored_networks = app_state.network_repository.list_all().await?;
        assert_eq!(stored_networks.len(), 10);

        for i in 0..10 {
            let expected_name = format!("network-{i}");
            assert!(
                stored_networks.iter().any(|n| n.name == expected_name),
                "Network {expected_name} not found"
            );
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_process_networks_duplicate_names() -> Result<()> {
        use crate::config::network::test_utils::*;

        let networks = vec![
            create_evm_network_wrapped("mainnet"),
            create_solana_network_wrapped("mainnet"),
        ];

        let config = Config {
            signers: vec![],
            relayers: vec![],
            notifications: vec![],
            networks: NetworksFileConfig::new(networks).unwrap(),
            plugins: Some(vec![]),
        };

        let app_state = ThinData(create_test_app_state());

        process_networks(&config, &app_state).await?;

        let stored_networks = app_state.network_repository.list_all().await?;
        assert_eq!(stored_networks.len(), 2);

        let mainnet_networks: Vec<_> = stored_networks
            .iter()
            .filter(|n| n.name == "mainnet")
            .collect();
        assert_eq!(mainnet_networks.len(), 2);
        assert!(mainnet_networks
            .iter()
            .any(|n| n.network_type == NetworkType::Evm));
        assert!(mainnet_networks
            .iter()
            .any(|n| n.network_type == NetworkType::Solana));

        Ok(())
    }

    #[tokio::test]
    async fn test_process_networks() -> Result<()> {
        use crate::config::network::test_utils::*;

        let networks = vec![
            create_evm_network_wrapped("mainnet"),
            create_solana_network_wrapped("devnet"),
        ];

        let config = Config {
            signers: vec![],
            relayers: vec![],
            notifications: vec![],
            networks: NetworksFileConfig::new(networks).unwrap(),
            plugins: Some(vec![]),
        };

        let app_state = ThinData(create_test_app_state());

        process_networks(&config, &app_state).await?;

        let stored_networks = app_state.network_repository.list_all().await?;
        assert_eq!(stored_networks.len(), 2);
        assert!(stored_networks
            .iter()
            .any(|n| n.name == "mainnet" && n.network_type == NetworkType::Evm));
        assert!(stored_networks
            .iter()
            .any(|n| n.name == "devnet" && n.network_type == NetworkType::Solana));

        Ok(())
    }

    #[tokio::test]
    async fn test_process_relayers() -> Result<()> {
        // Create test signers
        let signers = vec![SignerFileConfig {
            id: "test-signer-1".to_string(),
            config: SignerFileConfigEnum::Local(LocalSignerFileConfig {
                path: "tests/utils/test_keys/unit-test-local-signer.json".to_string(),
                passphrase: PlainOrEnvValue::Plain {
                    value: SecretString::new("test"),
                },
            }),
        }];

        // Create test relayers
        let relayers = vec![RelayerFileConfig {
            id: "test-relayer-1".to_string(),
            network_type: ConfigFileNetworkType::Evm,
            signer_id: "test-signer-1".to_string(),
            name: "test-relayer-1".to_string(),
            network: "test-network".to_string(),
            paused: false,
            policies: None,
            notification_id: None,
            custom_rpc_urls: None,
        }];

        // Create config
        let config = Config {
            signers: signers.clone(),
            relayers,
            notifications: vec![],
            networks: NetworksFileConfig::new(vec![]).unwrap(),
            plugins: Some(vec![]),
        };

        // Create app state
        let app_state = ThinData(create_test_app_state());

        // First process signers (required for relayers)
        process_signers(&config, &app_state).await?;

        // Process relayers
        process_relayers(&config, &app_state).await?;

        // Verify relayers were created
        let stored_relayers = app_state.relayer_repository.list_all().await?;
        assert_eq!(stored_relayers.len(), 1);
        assert_eq!(stored_relayers[0].id, "test-relayer-1");
        assert_eq!(stored_relayers[0].signer_id, "test-signer-1");
        assert!(!stored_relayers[0].address.is_empty()); // Address should be populated

        Ok(())
    }

    #[tokio::test]
    async fn test_process_plugins() -> Result<()> {
        // Create test plugins
        let plugins = vec![
            PluginFileConfig {
                id: "test-plugin-1".to_string(),
                path: "/app/plugins/test.ts".to_string(),
                timeout: None,
                emit_logs: false,
                emit_traces: false,
                config: None,
                raw_response: false,
                allow_get_invocation: false,
                forward_logs: false,
            },
            PluginFileConfig {
                id: "test-plugin-2".to_string(),
                path: "/app/plugins/test2.ts".to_string(),
                timeout: Some(12),
                emit_logs: false,
                emit_traces: false,
                config: None,
                raw_response: false,
                allow_get_invocation: false,
                forward_logs: false,
            },
        ];

        // Create config
        let config = Config {
            signers: vec![],
            relayers: vec![],
            notifications: vec![],
            networks: NetworksFileConfig::new(vec![]).unwrap(),
            plugins: Some(plugins),
        };

        // Create app state
        let app_state = ThinData(create_test_app_state());

        // Process plugins
        process_plugins(&config, &app_state).await?;

        // Verify plugins were created
        let plugin_1 = app_state
            .plugin_repository
            .get_by_id("test-plugin-1")
            .await?;
        let plugin_2 = app_state
            .plugin_repository
            .get_by_id("test-plugin-2")
            .await?;

        assert!(plugin_1.is_some());
        assert!(plugin_2.is_some());

        let plugin_1 = plugin_1.unwrap();
        let plugin_2 = plugin_2.unwrap();

        assert_eq!(plugin_1.path, "/app/plugins/test.ts");
        assert_eq!(plugin_2.path, "/app/plugins/test2.ts");

        // check that the timeout is set to the default value when not provided.
        assert_eq!(
            plugin_1.timeout.as_secs(),
            Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS).as_secs()
        );
        assert_eq!(
            plugin_2.timeout.as_secs(),
            Duration::from_secs(12).as_secs()
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_process_api_key() -> Result<()> {
        let server_config = Arc::new(crate::utils::mocks::mockutils::create_test_server_config(
            RepositoryStorageType::InMemory,
        ));
        let app_state = ThinData(create_test_app_state());

        process_api_key(&server_config, &app_state).await?;

        let pagination_query = PaginationQuery {
            page: 1,
            per_page: 10,
        };

        let stored_api_keys = app_state
            .api_key_repository
            .list_paginated(pagination_query)
            .await?;
        assert_eq!(stored_api_keys.items.len(), 1);
        assert_eq!(stored_api_keys.items[0].name, "default");

        Ok(())
    }

    #[tokio::test]
    async fn test_process_config_file() -> Result<()> {
        // Create test signers, relayers, and notifications
        let signers = vec![SignerFileConfig {
            id: "test-signer-1".to_string(),
            config: SignerFileConfigEnum::Local(LocalSignerFileConfig {
                path: "tests/utils/test_keys/unit-test-local-signer.json".to_string(),
                passphrase: PlainOrEnvValue::Plain {
                    value: SecretString::new("test"),
                },
            }),
        }];

        let relayers = vec![RelayerFileConfig {
            id: "test-relayer-1".to_string(),
            network_type: ConfigFileNetworkType::Evm,
            signer_id: "test-signer-1".to_string(),
            name: "test-relayer-1".to_string(),
            network: "test-network".to_string(),
            paused: false,
            policies: None,
            notification_id: None,
            custom_rpc_urls: None,
        }];

        let notifications = vec![NotificationConfig {
            id: "test-notification-1".to_string(),
            r#type: NotificationType::Webhook,
            url: "https://hooks.slack.com/test1".to_string(),
            signing_key: None,
        }];

        let plugins = vec![PluginFileConfig {
            id: "test-plugin-1".to_string(),
            path: "/app/plugins/test.ts".to_string(),
            timeout: None,
            emit_logs: false,
            emit_traces: false,
            allow_get_invocation: false,
            config: None,
            raw_response: false,
            forward_logs: false,
        }];

        // Create config
        let config = Config {
            signers,
            relayers,
            notifications,
            networks: NetworksFileConfig::new(vec![]).unwrap(),
            plugins: Some(plugins),
        };

        // Create shared repositories
        let signer_repo = Arc::new(InMemorySignerRepository::default());
        let relayer_repo = Arc::new(RelayerRepositoryStorage::new_in_memory());
        let notification_repo = Arc::new(InMemoryNotificationRepository::default());
        let network_repo = Arc::new(InMemoryNetworkRepository::default());
        let transaction_repo = Arc::new(TransactionRepositoryStorage::InMemory(
            InMemoryTransactionRepository::new(),
        ));
        let transaction_counter = Arc::new(InMemoryTransactionCounter::default());
        let plugin_repo = Arc::new(InMemoryPluginRepository::default());
        let api_key_repo = Arc::new(InMemoryApiKeyRepository::default());

        // Create a mock job producer
        let mut mock_job_producer = MockJobProducerTrait::new();
        mock_job_producer
            .expect_produce_transaction_request_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));
        mock_job_producer
            .expect_produce_submit_transaction_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));
        mock_job_producer
            .expect_produce_check_transaction_status_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));
        mock_job_producer
            .expect_produce_send_notification_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));
        let job_producer = Arc::new(mock_job_producer);

        // Create app state
        let app_state = ThinData(AppState {
            signer_repository: signer_repo.clone(),
            relayer_repository: relayer_repo.clone(),
            notification_repository: notification_repo.clone(),
            network_repository: network_repo.clone(),
            transaction_repository: transaction_repo.clone(),
            transaction_counter_store: transaction_counter.clone(),
            job_producer: job_producer.clone(),
            plugin_repository: plugin_repo.clone(),
            api_key_repository: api_key_repo.clone(),
        });

        // Process the entire config file
        let server_config = Arc::new(crate::utils::mocks::mockutils::create_test_server_config(
            RepositoryStorageType::InMemory,
        ));
        process_config_file(config, server_config, &app_state).await?;

        // Verify all repositories were populated
        let stored_signers = signer_repo.list_all().await?;
        assert_eq!(stored_signers.len(), 1);
        assert_eq!(stored_signers[0].id, "test-signer-1");

        let stored_relayers = relayer_repo.list_all().await?;
        assert_eq!(stored_relayers.len(), 1);
        assert_eq!(stored_relayers[0].id, "test-relayer-1");
        assert_eq!(stored_relayers[0].signer_id, "test-signer-1");

        let stored_notifications = notification_repo.list_all().await?;
        assert_eq!(stored_notifications.len(), 1);
        assert_eq!(stored_notifications[0].id, "test-notification-1");

        let stored_plugin = plugin_repo.get_by_id("test-plugin-1").await?;
        assert_eq!(stored_plugin.unwrap().path, "/app/plugins/test.ts");

        Ok(())
    }

    #[tokio::test]
    async fn test_process_signer_google_cloud_kms() {
        use crate::models::SecretString;

        let signer = SignerFileConfig {
            id: "gcp-kms-signer".to_string(),
            config: SignerFileConfigEnum::GoogleCloudKms(GoogleCloudKmsSignerFileConfig {
                service_account: GoogleCloudKmsServiceAccountFileConfig {
                    private_key: PlainOrEnvValue::Plain {
                        value: SecretString::new("-----BEGIN EXAMPLE PRIVATE KEY-----\nFAKEKEYDATA\n-----END EXAMPLE PRIVATE KEY-----\n"),
                    },
                    client_email: PlainOrEnvValue::Plain {
                        value: SecretString::new("test-service-account@example.com"),
                    },
                    private_key_id: PlainOrEnvValue::Plain {
                        value: SecretString::new("fake-private-key-id"),
                    },
                    client_id: "fake-client-id".to_string(),
                    project_id: "fake-project-id".to_string(),
                    auth_uri: "https://accounts.google.com/o/oauth2/auth".to_string(),
                    token_uri: "https://oauth2.googleapis.com/token".to_string(),
                    client_x509_cert_url: "https://www.googleapis.com/robot/v1/metadata/x509/test-service-account%40example.com".to_string(),
                    auth_provider_x509_cert_url: "https://www.googleapis.com/oauth2/v1/certs".to_string(),
                    universe_domain: "googleapis.com".to_string(),
                },
                key: GoogleCloudKmsKeyFileConfig {
                    location: "global".to_string(),
                    key_id: "fake-key-id".to_string(),
                    key_ring_id: "fake-key-ring-id".to_string(),
                    key_version: 1,
                },
            }),
        };

        let result = process_signer(&signer).await;

        assert!(
            result.is_ok(),
            "Failed to process Google Cloud KMS signer: {:?}",
            result.err()
        );
        let model = result.unwrap();

        assert_eq!(model.id, "gcp-kms-signer");
    }

    // Helper function to create test server config with specific settings
    fn create_test_server_config_with_settings(
        storage_type: RepositoryStorageType,
        reset_storage_on_start: bool,
    ) -> ServerConfig {
        ServerConfig {
            repository_storage_type: storage_type.clone(),
            reset_storage_on_start,
            ..create_test_server_config(storage_type)
        }
    }

    async fn create_test_redis_pool() -> Option<Arc<Pool>> {
        let cfg = deadpool_redis::Config::from_url("redis://127.0.0.1:6379");
        let pool = cfg
            .builder()
            .ok()?
            .max_size(16)
            .runtime(deadpool_redis::Runtime::Tokio1)
            .build()
            .ok()?;
        Some(Arc::new(pool))
    }

    // Helper function to create minimal test config
    fn create_minimal_test_config() -> Config {
        Config {
            signers: vec![SignerFileConfig {
                id: "test-signer-1".to_string(),
                config: SignerFileConfigEnum::Local(LocalSignerFileConfig {
                    path: "tests/utils/test_keys/unit-test-local-signer.json".to_string(),
                    passphrase: PlainOrEnvValue::Plain {
                        value: SecretString::new("test"),
                    },
                }),
            }],
            relayers: vec![RelayerFileConfig {
                id: "test-relayer-1".to_string(),
                network_type: ConfigFileNetworkType::Evm,
                signer_id: "test-signer-1".to_string(),
                name: "test-relayer-1".to_string(),
                network: "test-network".to_string(),
                paused: false,
                policies: None,
                notification_id: None,
                custom_rpc_urls: None,
            }],
            notifications: vec![NotificationConfig {
                id: "test-notification-1".to_string(),
                r#type: NotificationType::Webhook,
                url: "https://hooks.slack.com/test1".to_string(),
                signing_key: None,
            }],
            networks: NetworksFileConfig::new(vec![]).unwrap(),
            plugins: None,
        }
    }

    #[tokio::test]
    async fn test_should_process_config_file_inmemory_storage() -> Result<()> {
        let config = create_minimal_test_config();

        // Test 1: InMemory storage with reset_storage_on_start = false
        let server_config = Arc::new(create_test_server_config_with_settings(
            RepositoryStorageType::InMemory,
            false,
        ));

        let app_state = ThinData(create_test_app_state());
        process_config_file(config.clone(), server_config.clone(), &app_state).await?;

        let stored_relayers = app_state.relayer_repository.list_all().await?;
        assert_eq!(stored_relayers.len(), 1);
        assert_eq!(stored_relayers[0].id, "test-relayer-1");

        // Test 2: InMemory storage with reset_storage_on_start = true
        let server_config2 = Arc::new(create_test_server_config_with_settings(
            RepositoryStorageType::InMemory,
            true,
        ));

        let app_state2 = ThinData(create_test_app_state());
        process_config_file(config.clone(), server_config2, &app_state2).await?;

        let stored_relayers = app_state2.relayer_repository.list_all().await?;
        assert_eq!(stored_relayers.len(), 1);
        assert_eq!(stored_relayers[0].id, "test-relayer-1");

        Ok(())
    }

    #[tokio::test]
    async fn test_should_process_config_file_redis_storage_empty_repositories() -> Result<()> {
        let config = create_minimal_test_config();
        let server_config = Arc::new(create_test_server_config_with_settings(
            RepositoryStorageType::Redis,
            false,
        ));

        let app_state = ThinData(create_test_app_state());
        process_config_file(config, server_config, &app_state).await?;

        let stored_relayers = app_state.relayer_repository.list_all().await?;
        assert_eq!(stored_relayers.len(), 1);
        assert_eq!(stored_relayers[0].id, "test-relayer-1");

        Ok(())
    }

    #[tokio::test]
    async fn test_validate_config_bootstrap_state_empty() -> Result<()> {
        let config = create_minimal_test_config();
        let app_state = ThinData(create_test_app_state());

        let state = validate_config_bootstrap_state(&config, &app_state).await?;

        assert_eq!(state, ConfigBootstrapState::Empty);
        Ok(())
    }

    #[tokio::test]
    async fn test_validate_config_bootstrap_state_complete_without_marker() -> Result<()> {
        let config = create_minimal_test_config();
        let app_state = ThinData(create_test_app_state());
        let server_config =
            create_test_server_config_with_settings(RepositoryStorageType::Redis, false);

        process_plugins(&config, &app_state).await?;
        process_signers(&config, &app_state).await?;
        process_notifications(&config, &app_state).await?;
        process_networks(&config, &app_state).await?;
        process_relayers(&config, &app_state).await?;
        process_api_key(&server_config, &app_state).await?;

        let state = validate_config_bootstrap_state(&config, &app_state).await?;

        assert_eq!(state, ConfigBootstrapState::Complete);
        Ok(())
    }

    #[tokio::test]
    async fn test_validate_config_bootstrap_state_incomplete_without_marker() -> Result<()> {
        let config = create_minimal_test_config();
        let app_state = ThinData(create_test_app_state());

        process_plugins(&config, &app_state).await?;
        process_signers(&config, &app_state).await?;
        process_notifications(&config, &app_state).await?;
        process_networks(&config, &app_state).await?;

        let state = validate_config_bootstrap_state(&config, &app_state).await?;

        match state {
            ConfigBootstrapState::Incomplete { missing } => {
                assert!(missing.iter().any(|entry| entry == "relayer repository"));
                assert!(missing.iter().any(|entry| entry == "api key repository"));
            }
            other => panic!("Expected incomplete bootstrap state, got {other:?}"),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_process_config_file_errors_when_redis_bootstrap_state_is_incomplete() -> Result<()>
    {
        let config = create_minimal_test_config();
        let server_config = Arc::new(create_test_server_config_with_settings(
            RepositoryStorageType::Redis,
            false,
        ));

        let app_state = ThinData(create_test_app_state());

        app_state
            .relayer_repository
            .create(create_mock_relayer("existing-relayer".to_string(), false))
            .await?;

        let err = process_config_file(config, server_config, &app_state)
            .await
            .expect_err("partial bootstrap state should fail closed");
        assert!(err
            .to_string()
            .contains("incomplete bootstrap-managed config state"));

        Ok(())
    }

    #[tokio::test]
    async fn test_should_error_for_partial_config_file_redis_storage_populated_repositories(
    ) -> Result<()> {
        let config = create_minimal_test_config();
        let server_config = Arc::new(create_test_server_config_with_settings(
            RepositoryStorageType::Redis,
            false,
        ));

        // Create two identical app states to test the decision logic
        let app_state1 = ThinData(create_test_app_state());
        let app_state2 = ThinData(create_test_app_state());

        // Pre-populate repositories to simulate Redis already having data
        let existing_relayer1 = create_mock_relayer("existing-relayer".to_string(), false);
        let existing_relayer2 = create_mock_relayer("existing-relayer".to_string(), false);
        app_state1
            .relayer_repository
            .create(existing_relayer1)
            .await?;
        app_state2
            .relayer_repository
            .create(existing_relayer2)
            .await?;

        // Check initial state
        assert!(app_state1.relayer_repository.has_entries().await?);
        assert!(!app_state1.signer_repository.has_entries().await?);

        let err = process_config_file(config, server_config, &app_state2)
            .await
            .expect_err("partial bootstrap state should fail closed");
        assert!(err
            .to_string()
            .contains("incomplete bootstrap-managed config state"));

        Ok(())
    }

    #[tokio::test]
    async fn test_should_process_config_file_redis_storage_with_reset_flag() -> Result<()> {
        let config = create_minimal_test_config();
        let server_config = Arc::new(create_test_server_config_with_settings(
            RepositoryStorageType::Redis,
            true, // reset_storage_on_start = true
        ));

        let app_state = ThinData(create_test_app_state());

        // Pre-populate repositories to simulate Redis already having data
        let existing_relayer = create_mock_relayer("existing-relayer".to_string(), false);
        let existing_signer = create_mock_signer();
        app_state
            .relayer_repository
            .create(existing_relayer)
            .await?;
        app_state.signer_repository.create(existing_signer).await?;

        // Should process config file because reset_storage_on_start = true
        process_config_file(config, server_config, &app_state).await?;

        let stored_relayer = app_state
            .relayer_repository
            .get_by_id("existing-relayer".to_string())
            .await;
        assert!(
            stored_relayer.is_err(),
            "Existing relayer should not be found"
        );

        let stored_signer = app_state
            .signer_repository
            .get_by_id("existing-signer".to_string())
            .await;
        assert!(
            stored_signer.is_err(),
            "Existing signer should not be found"
        );

        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn test_process_if_needed_after_lock_skips_populated_redis_without_marker() -> Result<()>
    {
        let conn = create_test_redis_pool()
            .await
            .expect("Redis connection required");
        let prefix = "test_config_processing_skip_populated_without_marker";
        let bootstrap_meta_key = format!("{prefix}:bootstrap_meta");

        {
            let mut conn_clone = conn.get().await.expect("Failed to get connection");
            let _: Result<(), _> =
                redis::AsyncCommands::del(&mut conn_clone, &bootstrap_meta_key).await;
        }

        let config = create_minimal_test_config();
        let server_config =
            create_test_server_config_with_settings(RepositoryStorageType::Redis, false);
        let app_state = ThinData(create_test_app_state());

        process_signers(&config, &app_state).await?;
        process_notifications(&config, &app_state).await?;
        process_networks(&config, &app_state).await?;
        process_relayers(&config, &app_state).await?;
        process_api_key(&server_config, &app_state).await?;

        process_if_needed_after_lock(&config, &server_config, &app_state, &conn, prefix).await?;

        let stored_relayers = app_state.relayer_repository.list_all().await?;
        assert_eq!(stored_relayers.len(), 1);
        assert_eq!(stored_relayers[0].id, "test-relayer-1");
        assert!(is_config_processing_completed(&conn, prefix).await?);

        {
            let mut conn_clone = conn.get().await.expect("Failed to get connection");
            let _: Result<(), _> =
                redis::AsyncCommands::del(&mut conn_clone, &bootstrap_meta_key).await;
        }

        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn test_process_if_needed_after_lock_recovers_when_in_progress_marker_exists(
    ) -> Result<()> {
        let conn = create_test_redis_pool()
            .await
            .expect("Redis connection required");
        let prefix = "test_config_processing_recover_in_progress";
        let bootstrap_meta_key = format!("{prefix}:bootstrap_meta");

        {
            let mut conn_clone = conn.get().await.expect("Failed to get connection");
            let _: Result<(), _> =
                redis::AsyncCommands::del(&mut conn_clone, &bootstrap_meta_key).await;
        }

        let config = create_minimal_test_config();
        let server_config =
            create_test_server_config_with_settings(RepositoryStorageType::Redis, false);
        let app_state = ThinData(create_test_app_state());

        process_signers(&config, &app_state).await?;
        process_notifications(&config, &app_state).await?;
        process_networks(&config, &app_state).await?;
        process_relayers(&config, &app_state).await?;
        process_api_key(&server_config, &app_state).await?;
        set_config_processing_in_progress(&conn, prefix).await?;

        process_if_needed_after_lock(&config, &server_config, &app_state, &conn, prefix).await?;

        let stored_relayers = app_state.relayer_repository.list_all().await?;
        assert_eq!(stored_relayers.len(), 1);
        assert_eq!(stored_relayers[0].id, "test-relayer-1");
        assert!(is_config_processing_completed(&conn, prefix).await?);

        {
            let mut conn_clone = conn.get().await.expect("Failed to get connection");
            let _: Result<(), _> =
                redis::AsyncCommands::del(&mut conn_clone, &bootstrap_meta_key).await;
        }

        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn test_recover_config_processing_after_timeout_waits_for_extended_completion() {
        let conn = create_test_redis_pool()
            .await
            .expect("Redis connection required");
        let prefix = "test_config_recovery_extended_wait";
        let lock_key = format!("{prefix}:lock:{CONFIG_PROCESSING_LOCK_NAME}");
        let bootstrap_meta_key = format!("{prefix}:bootstrap_meta");

        {
            let mut conn_clone = conn.get().await.expect("Failed to get connection");
            let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &lock_key).await;
            let _: Result<(), _> =
                redis::AsyncCommands::del(&mut conn_clone, &bootstrap_meta_key).await;
        }

        let config = create_minimal_test_config();
        let server_config =
            create_test_server_config_with_settings(RepositoryStorageType::Redis, false);
        let app_state = ThinData(create_test_app_state());

        let lock = DistributedLock::new(
            conn.clone(),
            &lock_key,
            Duration::from_secs(BOOTSTRAP_LOCK_TTL_SECS),
        );
        let guard = lock
            .try_acquire()
            .await
            .expect("Should acquire lock")
            .expect("Lock should be available");

        let conn_for_task = conn.clone();
        let prefix_for_task = prefix.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            set_config_processing_completed(&conn_for_task, &prefix_for_task)
                .await
                .expect("Should set config processing completed");
            guard.release().await.expect("Should release lock");
        });

        let result = recover_config_processing_after_timeout(
            &config,
            &server_config,
            &app_state,
            &conn,
            prefix,
        )
        .await;

        assert!(
            result.is_ok(),
            "Should succeed when completion is observed during extended wait: {:?}",
            result
        );

        {
            let mut conn_clone = conn.get().await.expect("Failed to get connection");
            let _: Result<(), _> = redis::AsyncCommands::del(&mut conn_clone, &lock_key).await;
            let _: Result<(), _> =
                redis::AsyncCommands::del(&mut conn_clone, &bootstrap_meta_key).await;
        }
    }
}
