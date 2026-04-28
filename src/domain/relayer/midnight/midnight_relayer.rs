use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, info, instrument, warn};

use crate::{
    domain::{
        BalanceResponse, Relayer, SignDataRequest, SignDataResponse,
        SignTransactionExternalResponse, SignTransactionRequest, SignTypedDataRequest,
    },
    jobs::{JobProducerTrait, TransactionRequest},
    models::{
        DeletePendingTransactionsResponse, HealthCheckFailure, JsonRpcRequest, JsonRpcResponse,
        MidnightNetwork, NetworkRepoModel, NetworkRpcRequest, NetworkRpcResult,
        NetworkTransactionRequest, NetworkType, RelayerError, RelayerRepoModel, RelayerStatus,
        TransactionRepoModel, TransactionStatus,
    },
    repositories::{
        NetworkRepository, RelayerRepository, RelayerStateRepositoryStorage, Repository,
        SyncStateTrait, TransactionRepository,
    },
    services::{
        provider::MidnightProviderTrait,
        signer::{MidnightSigner, Signer},
        sync::midnight::{LedgerContextManager, SyncManager},
    },
};

/// Full Midnight relayer implementation.
///
/// Orchestrates transaction submission, status, health checks, and balance
/// queries for the Midnight network. The key Midnight-specific concepts:
///
/// - **Nonce = ledger index** — unlike EVM account nonces, Midnight tracks
///   the blockchain merkle tree height as its sequence number.
/// - **No cancellation** — Midnight transactions cannot be replaced/cancelled.
/// - **Sync manager** — maintains ledger context for wallet state tracking.
pub struct MidnightRelayer<P, RR, NR, TR, J, SS = RelayerStateRepositoryStorage>
where
    P: MidnightProviderTrait + Send + Sync,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    SS: SyncStateTrait + Send + Sync,
{
    pub relayer: RelayerRepoModel,
    pub network: MidnightNetwork,
    pub provider: Arc<P>,
    pub signer: Arc<MidnightSigner>,
    pub sync_manager: SyncManager<SS>,
    pub ledger_ctx: Arc<LedgerContextManager>,
    pub relayer_repository: Arc<RR>,
    pub network_repository: Arc<NR>,
    pub transaction_repository: Arc<TR>,
    pub job_producer: Arc<J>,
}

/// Type alias for the default concrete MidnightRelayer used in production.
pub type DefaultMidnightRelayer<J, TR, RR, NR> = MidnightRelayer<
    crate::services::provider::MidnightProvider,
    RR,
    NR,
    TR,
    J,
    RelayerStateRepositoryStorage,
>;

impl<P, RR, NR, TR, J, SS> MidnightRelayer<P, RR, NR, TR, J, SS>
where
    P: MidnightProviderTrait + Send + Sync,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    SS: SyncStateTrait + Send + Sync,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        relayer: RelayerRepoModel,
        network: MidnightNetwork,
        provider: Arc<P>,
        signer: Arc<MidnightSigner>,
        sync_manager: SyncManager<SS>,
        ledger_ctx: Arc<LedgerContextManager>,
        relayer_repository: Arc<RR>,
        network_repository: Arc<NR>,
        transaction_repository: Arc<TR>,
        job_producer: Arc<J>,
    ) -> Result<Self, RelayerError> {
        Ok(Self {
            relayer,
            network,
            provider,
            signer,
            sync_manager,
            ledger_ctx,
            relayer_repository,
            network_repository,
            transaction_repository,
            job_producer,
        })
    }

    /// Sync the current ledger index from the provider (Midnight's equivalent of nonce).
    async fn sync_nonce(&self) -> Result<u64, RelayerError> {
        let block_number = self
            .provider
            .get_block_number()
            .await
            .map_err(|e| RelayerError::ProviderError(e.to_string()))?;

        debug!(
            relayer_id = %self.relayer.id,
            block_number,
            "Synced Midnight ledger index"
        );

        Ok(block_number)
    }
}

