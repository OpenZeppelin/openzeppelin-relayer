//! Process-boot wiring for the Midnight shared DUST sync task.
//!
//! Runs after repos are populated and before per-relayer `initialize_relayer`.
//! Enumerates configured midnight relayers, groups them by `network_id`, and
//! calls `init_network_sync` once per network with the full set of wallet
//! seeds on that network. The per-network task starts its WS subscription
//! immediately and begins catching up; relayers that later subscribe via
//! `task.subscribe_wallet(seed)` observe the Ready transition through a
//! `WalletHandle`.
//!
//! This function is a no-op if the process has no midnight relayers.

#![cfg(feature = "midnight")]

use std::collections::HashMap;

use color_eyre::{eyre::WrapErr, Result};
use midnight_node_ledger_helpers::WalletSeed;
use tracing::{info, warn};

use crate::jobs::JobProducerTrait;
use crate::models::{
    NetworkConfigData, NetworkRepoModel, NetworkType, NotificationRepoModel, RelayerRepoModel,
    SignerConfigStorage, SignerRepoModel, ThinDataAppState, TransactionRepoModel,
};
use crate::repositories::{
    ApiKeyRepositoryTrait, NetworkRepository, PluginRepositoryTrait, RelayerRepository, Repository,
    TransactionCounterTrait, TransactionRepository,
};
use crate::services::sync::midnight::{init_network_sync, MidnightIndexerClient};

/// Enumerate midnight relayers, group by network, start one shared DUST sync
/// task per network. Must be called after `process_config_file` (so repos
/// are populated) and before `initialize_relayers` (so per-relayer init can
/// subscribe to the shared task rather than running its own one-shot sync).
pub async fn initialize_midnight_shared_sync<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
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
    let relayers = app_state
        .relayer_repository
        .list_all()
        .await
        .wrap_err("failed to list relayers for midnight shared sync init")?;

    // Collect (network_id, seed) pairs for every active midnight relayer.
    // Paused relayers are still included so unpausing is instant — the sync
    // task runs while they're paused at effectively zero cost.
    let mut by_network: HashMap<String, Vec<WalletSeed>> = HashMap::new();
    for relayer in &relayers {
        if relayer.network_type != NetworkType::Midnight {
            continue;
        }
        let signer = match app_state
            .signer_repository
            .get_by_id(relayer.signer_id.clone())
            .await
        {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    relayer_id = %relayer.id,
                    signer_id = %relayer.signer_id,
                    error = %e,
                    "skipping midnight relayer: signer lookup failed"
                );
                continue;
            }
        };
        let seed_bytes = match seed_bytes_from_signer(&signer) {
            Some(b) => b,
            None => {
                warn!(
                    relayer_id = %relayer.id,
                    signer_id = %relayer.signer_id,
                    "skipping midnight relayer: signer is not a 32-byte local key"
                );
                continue;
            }
        };
        by_network
            .entry(relayer.network.clone())
            .or_default()
            .push(WalletSeed::Medium(seed_bytes));
    }

    if by_network.is_empty() {
        info!("No midnight relayers configured; shared DUST sync task not started");
        return Ok(());
    }

    // For each network, look up its config so we can get the indexer URLs.
    for (network_id, seeds) in by_network {
        let network_model = match app_state
            .network_repository
            .get_by_name(NetworkType::Midnight, &network_id)
            .await
        {
            Ok(Some(m)) => m,
            Ok(None) => {
                warn!(
                    network_id,
                    "no midnight network config found for relayers on this network; skipping"
                );
                continue;
            }
            Err(e) => {
                warn!(
                    network_id,
                    error = %e,
                    "failed to load midnight network config; skipping"
                );
                continue;
            }
        };
        let midnight_cfg = match &network_model.config {
            NetworkConfigData::Midnight(cfg) => cfg.clone(),
            _ => {
                warn!(
                    network_id,
                    "network config is not a Midnight variant; skipping"
                );
                continue;
            }
        };
        let indexer = MidnightIndexerClient::new(midnight_cfg.indexer_urls.clone());
        let slot = init_network_sync(&network_id, seeds, indexer);
        info!(
            network_id,
            seed_count = slot.seed_count(),
            "Midnight shared DUST sync task started"
        );
    }

    Ok(())
}

/// Extract a 32-byte seed from a signer's raw-key storage. Returns None for
/// non-local signers (the midnight relayer currently requires a local signer
/// whose 32 raw bytes ARE the wallet seed).
fn seed_bytes_from_signer(signer: &SignerRepoModel) -> Option<[u8; 32]> {
    match &signer.config {
        SignerConfigStorage::Local(local) => {
            let bytes = local.raw_key.borrow();
            if bytes.len() == 32 {
                let mut out = [0u8; 32];
                out.copy_from_slice(&bytes);
                Some(out)
            } else {
                None
            }
        }
        _ => None,
    }
}
