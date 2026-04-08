use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, info, instrument};

use crate::{
    domain::Transaction,
    jobs::{JobProducerTrait, StatusCheckContext, TransactionStatusCheck},
    models::{
        MidnightNetwork, NetworkTransactionRequest, NetworkType, RelayerRepoModel,
        TransactionError, TransactionRepoModel, TransactionStatus, TransactionUpdateRequest,
    },
    repositories::{
        RelayerRepository, RelayerStateRepositoryStorage, Repository, SyncStateTrait,
        TransactionRepository,
    },
    services::{
        provider::MidnightProviderTrait,
        signer::MidnightSigner,
        sync::midnight::{indexer::ApplyStage, SyncManager},
    },
    utils::calculate_scheduled_timestamp,
};

/// Midnight transaction handler implementing the full lifecycle:
/// prepare → submit → check status.
///
/// Key differences from other networks:
/// - **No cancellation/replacement** — Midnight doesn't support nonce-based replacement
/// - **Dual hash tracking** — both extrinsic_tx_hash (Substrate) and pallet_tx_hash (application)
/// - **Status via indexer** — transaction status is queried from the GraphQL indexer, not RPC
pub struct MidnightTransaction<P, TR, RR, J, SS = RelayerStateRepositoryStorage>
where
    P: MidnightProviderTrait + Send + Sync,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    SS: SyncStateTrait + Send + Sync,
{
    pub relayer: RelayerRepoModel,
    pub network: MidnightNetwork,
    pub provider: Arc<P>,
    pub signer: Arc<MidnightSigner>,
    pub sync_manager: SyncManager<SS>,
    pub transaction_repository: Arc<TR>,
    pub relayer_repository: Arc<RR>,
    pub job_producer: Arc<J>,
}

/// Type alias for the default concrete transaction handler.
pub type DefaultMidnightTransaction = MidnightTransaction<
    crate::services::provider::MidnightProvider,
    crate::repositories::TransactionRepositoryStorage,
    crate::repositories::RelayerRepositoryStorage,
    crate::jobs::JobProducer,
    RelayerStateRepositoryStorage,
>;

impl<P, TR, RR, J, SS> MidnightTransaction<P, TR, RR, J, SS>
where
    P: MidnightProviderTrait + Send + Sync,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync,
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
        transaction_repository: Arc<TR>,
        relayer_repository: Arc<RR>,
        job_producer: Arc<J>,
    ) -> Result<Self, TransactionError> {
        Ok(Self {
            relayer,
            network,
            provider,
            signer,
            sync_manager,
            transaction_repository,
            relayer_repository,
            job_producer,
        })
    }

    /// Schedule a status check job for a submitted transaction.
    async fn schedule_status_check(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<(), TransactionError> {
        // Use absolute timestamp consistent with EVM/Stellar patterns
        let delay_seconds = self
            .network
            .average_blocktime()
            .map(|d| d.as_secs() as i64)
            .unwrap_or(6);

        self.job_producer
            .produce_check_transaction_status_job(
                TransactionStatusCheck::new(
                    tx.id.clone(),
                    self.relayer.id.clone(),
                    NetworkType::Midnight,
                ),
                Some(calculate_scheduled_timestamp(delay_seconds)),
            )
            .await
            .map_err(|e| {
                TransactionError::UnexpectedError(format!("Failed to enqueue status check: {e}"))
            })?;

        Ok(())
    }
}