#[async_trait]
impl<P, RR, NR, TR, J, SS> Relayer for MidnightRelayer<P, RR, NR, TR, J, SS>
where
    P: MidnightProviderTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    SS: SyncStateTrait + Send + Sync + 'static,
{
    #[instrument(
        level = "debug",
        skip(self, tx_request),
        fields(relayer_id = %self.relayer.id)
    )]
    async fn process_transaction_request(
        &self,
        tx_request: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError> {
        let network_model = self
            .network_repository
            .get_by_name(NetworkType::Midnight, &self.relayer.network)
            .await?
            .ok_or_else(|| {
                RelayerError::NetworkConfiguration(format!(
                    "Midnight network '{}' not found",
                    self.relayer.network
                ))
            })?;

        let tx_model =
            TransactionRepoModel::try_from((&tx_request, &self.relayer, &network_model))?;

        self.transaction_repository
            .create(tx_model.clone())
            .await
            .map_err(|e| RelayerError::QueueError(e.to_string()))?;

        info!(
            relayer_id = %self.relayer.id,
            tx_id = %tx_model.id,
            "Created Midnight transaction, enqueuing for processing"
        );

        self.job_producer
            .produce_transaction_request_job(
                TransactionRequest::new(tx_model.id.clone(), self.relayer.id.clone())
                    .with_network_type(NetworkType::Midnight),
                None,
            )
            .await
            .map_err(|e| RelayerError::QueueError(e.to_string()))?;

        Ok(tx_model)
    }

    async fn get_balance(&self) -> Result<BalanceResponse, RelayerError> {
        use crate::domain::relayer::{TokenBalance, TokenPrivacy};

        let balance = self
            .sync_manager
            .get_unshielded_balance()
            .await
            .unwrap_or(0);
        let dust = self.ledger_ctx.dust_balance();

        // Multi-token breakdown — surface tNIGHT and DUST under the same
        // shape so callers don't need a second round-trip to /status to
        // see whether the relayer has DUST. Shielded balances (any token
        // ever received via shielded transfer) are appended below with
        // `privacy: Shielded` and the raw 32-byte token-type hex as the
        // identifier — the chain treats unshielded NIGHT and shielded
        // NIGHT as separate token types (architecture doc §13.3), so we
        // can't collapse them into a single "NIGHT" entry.
        let mut balances = vec![
            TokenBalance {
                token: "tNIGHT".to_string(),
                balance: balance.to_string(),
                privacy: Some(TokenPrivacy::Unshielded),
            },
            TokenBalance {
                token: "DUST".to_string(),
                balance: dust.to_string(),
                privacy: Some(TokenPrivacy::Unshielded),
            },
        ];
        for (token_hex, value) in self.ledger_ctx.shielded_balances() {
            balances.push(TokenBalance {
                token: token_hex,
                balance: value.to_string(),
                privacy: Some(TokenPrivacy::Shielded),
            });
        }

        Ok(BalanceResponse {
            balance,
            unit: "tNIGHT".to_string(),
            balances: Some(balances),
        })
    }

    async fn delete_pending_transactions(
        &self,
    ) -> Result<DeletePendingTransactionsResponse, RelayerError> {
        Err(RelayerError::NotSupported(
            "Midnight does not support deleting pending transactions".into(),
        ))
    }

    async fn sign_data(&self, _request: SignDataRequest) -> Result<SignDataResponse, RelayerError> {
        Err(RelayerError::NotSupported(
            "Midnight does not support arbitrary data signing".into(),
        ))
    }

    async fn sign_typed_data(
        &self,
        _request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, RelayerError> {
        Err(RelayerError::NotSupported(
            "Midnight does not support typed data signing".into(),
        ))
    }

    async fn rpc(
        &self,
        _request: JsonRpcRequest<NetworkRpcRequest>,
    ) -> Result<JsonRpcResponse<NetworkRpcResult>, RelayerError> {
        Err(RelayerError::NotSupported(
            "Midnight RPC passthrough is not yet implemented".into(),
        ))
    }

    async fn get_status(&self) -> Result<RelayerStatus, RelayerError> {
        let nonce = self.sync_nonce().await.unwrap_or(0);

        let pending_count = self
            .transaction_repository
            .count_by_status(
                &self.relayer.id,
                &[TransactionStatus::Pending, TransactionStatus::Sent],
            )
            .await
            .unwrap_or(0);

        let balance = self
            .sync_manager
            .get_unshielded_balance()
            .await
            .unwrap_or(0);

        // Get addresses from the signer
        let unshielded_address = self
            .signer
            .address()
            .await
            .map(|a| a.to_string())
            .unwrap_or_default();
        let shielded_address = self.signer.shielded_address().to_string();
        let dust_address = self.signer.dust_address().to_string();

        // DUST balance: sum spendable DUST from the wallet's DustLocalState.
        // Returns "0" if DUST events haven't been synced yet (wallet state empty).
        let dust_balance = self.ledger_ctx.dust_balance().to_string();

        Ok(RelayerStatus::Midnight {
            balance: balance.to_string(),
            dust_balance,
            unshielded_address,
            shielded_address,
            dust_address,
            pending_transactions_count: pending_count,
            last_confirmed_transaction_timestamp: None,
            system_disabled: self.relayer.system_disabled,
            paused: self.relayer.paused,
            nonce: nonce.to_string(),
        })
    }

    #[instrument(level = "info", skip(self), fields(relayer_id = %self.relayer.id))]
    async fn initialize_relayer(&self) -> Result<(), RelayerError> {
        info!(relayer_id = %self.relayer.id, "Initializing Midnight relayer");

        // Health-check the provider and indexer
        self.provider
            .health_check()
            .await
            .map_err(|e| RelayerError::ProviderError(format!("RPC health check failed: {e}")))?;

        self.sync_manager.health_check().await.map_err(|e| {
            RelayerError::ProviderError(format!("Indexer health check failed: {e}"))
        })?;

        // Bootstrap LedgerContext from the node's runtime API.
        // This populates network_id and ledger parameters needed for tx building.
        // Subxt requires a WebSocket URL — convert https:// to wss://
        let rpc_url = self
            .network
            .rpc_urls
            .first()
            .map(|c| {
                c.url
                    .replace("https://", "wss://")
                    .replace("http://", "ws://")
            })
            .unwrap_or_default();

        match self.ledger_ctx.bootstrap_from_node(&rpc_url).await {
            Ok(()) => {
                info!(relayer_id = %self.relayer.id, "LedgerContext bootstrapped from node");
            }
            Err(e) => {
                warn!(
                    relayer_id = %self.relayer.id,
                    error = %e,
                    "LedgerContext bootstrap failed (non-fatal)"
                );
            }
        }

        // Sync initial ledger index
        let block = self.sync_nonce().await?;

        // Sync unshielded balance via WebSocket subscription
        let address = self
            .signer
            .address()
            .await
            .map_err(|e| RelayerError::ProviderError(e.to_string()))?
            .to_string();

        match self.sync_manager.sync_unshielded_balance(&address).await {
            Ok(result) => {
                info!(
                    relayer_id = %self.relayer.id,
                    block_number = block,
                    balance = result.balance,
                    utxos = result.created_utxos.len(),
                    "Midnight relayer initialized with balance sync"
                );

                // Inject the discovered UTXOs into the LedgerContext
                if !result.created_utxos.is_empty() {
                    if let Err(e) = self.ledger_ctx.inject_utxos(&result.created_utxos) {
                        warn!(
                            relayer_id = %self.relayer.id,
                            error = %e,
                            "Failed to inject UTXOs into LedgerContext"
                        );
                    }
                }
            }
            Err(e) => {
                warn!(
                    relayer_id = %self.relayer.id,
                    error = %e,
                    "Balance sync failed (non-fatal), relayer will retry"
                );
            }
        }

        // DUST sync via the process-wide sync task. Use the seed-aware
        // dispatch so runtime-added relayers (whose seed isn't in the shared
        // slot's list) resolve to their own isolated slot instead of falling
        // through. If neither shared nor isolated exists yet, the factory
        // will have registered an isolated slot during relayer construction;
        // we still guard for missing with a warn + skip.
        let wallet_seed_for_sync = self.ledger_ctx.wallet_seed().clone();
        if let Some(slot) = crate::services::sync::midnight::get_slot_for_seed(
            &self.network.network,
            &wallet_seed_for_sync,
        ) {
            let handle = slot
                .task
                .subscribe_wallet(self.ledger_ctx.wallet_seed().clone());

            // Mark the relayer as syncing up front; handlers already reject
            // tx requests for `system_disabled` relayers via
            // `validate_active_state`.
            if let Err(e) = self
                .relayer_repository
                .disable_relayer(
                    self.relayer.id.clone(),
                    crate::models::DisabledReason::Syncing("initial DUST catch-up".into()),
                )
                .await
            {
                warn!(
                    relayer_id = %self.relayer.id,
                    error = %e,
                    "failed to mark relayer as Syncing; wallet subscription still armed"
                );
            }

            // Background watcher: when the shared task signals Ready, clear
            // the disabled flag; on failure, surface it as SyncFailed so the
            // operator can see it in /status.
            let relayer_id = self.relayer.id.clone();
            let relayer_repo = self.relayer_repository.clone();
            tokio::spawn(async move {
                match handle.await_ready().await {
                    Ok(()) => {
                        if let Err(e) = relayer_repo.enable_relayer(relayer_id.clone()).await {
                            warn!(
                                relayer_id = %relayer_id,
                                error = %e,
                                "failed to mark relayer Ready after DUST sync"
                            );
                        } else {
                            info!(
                                relayer_id = %relayer_id,
                                "Midnight relayer wallet Ready"
                            );
                        }
                    }
                    Err(reason) => {
                        warn!(
                            relayer_id = %relayer_id,
                            reason = %reason,
                            "Midnight shared DUST sync failed"
                        );
                        let _ = relayer_repo
                            .disable_relayer(
                                relayer_id.clone(),
                                crate::models::DisabledReason::SyncFailed(reason),
                            )
                            .await;
                    }
                }
            });
        } else {
            warn!(
                relayer_id = %self.relayer.id,
                network_id = %self.network.network,
                "shared Midnight sync slot not found; DUST state will be stale"
            );
        }

        // Attempt shielded sync — populates the LedgerContext with wallet state.
        // This is the critical step that enables transaction building.
        let viewing_key = self.signer.viewing_key();
        let indexer = self.provider.get_indexer_client();
        let ledger_ctx = self.ledger_ctx.clone();

        // Restore persisted ledger state if available
        if let Ok(Some(state_bytes)) = self.sync_manager.load_context().await {
            if let Err(e) = ledger_ctx.restore_state(&state_bytes) {
                warn!(
                    relayer_id = %self.relayer.id,
                    error = %e,
                    "Failed to restore LedgerContext state, will sync from scratch"
                );
            }
        }

        match indexer.connect_wallet(&viewing_key).await {
            Ok(session_id) => {
                let start_idx = self.sync_manager.current_index().await.unwrap_or(0);

                match self
                    .sync_manager
                    .sync_shielded(&session_id, Some(start_idx), |event| {
                        use crate::services::sync::midnight::ShieldedEvent;
                        match &event {
                            ShieldedEvent::Transaction { raw_hex, .. } => {
                                // PR-3 v3: route observed shielded txs through the
                                // verification-skipping wallet apply path
                                // (`apply_observed_tx_offers`) instead of the strict
                                // `update_from_tx`. Strict re-verification rejects
                                // foreign txs with InvalidDustSpendProof because our
                                // local DUST/tblock state can't reproduce the
                                // sender's proof reference. Mirrors the TS reference
                                // `CoreWallet.replayEventsWithChanges` pattern.
                                match ledger_ctx.apply_observed_tx_offers(raw_hex) {
                                    Ok(n) if n > 0 => {
                                        debug!(offers = n, "Applied shielded offers to wallet");
                                    }
                                    Ok(_) => {}
                                    Err(e) => {
                                        warn!(error = %e, "Failed to apply observed tx offers");
                                    }
                                }
                            }
                            ShieldedEvent::MerkleUpdate {
                                update_hex,
                                start_index,
                                end_index,
                            } => {
                                let _ = ledger_ctx.apply_merkle_update(
                                    update_hex,
                                    *start_index,
                                    *end_index,
                                );
                            }
                            ShieldedEvent::Progress { .. } => {}
                        }
                    })
                    .await
                {
                    Ok(result) => {
                        info!(
                            relayer_id = %self.relayer.id,
                            transactions = result.transactions,
                            merkle_updates = result.merkle_updates,
                            highest_index = result.highest_index,
                            "Shielded sync completed, LedgerContext populated"
                        );

                        // Persist the updated ledger state
                        if result.transactions > 0 {
                            if let Ok(state_bytes) = ledger_ctx.serialize_state() {
                                let _ = self
                                    .sync_manager
                                    .persist_state(result.highest_index, Some(state_bytes))
                                    .await;
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            relayer_id = %self.relayer.id,
                            error = %e,
                            "Shielded sync failed (non-fatal)"
                        );
                    }
                }

                let _ = indexer.disconnect_wallet(&session_id).await;
            }
            Err(e) => {
                warn!(
                    relayer_id = %self.relayer.id,
                    error = %e,
                    "Shielded sync connect failed (non-fatal)"
                );
            }
        }

        Ok(())
    }

    async fn check_health(&self) -> Result<(), Vec<HealthCheckFailure>> {
        let mut failures = Vec::new();

        if let Err(e) = self.provider.health_check().await {
            failures.push(HealthCheckFailure::RpcValidationFailed(format!(
                "Midnight RPC health check failed: {e}"
            )));
        }

        if let Err(e) = self.sync_manager.health_check().await {
            failures.push(HealthCheckFailure::RpcValidationFailed(format!(
                "Midnight indexer health check failed: {e}"
            )));
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures)
        }
    }

    async fn validate_min_balance(&self) -> Result<(), RelayerError> {
        // Balance validation requires full wallet sync — skip for now
        Ok(())
    }

    async fn sign_transaction(
        &self,
        _request: &SignTransactionRequest,
    ) -> Result<SignTransactionExternalResponse, RelayerError> {
        Err(RelayerError::NotSupported(
            "Midnight does not support external transaction signing".into(),
        ))
    }
}
