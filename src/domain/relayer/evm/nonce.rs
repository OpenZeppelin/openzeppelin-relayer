//! Nonce management for EVM relayers.
//!
//! Handles nonce synchronization, gap detection, and gap resolution via NOOP transactions.
//! Also provides `handle_health_action` for targeted nonce health jobs dispatched
//! through the health check queue.

use std::collections::HashMap;
use std::time::Duration;

use tracing::{debug, error, info, instrument, warn};

use crate::{
    config::ServerConfig,
    constants::{
        EVM_STATUS_CHECK_INITIAL_DELAY_SECONDS, HEALTH_CHECK_ACTION_KEY,
        HEALTH_CHECK_ACTION_NONCE_HEALTH, MAX_GAP_SCAN_RANGE,
    },
    domain::relayer::RelayerError,
    jobs::{JobProducerTrait, TransactionRequest, TransactionStatusCheck},
    models::{
        EvmNetwork, EvmTransactionData, NetworkRepoModel, NetworkType, RelayerRepoModel,
        TransactionRepoModel, TransactionStatus,
    },
    repositories::{NetworkRepository, RelayerRepository, Repository, TransactionRepository},
    services::{
        provider::EvmProviderTrait, signer::DataSignerTrait, TransactionCounterServiceTrait,
    },
    utils::{calculate_scheduled_timestamp, DistributedLock},
};

use super::EvmRelayer;

/// Settling pause before gap scanning. Gives in-flight `prepare_transaction` calls
/// time to persist their reserved nonces to the repository. Best-effort mitigation
/// for the counter-increment-before-persist race. See `detect_nonce_gaps` docs.
const NONCE_GAP_SETTLE_DURATION: Duration = Duration::from_secs(2);

// ── Nonce synchronization & gap detection ─────────────────────────────────────

