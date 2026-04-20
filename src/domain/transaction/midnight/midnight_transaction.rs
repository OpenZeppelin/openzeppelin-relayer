use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, info, instrument};

use crate::{
    domain::Transaction,
    jobs::{JobProducerTrait, StatusCheckContext, TransactionSend, TransactionStatusCheck},
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
        sync::midnight::{indexer::ApplyStage, LedgerContextManager, SyncManager},
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
    pub ledger_ctx: Arc<LedgerContextManager>,
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
        ledger_ctx: Arc<LedgerContextManager>,
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
            ledger_ctx,
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

        let midnight_data = tx.network_data.get_midnight_transaction_data()?;

        // Build the transaction using StandardTrasactionInfo.
        //
        // The flow:
        // 1. Create StandardTrasactionInfo with LedgerContext + proof provider
        // 2. Set unshielded offer from the request's inputs/outputs
        // 3. Call .build() which: computes TTL, builds offers, pays DUST fees, proves
        // 4. Serialize the proven transaction
        //
        // PREREQUISITE: The LedgerContext must have the current chain state
        // (parameters, network_id, UTXOs). This is populated during relayer
        // initialization via the shielded/unshielded sync, and the ledger state
        // can also be bootstrapped from the RPC node.
        let context = self.ledger_ctx.context().clone();

        // Check if the context has a valid network_id (indicates it was populated)
        let has_state = context.with_ledger_state(|state| !state.network_id.is_empty());

        if !has_state {
            return Err(TransactionError::NotSupported(
                "LedgerContext has no chain state. The relayer must complete \
                 initial sync before transactions can be prepared. Ensure the \
                 node RPC is reachable and the indexer sync completed."
                    .into(),
            ));
        }

        // Create proof provider and transaction builder
        use midnight_node_ledger_helpers::{
            BuildUtxoOutput, BuildUtxoSpend, DefaultDB, FromContext, IntentInfo,
            StandardTrasactionInfo, UnshieldedOfferInfo, UnshieldedWallet, UtxoOutputInfo,
            UtxoSpendInfo, WalletAddress, WalletSeed, NIGHT,
        };
        use std::str::FromStr;

        let proof_server = Arc::new(crate::services::provider::RemoteProofServer::new(
            self.network.prover_url.clone(),
        ));

        let wallet_seed = self.ledger_ctx.wallet_seed().clone();
        let mut tx_info =
            StandardTrasactionInfo::new_from_context(context.clone(), proof_server, None);

        // Pay DUST fees from the wallet. Requires DUST to have been
        // registered and generated via the midnight-dust-generator tool.
        tx_info.set_funding_seeds(vec![wallet_seed.clone()]);

        // DUST trees are synced from wallet to LedgerState via the patched
        // midnight-ledger crate (DustLocalState fields made pub).

        // Build unshielded offer from request data
        if let Some(ref offer_req) = midnight_data.guaranteed_offer {
            let mut inputs: Vec<Box<dyn BuildUtxoSpend<DefaultDB>>> = Vec::new();
            let mut outputs: Vec<Box<dyn BuildUtxoOutput<DefaultDB>>> = Vec::new();

            for input in &offer_req.inputs {
                let value: u128 = input.value.parse().map_err(|_| {
                    TransactionError::ValidationError(format!(
                        "Invalid input value: {}",
                        input.value
                    ))
                })?;

                // The relayer can only spend UTXOs owned by its own wallet.
                // `origin` is accepted for API symmetry but the owner is always
                // the relayer's wallet_seed — any other value would be a request
                // the relayer cannot fulfill.
                inputs.push(Box::new(UtxoSpendInfo {
                    value,
                    owner: wallet_seed.clone(),
                    token_type: NIGHT,
                    intent_hash: None,
                    output_number: None,
                }));
            }

            for output in &offer_req.outputs {
                let value: u128 = output.value.parse().map_err(|_| {
                    TransactionError::ValidationError(format!(
                        "Invalid output value: {}",
                        output.value
                    ))
                })?;

                // Destinations other than "self" must be an unshielded bech32m
                // address (e.g. mn_addr_preview1…). "self" loops back to the
                // relayer's own wallet (useful for tests and change outputs).
                if output.destination == "self" {
                    outputs.push(Box::new(UtxoOutputInfo {
                        value,
                        owner: wallet_seed.clone(),
                        token_type: NIGHT,
                    }));
                } else {
                    let wallet_addr =
                        WalletAddress::from_str(&output.destination).map_err(|e| {
                            TransactionError::ValidationError(format!(
                                "Invalid destination address {}: {e}",
                                output.destination
                            ))
                        })?;
                    let unshielded = UnshieldedWallet::try_from(&wallet_addr).map_err(|e| {
                        TransactionError::ValidationError(format!(
                            "Destination {} is not an unshielded address: {e:?}",
                            output.destination
                        ))
                    })?;
                    outputs.push(Box::new(UtxoOutputInfo {
                        value,
                        owner: unshielded,
                        token_type: NIGHT,
                    }));
                }
            }

            let unshielded_offer = UnshieldedOfferInfo { inputs, outputs };

            // Wrap in an intent and add to the transaction.
            // Use segment 1 (fallible) when DUST registration is included,
            // since DUST actions require fallible segments only.
            let segment_id = 1; // fallible segment
            tx_info.add_intent(
                segment_id,
                Box::new(IntentInfo {
                    guaranteed_unshielded_offer: None,
                    fallible_unshielded_offer: Some(unshielded_offer),
                    actions: vec![],
                }),
            );
        }

        if tx_info.is_empty() {
            return Err(TransactionError::ValidationError(
                "Transaction has no offers or intents to build".into(),
            ));
        }

        // Build and prove the transaction
        info!(tx_id = %tx.id, "Building and proving Midnight transaction");
        let proven_tx = tx_info.prove().await.map_err(|e| {
            TransactionError::UnexpectedError(format!("Transaction build/prove failed: {e}"))
        })?;

        // Serialize using tagged serialization
        let serialized_bytes =
            midnight_node_ledger_helpers::serialize(&proven_tx).map_err(|e| {
                TransactionError::UnexpectedError(format!("Transaction serialization failed: {e}"))
            })?;

        let serialized_hex = hex::encode(&serialized_bytes);

        // Compute the pallet transaction hash
        let tx_hash = proven_tx.transaction_hash();
        let pallet_hash = hex::encode(tx_hash.0 .0);

        info!(
            tx_id = %tx.id,
            serialized_len = serialized_bytes.len(),
            pallet_hash = %pallet_hash,
            "Midnight transaction prepared and serialized"
        );

        // Store the serialized transaction and pallet hash in network data.
        // `hash` stays None until submit_transaction records the extrinsic hash
        // from Substrate — keeping serialized bytes out of the hash field keeps
        // the API surface honest and lets callers distinguish prepared vs sent.
        let updated_network_data = crate::models::MidnightTransactionData {
            hash: None,
            pallet_hash: Some(pallet_hash),
            block_hash: None,
            serialized_tx: Some(serialized_hex),
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
                    status: Some(TransactionStatus::Sent),
                    sent_at: Some(chrono::Utc::now().to_rfc3339()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        // Enqueue the submit job so the prepared transaction gets pushed to
        // the node. Every other network does this at the tail of prepare;
        // without it the tx would sit in Sent forever.
        self.job_producer
            .produce_submit_transaction_job(
                TransactionSend::submit(updated.id.clone(), updated.relayer_id.clone()),
                None,
            )
            .await
            .map_err(|e| {
                TransactionError::UnexpectedError(format!("Failed to enqueue submit job: {e}"))
            })?;

        Ok(updated)
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

        let serialized_hex = midnight_data.serialized_tx.as_deref().ok_or_else(|| {
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

        // Preserve serialized_tx so resubmit paths can replay without re-proving.
        // The pallet_hash set during prepare is authoritative for indexer queries;
        // prefer it over whatever the provider returned.
        let updated_network_data = crate::models::MidnightTransactionData {
            hash: Some(result.extrinsic_tx_hash.clone()),
            pallet_hash: midnight_data
                .pallet_hash
                .clone()
                .or(result.pallet_tx_hash.clone()),
            block_hash: midnight_data.block_hash.clone(),
            serialized_tx: midnight_data.serialized_tx.clone(),
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