#[async_trait]
impl<P, TR, RR, J, SS> Transaction for MidnightTransaction<P, TR, RR, J, SS>
where
    P: MidnightProviderTrait + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    SS: SyncStateTrait + Send + Sync + 'static,
{
    #[instrument(
        level = "debug",
        skip(self, tx),
        fields(tx_id = %tx.id, relayer_id = %tx.relayer_id)
    )]
    async fn prepare_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(tx_id = %tx.id, "Preparing Midnight transaction");

        // Full implementation requires:
        // 1. Incremental wallet sync (LedgerContext populated from indexer)
        // 2. Build UnshieldedOfferInfo from request inputs/outputs
        // 3. Generate ZK proofs via the remote proof server
        // 4. Serialize proven transaction via midnight-node-ledger-helpers::serialize()
        //
        // Until the LedgerContext is fully populated from shielded sync events,
        // we cannot construct valid Midnight extrinsics.
        Err(TransactionError::NotSupported(
            "Midnight transaction preparation is not yet fully implemented. \
             The LedgerContext must be populated from indexer wallet sync events \
             before transactions can be built and serialized."
                .into(),
        ))
    }

    #[instrument(
        level = "debug",
        skip(self, tx),
        fields(tx_id = %tx.id, relayer_id = %tx.relayer_id)
    )]
    async fn submit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(tx_id = %tx.id, "Submitting Midnight transaction");

        // Submit requires a serialized extrinsic in the network_data.
        // This is populated during prepare_transaction (which builds and proves the tx).
        // If prepare hasn't run or didn't produce serialized data, fail explicitly.
        let midnight_data = tx.network_data.get_midnight_transaction_data()?;

        let serialized_hex = midnight_data.hash.as_deref().ok_or_else(|| {
            TransactionError::NotSupported(
                "Midnight transaction has no serialized extrinsic. \
                     prepare_transaction must serialize the proven transaction first."
                    .into(),
            )
        })?;

        let result = self
            .provider
            .send_raw_extrinsic(serialized_hex)
            .await
            .map_err(|e| TransactionError::UnexpectedError(format!("Submit failed: {e}")))?;

        info!(
            tx_id = %tx.id,
            extrinsic_hash = %result.extrinsic_tx_hash,
            pallet_hash = ?result.pallet_tx_hash,
            "Midnight transaction submitted"
        );

        // Store extrinsic hash and pallet hash (if returned)
        let mut hashes = tx.hashes.clone();
        hashes.push(result.extrinsic_tx_hash.clone());

        // Store pallet hash separately in network data for status queries
        let updated_network_data = crate::models::MidnightTransactionData {
            hash: Some(result.extrinsic_tx_hash.clone()),
            pallet_hash: result.pallet_tx_hash.clone(),
            block_hash: midnight_data.block_hash.clone(),
            guaranteed_offer: midnight_data.guaranteed_offer.clone(),
            intents: midnight_data.intents.clone(),
            fallible_offers: midnight_data.fallible_offers.clone(),
        };

        self.transaction_repository
            .update_network_data(
                tx.id.clone(),
                crate::models::NetworkTransactionData::Midnight(updated_network_data),
            )
            .await
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        let updated = self
            .transaction_repository
            .partial_update(
                tx.id.clone(),
                TransactionUpdateRequest {
                    status: Some(TransactionStatus::Submitted),
                    hashes: Some(hashes),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        // Schedule a status check
        self.schedule_status_check(&updated).await?;

        Ok(updated)
    }

    async fn resubmit_transaction(
        &self,
        _tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Err(TransactionError::NotSupported(
            "Midnight does not support transaction resubmission".into(),
        ))
    }

    #[instrument(
        level = "debug",
        skip(self, tx, _context),
        fields(tx_id = %tx.id, relayer_id = %tx.relayer_id)
    )]
    async fn handle_transaction_status(
        &self,
        tx: TransactionRepoModel,
        _context: Option<StatusCheckContext>,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let midnight_data = tx.network_data.get_midnight_transaction_data()?;

        // The indexer's `transactions(offset: { hash })` accepts both extrinsic
        // and pallet hashes. Prefer pallet_hash (application-level, more stable)
        // and fall back to the extrinsic hash stored in hashes[].
        let query_hash = midnight_data
            .pallet_hash
            .as_deref()
            .or(midnight_data.hash.as_deref())
            .or(tx.hashes.last().map(String::as_str))
            .ok_or_else(|| {
                TransactionError::UnexpectedError(
                    "No hash available for status query. \
                     Transaction may not have been submitted successfully."
                        .into(),
                )
            })?;

        let indexer = self.provider.get_indexer_client();
        let tx_data = indexer
            .get_transaction_by_hash(query_hash)
            .await
            .map_err(|e| TransactionError::UnexpectedError(format!("Indexer query failed: {e}")))?;

        let (new_status, status_reason) = match tx_data {
            Some(data) => match data.apply_stage {
                Some(ApplyStage::SucceedEntirely) => (
                    TransactionStatus::Confirmed,
                    Some("Transaction succeeded entirely".into()),
                ),
                Some(ApplyStage::SucceedPartially) => (
                    TransactionStatus::Confirmed,
                    Some("Transaction partially succeeded".into()),
                ),
                Some(ApplyStage::FailEntirely) => (
                    TransactionStatus::Failed,
                    Some("Transaction failed entirely".into()),
                ),
                Some(ApplyStage::Pending) | None => {
                    debug!(tx_id = %tx.id, "Transaction still pending, scheduling recheck");
                    self.schedule_status_check(&tx).await?;
                    return Ok(tx);
                }
            },
            None => {
                // Transaction not found in indexer yet — re-check later
                debug!(tx_id = %tx.id, "Transaction not found in indexer yet");
                self.schedule_status_check(&tx).await?;
                return Ok(tx);
            }
        };

        info!(
            tx_id = %tx.id,
            status = ?new_status,
            "Midnight transaction reached final state"
        );

        let confirmed_at = if new_status == TransactionStatus::Confirmed {
            Some(chrono::Utc::now().to_rfc3339())
        } else {
            None
        };

        let updated = self
            .transaction_repository
            .partial_update(
                tx.id.clone(),
                TransactionUpdateRequest {
                    status: Some(new_status),
                    status_reason,
                    confirmed_at,
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        Ok(updated)
    }

    async fn cancel_transaction(
        &self,
        _tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Err(TransactionError::NotSupported(
            "Midnight does not support transaction cancellation".into(),
        ))
    }

    async fn replace_transaction(
        &self,
        _old_tx: TransactionRepoModel,
        _new_tx_request: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Err(TransactionError::NotSupported(
            "Midnight does not support transaction replacement".into(),
        ))
    }

    async fn sign_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        // Midnight transactions are signed as part of the prepare step
        // (ZK proof generation includes signing). This is a no-op.
        Ok(tx)
    }

    async fn validate_transaction(
        &self,
        _tx: TransactionRepoModel,
    ) -> Result<bool, TransactionError> {
        // Basic validation — a full implementation would check TTL, offer structure, etc.
        Ok(true)
    }
}