impl<P, RR, NR, TR, J, S, TCS> EvmRelayer<P, RR, NR, TR, J, S, TCS>
where
    P: EvmProviderTrait + Send + Sync,
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: DataSignerTrait + Send + Sync + 'static,
    TCS: TransactionCounterServiceTrait + Send + Sync + 'static,
{
    /// Synchronizes the nonce with the blockchain.
    #[instrument(
        level = "debug",
        skip(self),
        fields(
            request_id = ?crate::observability::request_id::get_request_id(),
            relayer_id = %self.relayer.id,
        )
    )]
    pub(crate) async fn sync_nonce(&self) -> Result<(), RelayerError> {
        let on_chain_nonce = self.get_on_chain_nonce().await?;

        let transaction_counter_nonce = self
            .transaction_counter_service
            .get()
            .await
            .ok()
            .flatten()
            .unwrap_or(0);

        let nonce = std::cmp::max(on_chain_nonce, transaction_counter_nonce);

        debug!(
            relayer_id = %self.relayer.id,
            on_chain_nonce = %on_chain_nonce,
            transaction_counter_nonce = %transaction_counter_nonce,
            "syncing nonce"
        );

        debug!(nonce = %nonce, "setting nonce for relayer");

        self.transaction_counter_service.set(nonce).await?;

        Ok(())
    }

    /// Fetches the on-chain nonce for this relayer's address.
    async fn get_on_chain_nonce(&self) -> Result<u64, RelayerError> {
        self.provider
            .get_transaction_count(&self.relayer.address)
            .await
            .map_err(|e| RelayerError::ProviderError(e.to_string()))
    }

    /// Checks if a transaction at the given nonce is still active (not a gap).
    ///
    /// Active statuses: Pending, Sent, Submitted, Mined — tx is still in-flight.
    /// Gap indicators: Failed, Canceled, Expired, Confirmed, or no tx at all.
    async fn find_active_tx_for_nonce(
        &self,
        nonce: u64,
    ) -> Result<Option<TransactionRepoModel>, RelayerError> {
        let tx = self
            .transaction_repository
            .find_by_nonce(&self.relayer.id, nonce)
            .await
            .map_err(|e| RelayerError::Internal(e.to_string()))?;

        match tx {
            Some(tx) => {
                let is_active = matches!(
                    tx.status,
                    TransactionStatus::Pending
                        | TransactionStatus::Sent
                        | TransactionStatus::Submitted
                        | TransactionStatus::Mined
                );
                if is_active {
                    Ok(Some(tx))
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    /// Detects nonce gaps between the on-chain nonce and a pre-captured counter snapshot.
    ///
    /// Accepts an optional pre-fetched on-chain nonce to avoid redundant RPC calls
    /// when the caller (e.g., `resolve_nonce_gaps`) already has it.
    /// Scans up to `MAX_GAP_SCAN_RANGE` nonces forward. Returns gap nonce list.
    ///
    /// # Known race condition (mitigated, not eliminated)
    ///
    /// The normal prepare path reserves a nonce via `get_and_increment()` before
    /// persisting it to the transaction repository. During that window, a nonce
    /// slot appears empty to this scanner — it could be misclassified as a gap.
    ///
    /// Mitigations in place:
    /// - **Settling pause**: `resolve_nonce_gaps` snapshots the counter, waits
    ///   `NONCE_GAP_SETTLE_DURATION`, then passes the snapshot here. Any nonce
    ///   reserved after the snapshot is excluded from the scan.
    /// - **Double-check**: `resolve_nonce_gaps` re-checks each gap candidate
    ///   via `find_active_tx_for_nonce` before creating a NOOP.
    /// - **Self-correcting**: if a false NOOP is created, it competes with the
    ///   real tx at the same nonce. The loser gets a nonce error which is handled
    ///   by the existing nonce recovery path — no funds lost, no stuck nonces.
    ///
    /// For a provably race-free solution, a `highest_persisted_nonce` watermark
    /// in the counter store would be needed (tracked as a follow-up).
    async fn detect_nonce_gaps(
        &self,
        on_chain_nonce: Option<u64>,
        counter_snapshot: Option<u64>,
    ) -> Result<Vec<u64>, RelayerError> {
        let on_chain_nonce = match on_chain_nonce {
            Some(n) => n,
            None => self.get_on_chain_nonce().await?,
        };

        let local_counter = match counter_snapshot {
            Some(n) => n,
            None => self
                .transaction_counter_service
                .get()
                .await
                .ok()
                .flatten()
                .unwrap_or(0),
        };

        if local_counter <= on_chain_nonce {
            return Ok(vec![]);
        }

        let scan_end = std::cmp::min(local_counter, on_chain_nonce + MAX_GAP_SCAN_RANGE);
        let mut gaps = Vec::new();

        for nonce in on_chain_nonce..scan_end {
            if self.find_active_tx_for_nonce(nonce).await?.is_none() {
                gaps.push(nonce);
            }
        }

        if local_counter > on_chain_nonce + MAX_GAP_SCAN_RANGE {
            warn!(
                relayer_id = %self.relayer.id,
                on_chain_nonce = on_chain_nonce,
                local_counter = local_counter,
                scan_range = MAX_GAP_SCAN_RANGE,
                "nonce gap exceeds scan range — operator investigation required"
            );
        }

        Ok(gaps)
    }

    /// Detects and resolves nonce gaps by creating gap-filling NOOP transactions.
    ///
    /// # Algorithm
    /// 1. Snapshot the counter + settle pause (allows in-flight `prepare_transaction` to persist)
    /// 2. Run `sync_nonce()` — raises counter to max(on_chain, local)
    /// 3. Run `detect_nonce_gaps()` with the snapshot — only scans nonces reserved before the pause
    /// 4. For each gap: double-check, create NOOP, push through prepare/submit pipeline
    ///
    /// # Returns
    /// Number of gaps filled, or error.
    #[instrument(
        level = "debug",
        skip(self),
        fields(
            request_id = ?crate::observability::request_id::get_request_id(),
            relayer_id = %self.relayer.id,
        )
    )]
    async fn resolve_nonce_gaps(&self) -> Result<usize, RelayerError> {
        // Snapshot the counter BEFORE the settling pause. Any nonce reserved via
        // get_and_increment() after this read is excluded from the scan range,
        // reducing the risk of misclassifying in-flight nonces as gaps.
        let counter_snapshot = self
            .transaction_counter_service
            .get()
            .await
            .ok()
            .flatten()
            .unwrap_or(0);

        // Settling pause: give in-flight prepare_transaction calls time to persist
        // their reserved nonces. This is a best-effort mitigation — not a guarantee.
        // See `detect_nonce_gaps` doc for the full race condition analysis.
        tokio::time::sleep(NONCE_GAP_SETTLE_DURATION).await;

        let on_chain_nonce = self.get_on_chain_nonce().await?;

        self.sync_nonce().await?;

        let gaps = self
            .detect_nonce_gaps(Some(on_chain_nonce), Some(counter_snapshot))
            .await?;
        if gaps.is_empty() {
            debug!("no nonce gaps detected");
            return Ok(0);
        }

        info!(
            relayer_id = %self.relayer.id,
            gap_count = gaps.len(),
            gaps = ?gaps,
            "filling nonce gaps with NOOP transactions"
        );

        let mut filled = 0;
        for nonce in &gaps {
            // Race guard — another instance may have filled this gap
            if self.find_active_tx_for_nonce(*nonce).await?.is_some() {
                debug!(
                    nonce = nonce,
                    "gap already filled by concurrent process, skipping"
                );
                continue;
            }

            match self.create_gap_filling_noop(*nonce).await {
                Ok(_) => {
                    filled += 1;
                    debug!(nonce = nonce, "created gap-filling NOOP transaction");
                }
                Err(e) => {
                    error!(
                        nonce = nonce,
                        error = %e,
                        "failed to create gap-filling NOOP, continuing with remaining gaps"
                    );
                }
            }
        }

        info!(
            relayer_id = %self.relayer.id,
            total_gaps = gaps.len(),
            filled = filled,
            "nonce gap resolution complete"
        );

        Ok(filled)
    }

    /// Creates a gap-filling NOOP transaction for a specific nonce.
    ///
    /// The transaction is created as `Pending` with a preset nonce and pushed through
    /// the normal prepare/submit pipeline.
    async fn create_gap_filling_noop(
        &self,
        nonce: u64,
    ) -> Result<TransactionRepoModel, RelayerError> {
        let network_model = self
            .network_repository
            .get_by_name(NetworkType::Evm, &self.relayer.network)
            .await?
            .ok_or_else(|| {
                RelayerError::NetworkConfiguration(format!(
                    "Network {} not found",
                    self.relayer.network
                ))
            })?;

        let evm_network = EvmNetwork::try_from(network_model.clone())?;

        let mut evm_data = EvmTransactionData {
            gas_price: None,
            gas_limit: None,
            nonce: None,
            value: crate::models::U256::from(0u64),
            data: None,
            from: self.relayer.address.clone(),
            to: None,
            chain_id: evm_network.id(),
            hash: None,
            signature: None,
            speed: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        crate::domain::evm::make_noop(&mut evm_data, &evm_network, Some(&self.provider))
            .await
            .map_err(|e| RelayerError::Internal(format!("Failed to create NOOP data: {e}")))?;

        evm_data.nonce = Some(nonce);

        let now = chrono::Utc::now().to_rfc3339();
        let tx = TransactionRepoModel {
            id: uuid::Uuid::new_v4().to_string(),
            relayer_id: self.relayer.id.clone(),
            status: TransactionStatus::Pending,
            status_reason: Some(format!("Gap-filling NOOP for nonce {nonce}")),
            created_at: now,
            sent_at: None,
            confirmed_at: None,
            valid_until: None,
            delete_at: None,
            network_type: NetworkType::Evm,
            network_data: crate::models::NetworkTransactionData::Evm(evm_data),
            priced_at: None,
            hashes: Vec::new(),
            noop_count: Some(1),
            is_canceled: Some(false),
            metadata: None,
        };

        self.transaction_repository
            .create(tx.clone())
            .await
            .map_err(|e| RelayerError::Internal(e.to_string()))?;

        // Push through prepare pipeline first, then schedule delayed status check as safety net
        self.job_producer
            .produce_transaction_request_job(
                TransactionRequest::new(tx.id.clone(), tx.relayer_id.clone()),
                None,
            )
            .await
            .map_err(RelayerError::from)?;

        self.job_producer
            .produce_check_transaction_status_job(
                TransactionStatusCheck::new(tx.id.clone(), tx.relayer_id.clone(), NetworkType::Evm),
                Some(calculate_scheduled_timestamp(
                    EVM_STATUS_CHECK_INITIAL_DELAY_SECONDS,
                )),
            )
            .await
            .map_err(RelayerError::from)?;

        info!(
            tx_id = %tx.id,
            nonce = nonce,
            "gap-filling NOOP transaction created and queued"
        );

        Ok(tx)
    }

    /// Handles a targeted health action dispatched via job metadata.
    ///
    /// Currently supported actions:
    /// - `nonce_health`: Detects and fills nonce gaps with NOOPs
    ///
    /// Returns `Ok(true)` if an action was handled, `Ok(false)` if no recognized action.
    pub(crate) async fn handle_health_action(
        &self,
        metadata: &HashMap<String, String>,
    ) -> Result<bool, RelayerError> {
        let action = match metadata.get(HEALTH_CHECK_ACTION_KEY) {
            Some(a) => a.as_str(),
            None => return Ok(false),
        };

        match action {
            HEALTH_CHECK_ACTION_NONCE_HEALTH => {
                info!(relayer_id = %self.relayer.id, "executing targeted nonce health action");

                // Acquire distributed lock to prevent concurrent gap resolution.
                let _lock_guard = if ServerConfig::get_distributed_mode() {
                    if let Some((pool, prefix)) = self.relayer_repository.connection_info() {
                        let lock_key = format!("{prefix}:lock:nonce_health:{}", self.relayer.id);
                        let lock = DistributedLock::new(pool, &lock_key, Duration::from_secs(60));

                        match lock.try_acquire().await {
                            Ok(Some(guard)) => {
                                debug!(lock_key = %lock_key, "acquired distributed lock for nonce health");
                                Some(guard)
                            }
                            Ok(None) => {
                                info!(
                                    relayer_id = %self.relayer.id,
                                    "nonce health already running for this relayer, skipping"
                                );
                                return Ok(true);
                            }
                            Err(e) => {
                                warn!(
                                    relayer_id = %self.relayer.id,
                                    error = %e,
                                    "failed to acquire nonce health lock, skipping"
                                );
                                return Ok(true);
                            }
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                match self.resolve_nonce_gaps().await {
                    Ok(filled) => {
                        info!(
                            relayer_id = %self.relayer.id,
                            gaps_filled = filled,
                            "nonce health action completed"
                        );
                    }
                    Err(e) => {
                        error!(
                            relayer_id = %self.relayer.id,
                            error = %e,
                            "nonce health action failed"
                        );
                        return Err(e);
                    }
                }

                Ok(true)
            }
            _ => {
                warn!(
                    relayer_id = %self.relayer.id,
                    action = %action,
                    "unknown targeted health action, skipping"
                );
                Ok(false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::relayer::{SignDataRequest, SignDataResponse, SignTypedDataRequest},
        jobs::MockJobProducerTrait,
        models::{
            NetworkType, RelayerEvmPolicy, RelayerNetworkPolicy, RelayerRepoModel, RpcConfig,
            SignerError, TransactionStatus,
        },
        repositories::{MockNetworkRepository, MockRelayerRepository, MockTransactionRepository},
        services::{
            provider::MockEvmProviderTrait, signer::DataSignerTrait,
            MockTransactionCounterServiceTrait,
        },
    };
    use async_trait::async_trait;
    use std::future::ready;
    use std::sync::Arc;

    mockall::mock! {
        pub Signer {}

        #[async_trait]
        impl DataSignerTrait for Signer {
            async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, SignerError>;
            async fn sign_typed_data(&self, request: SignTypedDataRequest) -> Result<SignDataResponse, SignerError>;
        }
    }

    fn create_test_evm_network() -> EvmNetwork {
        EvmNetwork {
            network: "mainnet".to_string(),
            rpc_urls: vec![RpcConfig::new(
                "https://mainnet.infura.io/v3/test".to_string(),
            )],
            average_blocktime_ms: 12000,
            is_testnet: false,
            tags: vec!["mainnet".to_string()],
            chain_id: 1,
            required_confirmations: 1,
            features: vec!["eip1559".to_string()],
            symbol: "ETH".to_string(),
            explorer_urls: None,
            gas_price_cache: None,
        }
    }

    fn create_test_relayer() -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test-relayer-id".to_string(),
            name: "test-relayer".to_string(),
            network: "mainnet".to_string(),
            network_type: NetworkType::Evm,
            signer_id: "test-signer-id".to_string(),
            address: "0x1234567890abcdef".to_string(),
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
            paused: false,
            notification_id: None,
            system_disabled: false,
            custom_rpc_urls: None,
            disabled_reason: None,
        }
    }

    #[allow(clippy::type_complexity)]
    fn setup_mocks() -> (
        MockEvmProviderTrait,
        MockRelayerRepository,
        MockNetworkRepository,
        MockTransactionRepository,
        MockJobProducerTrait,
        MockSigner,
        MockTransactionCounterServiceTrait,
    ) {
        (
            MockEvmProviderTrait::new(),
            MockRelayerRepository::new(),
            MockNetworkRepository::new(),
            MockTransactionRepository::new(),
            MockJobProducerTrait::new(),
            MockSigner::new(),
            MockTransactionCounterServiceTrait::new(),
        )
    }

    fn make_tx_with_status(status: TransactionStatus) -> TransactionRepoModel {
        TransactionRepoModel {
            id: "tx-id".to_string(),
            relayer_id: "test-relayer-id".to_string(),
            status,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_find_active_tx_for_nonce_with_submitted_tx_returns_some() {
        let (provider, relayer_repo, network_repo, mut tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        tx_repo
            .expect_find_by_nonce()
            .returning(|_, _| Ok(Some(make_tx_with_status(TransactionStatus::Submitted))));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.find_active_tx_for_nonce(5).await.unwrap();
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_find_active_tx_for_nonce_with_failed_tx_returns_none() {
        let (provider, relayer_repo, network_repo, mut tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        tx_repo
            .expect_find_by_nonce()
            .returning(|_, _| Ok(Some(make_tx_with_status(TransactionStatus::Failed))));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.find_active_tx_for_nonce(5).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_find_active_tx_for_nonce_with_no_tx_returns_none() {
        let (provider, relayer_repo, network_repo, mut tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        tx_repo.expect_find_by_nonce().returning(|_, _| Ok(None));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.find_active_tx_for_nonce(5).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_detect_nonce_gaps_with_gap() {
        let (
            mut provider,
            relayer_repo,
            network_repo,
            mut tx_repo,
            job_producer,
            signer,
            mut counter,
        ) = setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(5u64))));

        counter
            .expect_get()
            .returning(|| Box::pin(ready(Ok(Some(8u64)))));

        // nonce 5 → Submitted (active), 6 → Failed (gap), 7 → Sent (active)
        tx_repo
            .expect_find_by_nonce()
            .returning(|_, nonce| match nonce {
                5 => Ok(Some(make_tx_with_status(TransactionStatus::Submitted))),
                6 => Ok(Some(make_tx_with_status(TransactionStatus::Failed))),
                7 => Ok(Some(make_tx_with_status(TransactionStatus::Sent))),
                _ => Ok(None),
            });

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let gaps = relayer.detect_nonce_gaps(None, None).await.unwrap();
        assert_eq!(gaps, vec![6u64]);
    }

    #[tokio::test]
    async fn test_detect_nonce_gaps_no_gaps() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, mut counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(5u64))));

        counter
            .expect_get()
            .returning(|| Box::pin(ready(Ok(Some(5u64)))));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let gaps = relayer.detect_nonce_gaps(None, None).await.unwrap();
        assert!(gaps.is_empty());
    }

    #[tokio::test]
    async fn test_detect_nonce_gaps_counter_behind_chain() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, mut counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(10u64))));

        counter
            .expect_get()
            .returning(|| Box::pin(ready(Ok(Some(5u64)))));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let gaps = relayer.detect_nonce_gaps(None, None).await.unwrap();
        assert!(gaps.is_empty());
    }

    #[tokio::test]
    async fn test_resolve_nonce_gaps_no_gaps() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, mut counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        // get_on_chain_nonce + sync_nonce both call get_transaction_count
        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(5u64))));

        // sync_nonce calls get() then set(); detect_nonce_gaps calls get() again
        counter
            .expect_get()
            .returning(|| Box::pin(ready(Ok(Some(5u64)))));

        counter.expect_set().returning(|_| Box::pin(ready(Ok(()))));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.resolve_nonce_gaps().await.unwrap();
        assert_eq!(result, 0);
    }

    #[tokio::test]
    async fn test_resolve_nonce_gaps_fills_gaps() {
        use crate::config::{EvmNetworkConfig, NetworkConfigCommon};

        let (
            mut provider,
            relayer_repo,
            mut network_repo,
            mut tx_repo,
            mut job_producer,
            signer,
            mut counter,
        ) = setup_mocks();
        let relayer_model = create_test_relayer();

        // get_on_chain_nonce + sync_nonce both call get_transaction_count
        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(5u64))));

        // sync_nonce calls get() then set(); detect_nonce_gaps calls get() again
        counter
            .expect_get()
            .returning(|| Box::pin(ready(Ok(Some(8u64)))));

        counter.expect_set().returning(|_| Box::pin(ready(Ok(()))));

        // detect_nonce_gaps: nonce 5 → active, 6 → gap, 7 → active
        // resolve_nonce_gaps double-check: nonce 6 → still gap (Failed)
        tx_repo
            .expect_find_by_nonce()
            .returning(|_, nonce| match nonce {
                5 => Ok(Some(make_tx_with_status(TransactionStatus::Submitted))),
                6 => Ok(Some(make_tx_with_status(TransactionStatus::Failed))),
                7 => Ok(Some(make_tx_with_status(TransactionStatus::Sent))),
                _ => Ok(None),
            });

        // create_gap_filling_noop needs network repo
        let config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "mainnet".to_string(),
                from: None,
                rpc_urls: Some(vec![crate::models::RpcConfig::new(
                    "https://mainnet.infura.io/v3/test".to_string(),
                )]),
                explorer_urls: None,
                average_blocktime_ms: Some(12000),
                is_testnet: Some(false),
                tags: Some(vec!["mainnet".to_string()]),
            },
            chain_id: Some(1),
            required_confirmations: Some(1),
            features: Some(vec!["eip1559".to_string()]),
            symbol: Some("ETH".to_string()),
            gas_price_cache: None,
        };
        let network_model = NetworkRepoModel::new_evm(config);

        network_repo
            .expect_get_by_name()
            .returning(move |_, _| Ok(Some(network_model.clone())));

        tx_repo.expect_create().returning(Ok);

        job_producer
            .expect_produce_transaction_request_job()
            .returning(|_, _| Box::pin(ready(Ok(()))));

        job_producer
            .expect_produce_check_transaction_status_job()
            .returning(|_, _| Box::pin(ready(Ok(()))));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.resolve_nonce_gaps().await.unwrap();
        assert_eq!(result, 1);
    }

    #[tokio::test]
    async fn test_handle_health_action_no_action_returns_false() {
        let (provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let metadata = HashMap::new();
        let result = relayer.handle_health_action(&metadata).await.unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn test_handle_health_action_unknown_action_returns_false() {
        let (provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let mut metadata = HashMap::new();
        metadata.insert(HEALTH_CHECK_ACTION_KEY.to_string(), "unknown".to_string());
        let result = relayer.handle_health_action(&metadata).await.unwrap();
        assert!(!result);
    }
}
