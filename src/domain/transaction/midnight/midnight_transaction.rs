//! Midnight transaction implementation
//!
//! This module provides the core transaction handling logic for Midnight network transactions.

use crate::{
    domain::{
        SignTransactionResponse, Transaction,
        midnight::{DUST_TOKEN_TYPE, MidnightTransactionBuilder, to_midnight_network_id},
    },
    jobs::{JobProducer, JobProducerTrait, TransactionSend, TransactionStatusCheck},
    models::{
        MidnightNetwork, MidnightOfferRequest, NetworkTransactionData, NetworkTransactionRequest,
        NetworkType, RelayerRepoModel, TransactionError, TransactionRepoModel, TransactionStatus,
        TransactionUpdateRequest, produce_transaction_update_notification_payload,
    },
    repositories::{
        RelayerRepositoryStorage, Repository, TransactionCounterRepositoryStorage,
        TransactionCounterTrait, TransactionRepository, TransactionRepositoryStorage,
    },
    services::{
        midnight::handler::{QuickSyncStrategy, SyncManager},
        provider::{
            MidnightProvider, MidnightProviderTrait, TransactionSubmissionResult,
            remote_prover::RemoteProofServer,
        },
        signer::{MidnightSigner, MidnightSignerTrait},
        sync::midnight::indexer::ApplyStage,
    },
};
use async_trait::async_trait;
use chrono::Utc;
use log::{debug, info};
use midnight_node_ledger_helpers::{
    DefaultDB, InputInfo, LedgerContext, OfferInfo, OutputInfo, WalletSeed,
};
use rand::Rng;
use std::sync::Arc;
use tokio::sync::Mutex;

#[allow(dead_code)]
/// Midnight transaction handler with generic dependencies
pub struct MidnightTransaction<P, R, T, J, S, C>
where
    P: MidnightProviderTrait,
    R: Repository<RelayerRepoModel, String>,
    T: TransactionRepository,
    J: JobProducerTrait,
    S: MidnightSignerTrait,
    C: TransactionCounterTrait,
{
    relayer: RelayerRepoModel,
    provider: Arc<P>,
    relayer_repository: Arc<R>,
    transaction_repository: Arc<T>,
    job_producer: Arc<J>,
    signer: Arc<S>,
    transaction_counter_service: Arc<C>,
    sync_manager: Arc<Mutex<SyncManager<QuickSyncStrategy>>>,
    network: MidnightNetwork,
}

#[allow(dead_code, clippy::too_many_arguments)]
impl<P, R, T, J, S, C> MidnightTransaction<P, R, T, J, S, C>
where
    P: MidnightProviderTrait,
    R: Repository<RelayerRepoModel, String>,
    T: TransactionRepository,
    J: JobProducerTrait,
    S: MidnightSignerTrait,
    C: TransactionCounterTrait,
{
    /// Creates a new `MidnightTransaction`.
    ///
    /// # Arguments
    ///
    /// * `relayer` - The relayer model.
    /// * `provider` - The Midnight provider.
    /// * `relayer_repository` - Storage for relayer repository.
    /// * `transaction_repository` - Storage for transaction repository.
    /// * `job_producer` - Producer for job queue.
    /// * `signer` - The signer service.
    /// * `transaction_counter_service` - Service for managing transaction counters.
    /// * `sync_manager` - Sync manager.
    /// * `network` - The Midnight network configuration.
    ///
    /// # Returns
    ///
    /// A result containing the new `MidnightTransaction` or a `TransactionError`.
    pub fn new(
        relayer: RelayerRepoModel,
        provider: Arc<P>,
        relayer_repository: Arc<R>,
        transaction_repository: Arc<T>,
        job_producer: Arc<J>,
        signer: Arc<S>,
        transaction_counter_service: Arc<C>,
        sync_manager: Arc<Mutex<SyncManager<QuickSyncStrategy>>>,
        network: MidnightNetwork,
    ) -> Result<Self, TransactionError> {
        Ok(Self {
            relayer,
            provider,
            relayer_repository,
            transaction_repository,
            job_producer,
            signer,
            transaction_counter_service,
            sync_manager,
            network,
        })
    }

    /// Returns a reference to the provider.
    pub fn provider(&self) -> &P {
        &self.provider
    }

    /// Returns a reference to the relayer model.
    pub fn relayer(&self) -> &RelayerRepoModel {
        &self.relayer
    }

    /// Returns a reference to the job producer.
    pub fn job_producer(&self) -> &J {
        &self.job_producer
    }

    /// Returns a reference to the transaction repository.
    pub fn transaction_repository(&self) -> &T {
        &self.transaction_repository
    }

    /// Returns a reference to the network configuration.
    pub fn network(&self) -> &MidnightNetwork {
        &self.network
    }

    /// Returns a reference to the sync manager.
    pub fn sync_manager(&self) -> &Arc<Mutex<SyncManager<QuickSyncStrategy>>> {
        &self.sync_manager
    }

    /// Enqueue a submit-transaction job for the given transaction.
    pub async fn enqueue_submit(&self, tx: &TransactionRepoModel) -> Result<(), TransactionError> {
        let job = TransactionSend::submit(tx.id.clone(), tx.relayer_id.clone());
        self.job_producer()
            .produce_submit_transaction_job(job, None)
            .await?;
        Ok(())
    }

    /// Sends a transaction update notification if a notification ID is configured.
    pub(super) async fn send_transaction_update_notification(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<(), TransactionError> {
        if let Some(notification_id) = &self.relayer().notification_id {
            self.job_producer()
                .produce_send_notification_job(
                    produce_transaction_update_notification_payload(notification_id, tx),
                    None,
                )
                .await
                .map_err(|e| {
                    TransactionError::UnexpectedError(format!("Failed to send notification: {}", e))
                })?;
        }
        Ok(())
    }

    /// Convert API offer request to Midnight's OfferInfo type
    /// This method handles UTXO selection, fee calculation, and change output creation
    async fn convert_offer_request_to_offer_info(
        &self,
        offer_request: &MidnightOfferRequest,
        from_wallet_seed: WalletSeed,
        context: &Arc<LedgerContext<DefaultDB>>,
    ) -> Result<OfferInfo<DefaultDB>, TransactionError> {
        // Validate we have at least one input
        if offer_request.inputs.is_empty() {
            return Err(TransactionError::ValidationError(
                "At least one input is required".to_string(),
            ));
        }

        let output_requests = &offer_request.outputs;

        // Validate all inputs are from the relayer's wallet
        for input_req in &offer_request.inputs {
            // Parse the origin wallet seed
            let origin_seed_bytes = hex::decode(&input_req.origin).map_err(|e| {
                TransactionError::ValidationError(format!("Invalid origin wallet seed hex: {}", e))
            })?;

            if origin_seed_bytes.len() != 32 {
                return Err(TransactionError::ValidationError(
                    "Wallet seed must be 32 bytes".to_string(),
                ));
            }

            let mut origin_array = [0u8; 32];
            origin_array.copy_from_slice(&origin_seed_bytes);
            let origin_seed = WalletSeed::Medium(origin_array);

            // Verify the origin matches the relayer's wallet
            if origin_seed != from_wallet_seed {
                return Err(TransactionError::ValidationError(
                    "All input origins must match relayer wallet".to_string(),
                ));
            }
        }

        // Calculate total output amount
        let mut total_output_amount = 0u128;
        // Convert TokenType::Dust to ShieldedTokenType for InputInfo/OutputInfo
        // Dust uses a special zero hash representation
        use midnight_node_ledger_helpers::{HashOutput, ShieldedTokenType, TokenType};
        let token_type = match DUST_TOKEN_TYPE {
            TokenType::Dust => ShieldedTokenType(HashOutput([0u8; 32])),
            TokenType::Shielded(st) => st,
            TokenType::Unshielded(_) => {
                return Err(TransactionError::ValidationError(
                    "Unshielded tokens not supported".to_string(),
                ));
            }
        };

        for output_req in output_requests {
            let amount = output_req.value.parse::<u128>().map_err(|e| {
                TransactionError::ValidationError(format!("Invalid output value: {}", e))
            })?;
            total_output_amount += amount;
        }

        // Calculate fee based on transaction structure
        // Use actual number of inputs and potentially outputs.len() + 1 outputs (including change)
        let num_inputs = offer_request.inputs.len();
        let num_outputs = output_requests.len() + 1; // +1 for potential change output

        // // Use Wallet::calculate_fee to get the minimum fee
        // let calculated_fee = midnight_node_ledger_helpers::Wallet::<DefaultDB>::calculate_fee(
        // 	num_inputs,
        // 	num_outputs
        // );

        // The Wallet::calculate_fee method seems to overestimate fees significantly
        // Based on observed fees from actual transactions:
        // - 1 input, 2 outputs = ~60,855 tDUST
        // - Add ~20k tDUST for each additional input/output
        let base_fee = 61000u128;
        let per_input_fee = if num_inputs > 1 {
            (num_inputs - 1) as u128 * 20000
        } else {
            0
        };
        let per_output_fee = if num_outputs > 2 {
            (num_outputs - 2) as u128 * 20000
        } else {
            0
        };
        let calculated_fee = base_fee + per_input_fee + per_output_fee;

        debug!(
            "Transaction with {} inputs, {} outputs: {} tDUST + {} tDUST fee = {} tDUST total",
            num_inputs,
            num_outputs,
            total_output_amount,
            calculated_fee,
            total_output_amount + calculated_fee
        );

        // Get the wallet to find available UTXOs
        let from_wallet = context.wallet_from_seed(from_wallet_seed);

        // Build the offer
        let mut offer = OfferInfo::default();

        // Process each input request
        let mut total_input_amount = 0u128;
        for input_req in &offer_request.inputs {
            // Parse input value
            let requested_value = input_req.value.parse::<u128>().map_err(|e| {
                TransactionError::ValidationError(format!("Invalid input value: {}", e))
            })?;

            // Create a temporary input_info to find the minimum UTXO that can cover this input
            let temp_input_info = InputInfo::<WalletSeed> {
                origin: from_wallet_seed,
                token_type,
                value: requested_value,
            };

            // Find the actual UTXO that will be selected for this input
            let selected_coin = temp_input_info.min_match_coin(&from_wallet.shielded.state);
            let actual_utxo_value = selected_coin.value;

            debug!(
                "Selected UTXO with value: {} tDUST for requested value: {} tDUST",
                actual_utxo_value, requested_value
            );

            // Create the actual input with the exact UTXO value
            let input_info = InputInfo::<WalletSeed> {
                origin: from_wallet_seed,
                token_type,
                value: actual_utxo_value, // Use the exact value of the selected UTXO
            };

            offer.inputs.push(Box::new(input_info));
            total_input_amount += actual_utxo_value;
        }

        // Verify total inputs can cover outputs + fees
        if total_input_amount < total_output_amount + calculated_fee {
            return Err(TransactionError::InsufficientBalance(format!(
                "Total input value {} is insufficient for outputs {} + fee {} = {} tDUST",
                total_input_amount,
                total_output_amount,
                calculated_fee,
                total_output_amount + calculated_fee
            )));
        }

        // Add all requested outputs
        for output_req in output_requests {
            // Parse destination wallet seed
            let dest_seed_bytes = hex::decode(&output_req.destination).map_err(|e| {
                TransactionError::ValidationError(format!(
                    "Invalid destination wallet seed hex: {}",
                    e
                ))
            })?;

            if dest_seed_bytes.len() != 32 {
                return Err(TransactionError::ValidationError(
                    "Wallet seed must be 32 bytes".to_string(),
                ));
            }

            let mut dest_array = [0u8; 32];
            dest_array.copy_from_slice(&dest_seed_bytes);
            let dest_seed = WalletSeed::Medium(dest_array);

            // Parse amount
            let value = output_req.value.parse::<u128>().map_err(|e| {
                TransactionError::ValidationError(format!("Invalid output value: {}", e))
            })?;

            // For now, we only support DUST_TOKEN_TYPE
            let output_info = OutputInfo::<WalletSeed> {
                destination: dest_seed,
                token_type, // Always DUST_TOKEN_TYPE
                value,
            };

            offer.outputs.push(Box::new(output_info));
            debug!(
                "Added output: {} tDUST to {:?}",
                value,
                hex::encode(dest_seed.as_bytes())
            );
        }

        // Calculate change and create change output if needed
        // This ensures the indexer recognizes the transaction as relevant to the sender
        let change_amount = total_input_amount.saturating_sub(total_output_amount + calculated_fee);
        if change_amount > 0 {
            let change_output = OutputInfo::<WalletSeed> {
                destination: from_wallet_seed, // Send change back to sender
                token_type,
                value: change_amount,
            };
            offer.outputs.push(Box::new(change_output));
            debug!(
                "Added change output: {} tDUST back to sender",
                change_amount
            );
        } else {
            debug!("No change output needed (exact amount)");
        }

        Ok(offer)
    }

    /// Helper method to schedule a transaction status check job.
    pub(super) async fn schedule_status_check(
        &self,
        tx: &TransactionRepoModel,
        delay_seconds: Option<i64>,
    ) -> Result<(), TransactionError> {
        let delay = delay_seconds.map(|seconds| Utc::now().timestamp() + seconds);
        self.job_producer()
            .produce_check_transaction_status_job(
                TransactionStatusCheck::new(
                    tx.id.clone(),
                    tx.relayer_id.clone(),
                    NetworkType::Midnight,
                ),
                delay,
            )
            .await
            .map_err(|e| {
                TransactionError::UnexpectedError(format!("Failed to schedule status check: {}", e))
            })
    }
}

#[async_trait]
impl<P, R, T, J, S, C> Transaction for MidnightTransaction<P, R, T, J, S, C>
where
    P: MidnightProviderTrait + Send + Sync,
    R: Repository<RelayerRepoModel, String> + Send + Sync,
    T: TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    S: MidnightSignerTrait + Send + Sync,
    C: TransactionCounterTrait + Send + Sync,
{
    async fn prepare_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        log::debug!("Preparing Midnight transaction: {}", tx.id);

        // Extract Midnight-specific data
        let midnight_data = tx.network_data.get_midnight_transaction_data()?;
        let wallet_seed = self.signer.wallet_seed();

        // Perform incremental sync - the sync manager will automatically
        // read from the last synced blockchain index stored for this relayer
        let mut sync_manager = self.sync_manager.lock().await;
        sync_manager
            .sync_incremental()
            .await
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        let context = sync_manager.get_context();
        drop(sync_manager);

        // Check balance
        let balance = self.provider.get_balance(wallet_seed, &context).await?;
        info!("Wallet balance: {} tDUST", balance);

        // Create proof provider
        let proof_provider = Box::new(RemoteProofServer::new(
            self.network.prover_url.clone(),
            to_midnight_network_id(&self.network.network),
        ));

        // Generate cryptographically secure random seed with timestamp for uniqueness
        let mut rng_seed = [0u8; 32];
        rand::rng().fill(&mut rng_seed);

        // Mix in current timestamp to ensure uniqueness across transaction attempts
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let timestamp_bytes = timestamp.to_le_bytes();

        // XOR the first 8 bytes of the seed with timestamp for uniqueness
        for (i, &byte) in timestamp_bytes.iter().enumerate() {
            rng_seed[i] ^= byte;
        }

        // Build transaction based on request data
        let mut builder = MidnightTransactionBuilder::<DefaultDB>::new()
            .with_context(context.clone())
            .with_proof_provider({
                // Convert Box<RemoteProofServer> to Arc<dyn ProofProvider<D>>
                // RemoteProofServer implements ProofProvider, so we can convert it
                use midnight_node_ledger_helpers::ProofProvider;
                let provider: Arc<RemoteProofServer> = Arc::from(*proof_provider);
                provider as Arc<dyn ProofProvider<DefaultDB>>
            })
            .with_rng_seed(rng_seed);

        // Convert and add guaranteed offer if present
        if let Some(offer_request) = &midnight_data.guaranteed_offer {
            let offer = self
                .convert_offer_request_to_offer_info(offer_request, *wallet_seed, &context)
                .await?;
            builder = builder.with_guaranteed_offer(offer);
        }

        if !midnight_data.intents.is_empty() || !midnight_data.fallible_offers.is_empty() {
            return Err(TransactionError::NotSupported(
                "Contract interactions not yet supported".to_string(),
            ));
        }

        // Build and prove the transaction
        let proven_transaction = builder.build().await?;

        // Serialize the transaction using Midnight's serialize function
        let serialized_tx =
            midnight_node_ledger_helpers::serialize(&proven_transaction).map_err(|e| {
                TransactionError::UnexpectedError(format!(
                    "Failed to serialize transaction: {:?}",
                    e
                ))
            })?;

        // Update transaction with prepared data
        let mut updated_midnight_data = midnight_data.clone();
        updated_midnight_data.raw = Some(serialized_tx);

        let update = TransactionUpdateRequest {
            status: Some(TransactionStatus::Sent),
            network_data: Some(NetworkTransactionData::Midnight(updated_midnight_data)),
            priced_at: Some(Utc::now().to_rfc3339()),
            ..Default::default()
        };

        let updated_tx = self
            .transaction_repository
            .partial_update(tx.id.clone(), update)
            .await?;

        self.enqueue_submit(&updated_tx).await?;
        self.send_transaction_update_notification(&updated_tx)
            .await?;

        Ok(updated_tx)
    }

    async fn submit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        // Extract Midnight-specific data
        let midnight_data = tx.network_data.get_midnight_transaction_data()?;

        // Check if we have serialized transaction data
        let serialized_tx = midnight_data.raw.as_ref().ok_or_else(|| {
            TransactionError::UnexpectedError(
                "Transaction not prepared - missing serialized data".to_string(),
            )
        })?;

        // Deserialize the transaction
        use midnight_node_ledger_helpers::{PedersenRandomness, ProofMarker, Signature};
        let transaction = midnight_node_ledger_helpers::deserialize::<
            midnight_node_ledger_helpers::Transaction<
                Signature,
                ProofMarker,
                PedersenRandomness,
                DefaultDB,
            >,
            _,
        >(&serialized_tx[..])
        .map_err(|e| {
            TransactionError::UnexpectedError(format!("Failed to deserialize transaction: {:?}", e))
        })?;

        // Submit to the network
        let result_json = self.provider.send_transaction(transaction).await?;

        // Parse the JSON response
        let result: TransactionSubmissionResult =
            serde_json::from_str(&result_json).map_err(|e| {
                TransactionError::UnexpectedError(format!(
                    "Failed to parse transaction result: {}",
                    e
                ))
            })?;

        // Update transaction with hash and block hash
        let mut updated_midnight_data = midnight_data.clone();
        updated_midnight_data.hash = Some(result.extrinsic_tx_hash.clone());
        updated_midnight_data.pallet_hash = Some(result.pallet_tx_hash.clone());

        let update = TransactionUpdateRequest {
            status: Some(TransactionStatus::Submitted),
            network_data: Some(NetworkTransactionData::Midnight(updated_midnight_data)),
            sent_at: Some(Utc::now().to_rfc3339()),
            hashes: Some(vec![result.extrinsic_tx_hash]),
            ..Default::default()
        };

        let updated_tx = self
            .transaction_repository
            .partial_update(tx.id.clone(), update)
            .await?;

        // Schedule status check
        let job = crate::jobs::TransactionStatusCheck::new(
            updated_tx.id.clone(),
            updated_tx.relayer_id.clone(),
            NetworkType::Midnight,
        );
        self.job_producer()
            .produce_check_transaction_status_job(job, None)
            .await?;

        self.send_transaction_update_notification(&updated_tx)
            .await?;

        Ok(updated_tx)
    }

    async fn resubmit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        // TODO: Implement transaction resubmission
        // This might involve resubmitting only failed segments

        // For now, just return the transaction as-is
        Ok(tx)
    }

    async fn handle_transaction_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        log::debug!("Handling Midnight transaction status: {}", tx.id);

        // If transaction is in a final state, return as-is
        if matches!(
            tx.status,
            TransactionStatus::Confirmed | TransactionStatus::Failed | TransactionStatus::Expired
        ) {
            return Ok(tx);
        }

        // Extract Midnight-specific data
        let midnight_data = tx.network_data.get_midnight_transaction_data()?;

        // Check if we have a transaction hash
        let pallet_tx_hash = midnight_data.pallet_hash.as_ref().ok_or_else(|| {
            TransactionError::UnexpectedError("Transaction hash is missing".to_string())
        })?;

        match self.provider.get_transaction_by_hash(pallet_tx_hash).await {
            Ok(Some(tx_data)) => {
                // Check the applyStage field to determine success/failure
                if let Some(apply_stage_value) = tx_data.get("applyStage") {
                    // Deserialize the applyStage value into the ApplyStage enum
                    let apply_stage: ApplyStage = serde_json::from_value(apply_stage_value.clone())
                        .map_err(|e| {
                            TransactionError::UnexpectedError(format!(
                                "Failed to parse applyStage: {}",
                                e
                            ))
                        })?;

                    match apply_stage {
                        ApplyStage::SucceedEntirely => {
                            // Transaction succeeded entirely
                            let update = TransactionUpdateRequest {
                                status: Some(TransactionStatus::Confirmed),
                                confirmed_at: Some(Utc::now().to_rfc3339()),
                                ..Default::default()
                            };

                            let updated_tx = self
                                .transaction_repository
                                .partial_update(tx.id.clone(), update)
                                .await?;

                            self.send_transaction_update_notification(&updated_tx)
                                .await?;

                            Ok(updated_tx)
                        }
                        ApplyStage::FailEntirely => {
                            // Transaction failed entirely
                            log::warn!("Transaction {} failed entirely", tx.id);

                            let update = TransactionUpdateRequest {
                                status: Some(TransactionStatus::Failed),
                                ..Default::default()
                            };

                            let updated_tx = self
                                .transaction_repository
                                .partial_update(tx.id.clone(), update)
                                .await?;

                            self.send_transaction_update_notification(&updated_tx)
                                .await?;

                            Ok(updated_tx)
                        }
                        ApplyStage::SucceedPartially => {
                            // Partial success - could be expanded to handle segment-specific results
                            log::warn!("Transaction {} succeeded partially", tx.id);

                            // For now, treat any partial success as a failure
                            // In the future, this could be more nuanced
                            let update = TransactionUpdateRequest {
                                status: Some(TransactionStatus::Failed),
                                ..Default::default()
                            };

                            let updated_tx = self
                                .transaction_repository
                                .partial_update(tx.id.clone(), update)
                                .await?;

                            self.send_transaction_update_notification(&updated_tx)
                                .await?;

                            Ok(updated_tx)
                        }
                        ApplyStage::Pending => {
                            // Still pending - schedule another check
                            log::info!("Transaction {} is still pending", tx.id);

                            self.schedule_status_check(&tx, Some(5)).await?;

                            Ok(tx)
                        }
                    }
                } else {
                    // No applyStage field - this shouldn't happen
                    log::error!("Transaction {} missing applyStage field", tx.id);

                    // Schedule another status check
                    self.schedule_status_check(&tx, Some(5)).await?;

                    Ok(tx)
                }
            }
            Ok(None) => {
                // Transaction not found in indexer yet
                log::info!("Transaction {} not found in indexer yet", tx.id);

                // Schedule another status check
                self.schedule_status_check(&tx, Some(5)).await?;

                Ok(tx)
            }
            Err(e) => {
                // Error querying indexer
                log::error!("Error querying indexer for transaction {}: {}", tx.id, e);

                // Schedule another status check
                self.schedule_status_check(&tx, Some(5)).await?;

                Ok(tx)
            }
        }
    }

    async fn cancel_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        // TODO: Implement transaction cancellation
        // Note: Midnight transactions might not be cancellable once submitted

        log::debug!("Cancelling Midnight transaction: {}", tx.id);

        Err(TransactionError::NotSupported(
            "Transaction cancellation is not supported for Midnight".to_string(),
        ))
    }

    async fn replace_transaction(
        &self,
        old_tx: TransactionRepoModel,
        _new_tx_request: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, TransactionError> {
        // TODO: Implement transaction replacement
        // Note: Midnight transactions might not be replaceable

        log::debug!("Replacing Midnight transaction: {}", old_tx.id);

        Err(TransactionError::NotSupported(
            "Transaction replacement is not supported for Midnight".to_string(),
        ))
    }

    async fn sign_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let signature_response = self
            .signer
            .sign_transaction(tx.network_data.clone())
            .await?;

        // Extract the Midnight signature from the response
        let signature = match signature_response {
            SignTransactionResponse::Midnight(midnight_sig) => midnight_sig,
            _ => {
                return Err(TransactionError::InvalidType(
                    "Expected Midnight signature response".to_string(),
                ));
            }
        };

        let mut updated_midnight_data = tx.network_data.get_midnight_transaction_data()?;
        updated_midnight_data.signature = Some(signature.signature);

        let update = TransactionUpdateRequest {
            network_data: Some(NetworkTransactionData::Midnight(updated_midnight_data)),
            ..Default::default()
        };

        let updated_tx = self
            .transaction_repository
            .partial_update(tx.id.clone(), update)
            .await?;

        Ok(updated_tx)
    }

    async fn validate_transaction(
        &self,
        _tx: TransactionRepoModel,
    ) -> Result<bool, TransactionError> {
        // NOTE: This is already handled by the transaction builder in the prepare_transaction method
        Ok(true)
    }
}

/// Default concrete type for Midnight transactions
pub type DefaultMidnightTransaction = MidnightTransaction<
    MidnightProvider,
    RelayerRepositoryStorage,
    TransactionRepositoryStorage,
    JobProducer,
    MidnightSigner,
    TransactionCounterRepositoryStorage,
>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Address;
    use crate::{
        config::network::IndexerUrls,
        domain::{SignTransactionResponse, SignTransactionResponseMidnight},
        jobs::MockJobProducerTrait,
        models::{
            MidnightNetwork, MidnightOfferRequest, MidnightTransactionData,
            MidnightTransactionRequest, NetworkTransactionData, NetworkTransactionRequest,
            NetworkType, RelayerMidnightPolicy, RelayerNetworkPolicy, RelayerRepoModel,
            SignerError, TransactionRepoModel, TransactionStatus, U256,
            midnight::{MidnightInputRequest, MidnightOutputRequest},
        },
        repositories::{
            MockRepository, MockTransactionCounterTrait, MockTransactionRepository,
            RelayerStateRepositoryStorage,
        },
        services::{
            midnight::{handler::SyncManager, indexer::MidnightIndexerClient},
            provider::MockMidnightProviderTrait,
            signer::{MidnightSignerTrait, Signer},
        },
    };
    use chrono::Utc;
    use midnight_node_ledger_helpers::{NetworkId, WalletSeed};
    use mockall::predicate::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;

    // Helper functions for loading test fixtures
    fn get_context_fixture_path(seed: &WalletSeed) -> Option<PathBuf> {
        let seed_hex = hex::encode(seed.as_bytes());
        let fixture_dir = PathBuf::from("tests/fixtures/midnight");

        // Look for any context fixture for this seed
        if let Ok(entries) = fs::read_dir(&fixture_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with(&format!("context_{}_", seed_hex)) && name.ends_with(".bin")
                    {
                        return Some(path);
                    }
                }
            }
        }
        None
    }

    // Helper to create a sync manager with context from fixture
    async fn create_sync_manager_with_fixture(
        seed: &WalletSeed,
        network: &MidnightNetwork,
        relayer_id: String,
    ) -> SyncManager<crate::services::midnight::handler::QuickSyncStrategy> {
        let sync_state_store = Arc::new(RelayerStateRepositoryStorage::new_in_memory());

        // Create sync manager first
        let sync_manager = SyncManager::new(
            &MidnightIndexerClient::new(network.indexer_urls.clone()),
            seed,
            NetworkId::TestNet,
            sync_state_store,
            relayer_id,
        )
        .await
        .unwrap();

        // Try to load and restore context from fixture
        if let Some(fixture_path) = get_context_fixture_path(seed) {
            if let Ok(context_bytes) = fs::read(&fixture_path) {
                eprintln!("Loading context fixture from: {:?}", fixture_path);
                eprintln!("Context fixture size: {} bytes", context_bytes.len());

                // Restore the context directly
                if let Err(e) = sync_manager.restore_context(&context_bytes) {
                    eprintln!("Warning: Failed to restore context from fixture: {:?}", e);
                } else {
                    eprintln!("Successfully restored context from fixture");
                }
            }
        }

        sync_manager
    }

    // Test implementation for MidnightSignerTrait
    struct TestMidnightSigner {
        wallet_seed: WalletSeed,
    }

    #[async_trait]
    impl Signer for TestMidnightSigner {
        async fn address(&self) -> Result<Address, SignerError> {
            Ok(Address::Midnight("test_midnight_address".to_string()))
        }

        async fn sign_transaction(
            &self,
            _transaction: NetworkTransactionData,
        ) -> Result<SignTransactionResponse, SignerError> {
            Ok(SignTransactionResponse::Midnight(
                SignTransactionResponseMidnight {
                    signature: "test_signature".to_string(),
                },
            ))
        }
    }

    impl MidnightSignerTrait for TestMidnightSigner {
        fn wallet_seed(&self) -> &midnight_node_ledger_helpers::WalletSeed {
            &self.wallet_seed
        }
    }

    fn create_test_relayer() -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test-relayer-id".to_string(),
            name: "Test Relayer".to_string(),
            network: "testnet".to_string(),
            address: "test_midnight_address".to_string(),
            paused: false,
            system_disabled: false,
            signer_id: "test-signer-id".to_string(),
            notification_id: Some("test-notification-id".to_string()),
            policies: RelayerNetworkPolicy::Midnight(RelayerMidnightPolicy {
                min_balance: Some(100_000_000), // 0.1 tDUST
            }),
            network_type: NetworkType::Midnight,
            custom_rpc_urls: None,
            disabled_reason: None,
        }
    }

    fn create_test_transaction_request() -> NetworkTransactionRequest {
        NetworkTransactionRequest::Midnight(MidnightTransactionRequest {
            ttl: Some((Utc::now() + chrono::Duration::minutes(5)).to_rfc3339()),
            guaranteed_offer: Some(MidnightOfferRequest {
                inputs: vec![MidnightInputRequest {
                    origin: hex::encode([1u8; 32]),
                    token_type: hex::encode([2u8; 34]),
                    value: "100000000".to_string(), // 0.1 tDUST
                }],
                outputs: vec![MidnightOutputRequest {
                    destination: hex::encode([2u8; 32]),
                    token_type: hex::encode([2u8; 34]),
                    value: "50000000".to_string(), // 0.05 tDUST
                }],
            }),
            intents: vec![],
            fallible_offers: vec![],
        })
    }

    fn create_test_transaction(relayer_id: &str) -> TransactionRepoModel {
        TransactionRepoModel {
            id: "test-tx-id".to_string(),
            relayer_id: relayer_id.to_string(),
            status: TransactionStatus::Pending,
            status_reason: None,
            created_at: Utc::now().to_rfc3339(),
            sent_at: None,
            confirmed_at: None,
            valid_until: None,
            network_type: NetworkType::Midnight,
            network_data: NetworkTransactionData::Midnight(MidnightTransactionData {
                guaranteed_offer: Some(MidnightOfferRequest {
                    inputs: vec![MidnightInputRequest {
                        origin: hex::encode([1u8; 32]),
                        token_type: hex::encode([2u8; 34]),
                        value: "100000000".to_string(), // 0.1 tDUST
                    }],
                    outputs: vec![MidnightOutputRequest {
                        destination: hex::encode([2u8; 32]),
                        token_type: hex::encode([2u8; 34]),
                        value: "50000000".to_string(), // 0.05 tDUST
                    }],
                }),
                intents: vec![],
                fallible_offers: vec![],
                raw: None,
                signature: None,
                hash: None,
                pallet_hash: None,
                block_hash: None,
                segment_results: None,
            }),
            delete_at: None,
            priced_at: None,
            hashes: vec![],
            noop_count: None,
            is_canceled: Some(false),
        }
    }

    fn create_test_network() -> MidnightNetwork {
        MidnightNetwork {
            network: "testnet".to_string(),
            rpc_urls: vec!["https://rpc.testnet.midnight.org".to_string()],
            explorer_urls: None,
            average_blocktime_ms: 5000,
            is_testnet: true,
            tags: vec![],
            prover_url: "https://prover.testnet.midnight.org".to_string(),
            indexer_urls: IndexerUrls {
                http: "https://indexer.testnet.midnight.org".to_string(),
                ws: "wss://indexer.testnet.midnight.org".to_string(),
            },
        }
    }

    fn setup_midnight_test() {
        // Set required environment variable for Midnight tests
        unsafe {
            std::env::set_var(
                "MIDNIGHT_LEDGER_TEST_STATIC_DIR",
                "/tmp/midnight-test-static",
            );
        }
    }

    #[tokio::test]
    async fn test_enqueue_submit() {
        setup_midnight_test();

        let relayer = create_test_relayer();
        let test_tx = create_test_transaction(&relayer.id);
        let network = create_test_network();

        // Mock repositories
        let mock_relayer_repo = MockRepository::new();
        let mock_transaction_repo = MockTransactionRepository::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mock_provider = MockMidnightProviderTrait::new();
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let test_signer = TestMidnightSigner { wallet_seed };
        let mock_counter = MockTransactionCounterTrait::new();

        // Set up expectations
        mock_job_producer
            .expect_produce_submit_transaction_job()
            .withf(|job, delay| {
                job.transaction_id == "test-tx-id"
                    && job.relayer_id == "test-relayer-id"
                    && delay.is_none()
            })
            .returning(|_, _| Box::pin(async { Ok(()) }));

        // Create a minimal context for the sync manager - we won't use it in this test
        let wallet_seed = WalletSeed::Medium([1u8; 32]);

        // Create transaction handler without using sync manager in this test
        let midnight_transaction = MidnightTransaction::new(
            relayer.clone(),
            Arc::new(mock_provider),
            Arc::new(mock_relayer_repo),
            Arc::new(mock_transaction_repo),
            Arc::new(mock_job_producer),
            Arc::new(test_signer),
            Arc::new(mock_counter),
            Arc::new(tokio::sync::Mutex::new(
                SyncManager::new(
                    &MidnightIndexerClient::new(network.indexer_urls.clone()),
                    &wallet_seed,
                    NetworkId::TestNet,
                    Arc::new(RelayerStateRepositoryStorage::new_in_memory()),
                    relayer.id.clone(),
                )
                .await
                .unwrap(),
            )),
            network,
        )
        .unwrap();

        // Execute test
        let result = midnight_transaction.enqueue_submit(&test_tx).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_send_transaction_update_notification() {
        setup_midnight_test();

        let relayer = create_test_relayer();
        let test_tx = create_test_transaction(&relayer.id);
        let network = create_test_network();

        // Mock repositories
        let mock_relayer_repo = MockRepository::new();
        let mock_transaction_repo = MockTransactionRepository::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mock_provider = MockMidnightProviderTrait::new();
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let test_signer = TestMidnightSigner { wallet_seed };
        let mock_counter = MockTransactionCounterTrait::new();

        // Set up expectations
        mock_job_producer
            .expect_produce_send_notification_job()
            .withf(|payload, _| payload.notification_id == "test-notification-id")
            .returning(|_, _| Box::pin(async { Ok(()) }));

        // Create transaction handler
        let wallet_seed = WalletSeed::Medium([1u8; 32]);

        let midnight_transaction = MidnightTransaction::new(
            relayer.clone(),
            Arc::new(mock_provider),
            Arc::new(mock_relayer_repo),
            Arc::new(mock_transaction_repo),
            Arc::new(mock_job_producer),
            Arc::new(test_signer),
            Arc::new(mock_counter),
            Arc::new(tokio::sync::Mutex::new(
                SyncManager::new(
                    &MidnightIndexerClient::new(network.indexer_urls.clone()),
                    &wallet_seed,
                    NetworkId::TestNet,
                    Arc::new(RelayerStateRepositoryStorage::new_in_memory()),
                    relayer.id.clone(),
                )
                .await
                .unwrap(),
            )),
            network,
        )
        .unwrap();

        // Execute test
        let result = midnight_transaction
            .send_transaction_update_notification(&test_tx)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_schedule_status_check() {
        setup_midnight_test();

        let relayer = create_test_relayer();
        let test_tx = create_test_transaction(&relayer.id);
        let network = create_test_network();

        // Mock repositories
        let mock_relayer_repo = MockRepository::new();
        let mock_transaction_repo = MockTransactionRepository::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mock_provider = MockMidnightProviderTrait::new();
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let test_signer = TestMidnightSigner { wallet_seed };
        let mock_counter = MockTransactionCounterTrait::new();

        // Set up expectations
        mock_job_producer
            .expect_produce_check_transaction_status_job()
            .withf(|job, delay| {
                job.transaction_id == "test-tx-id"
                    && job.relayer_id == "test-relayer-id"
                    && delay.is_some()
            })
            .returning(|_, _| Box::pin(async { Ok(()) }));

        // Create transaction handler
        let wallet_seed = WalletSeed::Medium([1u8; 32]);

        let midnight_transaction = MidnightTransaction::new(
            relayer.clone(),
            Arc::new(mock_provider),
            Arc::new(mock_relayer_repo),
            Arc::new(mock_transaction_repo),
            Arc::new(mock_job_producer),
            Arc::new(test_signer),
            Arc::new(mock_counter),
            Arc::new(tokio::sync::Mutex::new(
                SyncManager::new(
                    &MidnightIndexerClient::new(network.indexer_urls.clone()),
                    &wallet_seed,
                    NetworkId::TestNet,
                    Arc::new(RelayerStateRepositoryStorage::new_in_memory()),
                    relayer.id.clone(),
                )
                .await
                .unwrap(),
            )),
            network,
        )
        .unwrap();

        // Execute test
        let result = midnight_transaction
            .schedule_status_check(&test_tx, Some(10))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cancel_transaction_not_supported() {
        setup_midnight_test();

        let relayer = create_test_relayer();
        let test_tx = create_test_transaction(&relayer.id);
        let network = create_test_network();

        // Mock repositories
        let mock_relayer_repo = MockRepository::new();
        let mock_transaction_repo = MockTransactionRepository::new();
        let mock_job_producer = MockJobProducerTrait::new();
        let mock_provider = MockMidnightProviderTrait::new();
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let test_signer = TestMidnightSigner { wallet_seed };
        let mock_counter = MockTransactionCounterTrait::new();

        // Create transaction handler
        let wallet_seed = WalletSeed::Medium([1u8; 32]);

        let midnight_transaction = MidnightTransaction::new(
            relayer.clone(),
            Arc::new(mock_provider),
            Arc::new(mock_relayer_repo),
            Arc::new(mock_transaction_repo),
            Arc::new(mock_job_producer),
            Arc::new(test_signer),
            Arc::new(mock_counter),
            Arc::new(tokio::sync::Mutex::new(
                SyncManager::new(
                    &MidnightIndexerClient::new(network.indexer_urls.clone()),
                    &wallet_seed,
                    NetworkId::TestNet,
                    Arc::new(RelayerStateRepositoryStorage::new_in_memory()),
                    relayer.id.clone(),
                )
                .await
                .unwrap(),
            )),
            network,
        )
        .unwrap();

        // Execute test
        let result = midnight_transaction.cancel_transaction(test_tx).await;
        assert!(matches!(result, Err(TransactionError::NotSupported(_))));
    }

    #[tokio::test]
    async fn test_replace_transaction_not_supported() {
        setup_midnight_test();

        let relayer = create_test_relayer();
        let test_tx = create_test_transaction(&relayer.id);
        let new_request = create_test_transaction_request();
        let network = create_test_network();

        // Mock repositories
        let mock_relayer_repo = MockRepository::new();
        let mock_transaction_repo = MockTransactionRepository::new();
        let mock_job_producer = MockJobProducerTrait::new();
        let mock_provider = MockMidnightProviderTrait::new();
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let test_signer = TestMidnightSigner { wallet_seed };
        let mock_counter = MockTransactionCounterTrait::new();

        // Create transaction handler
        let wallet_seed = WalletSeed::Medium([1u8; 32]);

        let midnight_transaction = MidnightTransaction::new(
            relayer.clone(),
            Arc::new(mock_provider),
            Arc::new(mock_relayer_repo),
            Arc::new(mock_transaction_repo),
            Arc::new(mock_job_producer),
            Arc::new(test_signer),
            Arc::new(mock_counter),
            Arc::new(tokio::sync::Mutex::new(
                SyncManager::new(
                    &MidnightIndexerClient::new(network.indexer_urls.clone()),
                    &wallet_seed,
                    NetworkId::TestNet,
                    Arc::new(RelayerStateRepositoryStorage::new_in_memory()),
                    relayer.id.clone(),
                )
                .await
                .unwrap(),
            )),
            network,
        )
        .unwrap();

        // Execute test
        let result = midnight_transaction
            .replace_transaction(test_tx, new_request)
            .await;
        assert!(matches!(result, Err(TransactionError::NotSupported(_))));
    }

    #[tokio::test]
    async fn test_prepare_transaction_insufficient_balance() {
        setup_midnight_test();

        let relayer = create_test_relayer();
        let test_tx = create_test_transaction(&relayer.id);
        let network = create_test_network();

        // Mock repositories
        let mock_relayer_repo = MockRepository::new();
        let mock_transaction_repo = MockTransactionRepository::new();
        let mock_job_producer = MockJobProducerTrait::new();
        let mut mock_provider = MockMidnightProviderTrait::new();
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let test_signer = TestMidnightSigner { wallet_seed };
        let mock_counter = MockTransactionCounterTrait::new();

        // Wallet seed is already set in test_signer

        // Mock provider with insufficient balance
        mock_provider
            .expect_get_balance()
            .returning(|_, _| Box::pin(async { Ok(U256::from(1000u128)) })); // 0.001 tDUST (insufficient)

        // Create transaction handler
        let midnight_transaction = MidnightTransaction::new(
            relayer.clone(),
            Arc::new(mock_provider),
            Arc::new(mock_relayer_repo),
            Arc::new(mock_transaction_repo),
            Arc::new(mock_job_producer),
            Arc::new(test_signer),
            Arc::new(mock_counter),
            Arc::new(tokio::sync::Mutex::new(
                SyncManager::new(
                    &MidnightIndexerClient::new(network.indexer_urls.clone()),
                    &wallet_seed,
                    NetworkId::TestNet,
                    Arc::new(RelayerStateRepositoryStorage::new_in_memory()),
                    relayer.id.clone(),
                )
                .await
                .unwrap(),
            )),
            network,
        )
        .unwrap();

        // Execute test - should fail during offer conversion due to insufficient balance
        let result = midnight_transaction.prepare_transaction(test_tx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_submit_transaction_missing_raw_data() {
        setup_midnight_test();

        let relayer = create_test_relayer();
        let test_tx = create_test_transaction(&relayer.id);
        let network = create_test_network();

        // Mock repositories
        let mock_relayer_repo = MockRepository::new();
        let mock_transaction_repo = MockTransactionRepository::new();
        let mock_job_producer = MockJobProducerTrait::new();
        let mock_provider = MockMidnightProviderTrait::new();
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let test_signer = TestMidnightSigner { wallet_seed };
        let mock_counter = MockTransactionCounterTrait::new();

        // Create transaction handler
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let midnight_transaction = MidnightTransaction::new(
            relayer.clone(),
            Arc::new(mock_provider),
            Arc::new(mock_relayer_repo),
            Arc::new(mock_transaction_repo),
            Arc::new(mock_job_producer),
            Arc::new(test_signer),
            Arc::new(mock_counter),
            Arc::new(tokio::sync::Mutex::new(
                SyncManager::new(
                    &MidnightIndexerClient::new(network.indexer_urls.clone()),
                    &wallet_seed,
                    midnight_node_ledger_helpers::NetworkId::TestNet,
                    Arc::new(RelayerStateRepositoryStorage::new_in_memory()),
                    relayer.id.clone(),
                )
                .await
                .unwrap(),
            )),
            network,
        )
        .unwrap();

        // Execute test - should fail because raw data is missing
        let result = midnight_transaction.submit_transaction(test_tx).await;
        assert!(result.is_err());
        assert!(matches!(result, Err(TransactionError::UnexpectedError(_))));
    }

    #[tokio::test]
    async fn test_handle_transaction_status_success_entirely() {
        setup_midnight_test();

        let relayer = create_test_relayer();
        let mut test_tx = create_test_transaction(&relayer.id);
        test_tx.status = TransactionStatus::Submitted;
        let network = create_test_network();

        // Add pallet hash
        if let NetworkTransactionData::Midnight(ref mut data) = test_tx.network_data {
            data.pallet_hash = Some("0xdef456".to_string());
        }

        // Mock repositories
        let mock_relayer_repo = MockRepository::new();
        let mut mock_transaction_repo = MockTransactionRepository::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mut mock_provider = MockMidnightProviderTrait::new();
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let test_signer = TestMidnightSigner { wallet_seed };
        let mock_counter = MockTransactionCounterTrait::new();

        // Mock provider get_transaction_by_hash
        mock_provider
            .expect_get_transaction_by_hash()
            .withf(|hash| hash == "0xdef456")
            .returning(|_| {
                let mut tx_data = serde_json::Map::new();
                tx_data.insert(
                    "applyStage".to_string(),
                    serde_json::to_value(ApplyStage::SucceedEntirely).unwrap(),
                );
                Box::pin(async move { Ok(Some(serde_json::Value::Object(tx_data))) })
            });

        // Mock transaction repository update
        mock_transaction_repo
            .expect_partial_update()
            .withf(|id, update| {
                id == "test-tx-id" && update.status == Some(TransactionStatus::Confirmed)
            })
            .returning(|id, _| {
                let mut tx = create_test_transaction("test-relayer-id");
                tx.id = id;
                tx.status = TransactionStatus::Confirmed;
                Ok(tx)
            });

        // Mock job producer
        mock_job_producer
            .expect_produce_send_notification_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        // Create transaction handler
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let midnight_transaction = MidnightTransaction::new(
            relayer.clone(),
            Arc::new(mock_provider),
            Arc::new(mock_relayer_repo),
            Arc::new(mock_transaction_repo),
            Arc::new(mock_job_producer),
            Arc::new(test_signer),
            Arc::new(mock_counter),
            Arc::new(tokio::sync::Mutex::new(
                SyncManager::new(
                    &MidnightIndexerClient::new(network.indexer_urls.clone()),
                    &wallet_seed,
                    midnight_node_ledger_helpers::NetworkId::TestNet,
                    Arc::new(RelayerStateRepositoryStorage::new_in_memory()),
                    relayer.id.clone(),
                )
                .await
                .unwrap(),
            )),
            network,
        )
        .unwrap();

        // Execute test
        let result = midnight_transaction
            .handle_transaction_status(test_tx)
            .await;
        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.status, TransactionStatus::Confirmed);
    }

    #[tokio::test]
    async fn test_handle_transaction_status_fail_entirely() {
        setup_midnight_test();

        let relayer = create_test_relayer();
        let mut test_tx = create_test_transaction(&relayer.id);
        test_tx.status = TransactionStatus::Submitted;
        let network = create_test_network();

        // Add pallet hash
        if let NetworkTransactionData::Midnight(ref mut data) = test_tx.network_data {
            data.pallet_hash = Some("0xdef456".to_string());
        }

        // Mock repositories
        let mock_relayer_repo = MockRepository::new();
        let mut mock_transaction_repo = MockTransactionRepository::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mut mock_provider = MockMidnightProviderTrait::new();
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let test_signer = TestMidnightSigner { wallet_seed };
        let mock_counter = MockTransactionCounterTrait::new();

        // Mock provider get_transaction_by_hash
        mock_provider
            .expect_get_transaction_by_hash()
            .withf(|hash| hash == "0xdef456")
            .returning(|_| {
                let mut tx_data = serde_json::Map::new();
                tx_data.insert(
                    "applyStage".to_string(),
                    serde_json::to_value(ApplyStage::FailEntirely).unwrap(),
                );
                Box::pin(async move { Ok(Some(serde_json::Value::Object(tx_data))) })
            });

        // Mock transaction repository update
        mock_transaction_repo
            .expect_partial_update()
            .withf(|id, update| {
                id == "test-tx-id" && update.status == Some(TransactionStatus::Failed)
            })
            .returning(|id, _| {
                let mut tx = create_test_transaction("test-relayer-id");
                tx.id = id;
                tx.status = TransactionStatus::Failed;
                Ok(tx)
            });

        // Mock job producer
        mock_job_producer
            .expect_produce_send_notification_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        // Create transaction handler
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let midnight_transaction = MidnightTransaction::new(
            relayer.clone(),
            Arc::new(mock_provider),
            Arc::new(mock_relayer_repo),
            Arc::new(mock_transaction_repo),
            Arc::new(mock_job_producer),
            Arc::new(test_signer),
            Arc::new(mock_counter),
            Arc::new(tokio::sync::Mutex::new(
                SyncManager::new(
                    &MidnightIndexerClient::new(network.indexer_urls.clone()),
                    &wallet_seed,
                    midnight_node_ledger_helpers::NetworkId::TestNet,
                    Arc::new(RelayerStateRepositoryStorage::new_in_memory()),
                    relayer.id.clone(),
                )
                .await
                .unwrap(),
            )),
            network,
        )
        .unwrap();

        // Execute test
        let result = midnight_transaction
            .handle_transaction_status(test_tx)
            .await;
        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.status, TransactionStatus::Failed);
    }

    #[tokio::test]
    async fn test_handle_transaction_status_pending() {
        setup_midnight_test();

        let relayer = create_test_relayer();
        let mut test_tx = create_test_transaction(&relayer.id);
        test_tx.status = TransactionStatus::Submitted;
        let network = create_test_network();

        // Add pallet hash
        if let NetworkTransactionData::Midnight(ref mut data) = test_tx.network_data {
            data.pallet_hash = Some("0xdef456".to_string());
        }

        // Mock repositories
        let mock_relayer_repo = MockRepository::new();
        let mock_transaction_repo = MockTransactionRepository::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mut mock_provider = MockMidnightProviderTrait::new();
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let test_signer = TestMidnightSigner { wallet_seed };
        let mock_counter = MockTransactionCounterTrait::new();

        // Mock provider get_transaction_by_hash
        mock_provider
            .expect_get_transaction_by_hash()
            .withf(|hash| hash == "0xdef456")
            .returning(|_| {
                let mut tx_data = serde_json::Map::new();
                tx_data.insert(
                    "applyStage".to_string(),
                    serde_json::to_value(ApplyStage::Pending).unwrap(),
                );
                Box::pin(async move { Ok(Some(serde_json::Value::Object(tx_data))) })
            });

        // Mock job producer - expect schedule_status_check
        mock_job_producer
            .expect_produce_check_transaction_status_job()
            .withf(|job, delay| job.transaction_id == "test-tx-id" && delay.is_some())
            .returning(|_, _| Box::pin(async { Ok(()) }));

        // Create transaction handler
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let midnight_transaction = MidnightTransaction::new(
            relayer.clone(),
            Arc::new(mock_provider),
            Arc::new(mock_relayer_repo),
            Arc::new(mock_transaction_repo),
            Arc::new(mock_job_producer),
            Arc::new(test_signer),
            Arc::new(mock_counter),
            Arc::new(tokio::sync::Mutex::new(
                SyncManager::new(
                    &MidnightIndexerClient::new(network.indexer_urls.clone()),
                    &wallet_seed,
                    midnight_node_ledger_helpers::NetworkId::TestNet,
                    Arc::new(RelayerStateRepositoryStorage::new_in_memory()),
                    relayer.id.clone(),
                )
                .await
                .unwrap(),
            )),
            network,
        )
        .unwrap();

        // Execute test
        let result = midnight_transaction
            .handle_transaction_status(test_tx.clone())
            .await;
        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.status, TransactionStatus::Submitted); // Status unchanged
    }

    #[tokio::test]
    async fn test_handle_transaction_status_not_found() {
        setup_midnight_test();

        let relayer = create_test_relayer();
        let mut test_tx = create_test_transaction(&relayer.id);
        test_tx.status = TransactionStatus::Submitted;
        let network = create_test_network();

        // Add pallet hash
        if let NetworkTransactionData::Midnight(ref mut data) = test_tx.network_data {
            data.pallet_hash = Some("0xdef456".to_string());
        }

        // Mock repositories
        let mock_relayer_repo = MockRepository::new();
        let mock_transaction_repo = MockTransactionRepository::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mut mock_provider = MockMidnightProviderTrait::new();
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let test_signer = TestMidnightSigner { wallet_seed };
        let mock_counter = MockTransactionCounterTrait::new();

        // Mock provider get_transaction_by_hash - transaction not found
        mock_provider
            .expect_get_transaction_by_hash()
            .withf(|hash| hash == "0xdef456")
            .returning(|_| Box::pin(async move { Ok(None) }));

        // Mock job producer - expect schedule_status_check
        mock_job_producer
            .expect_produce_check_transaction_status_job()
            .withf(|job, delay| job.transaction_id == "test-tx-id" && delay.is_some())
            .returning(|_, _| Box::pin(async { Ok(()) }));

        // Create transaction handler
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let midnight_transaction = MidnightTransaction::new(
            relayer.clone(),
            Arc::new(mock_provider),
            Arc::new(mock_relayer_repo),
            Arc::new(mock_transaction_repo),
            Arc::new(mock_job_producer),
            Arc::new(test_signer),
            Arc::new(mock_counter),
            Arc::new(tokio::sync::Mutex::new(
                SyncManager::new(
                    &MidnightIndexerClient::new(network.indexer_urls.clone()),
                    &wallet_seed,
                    midnight_node_ledger_helpers::NetworkId::TestNet,
                    Arc::new(RelayerStateRepositoryStorage::new_in_memory()),
                    relayer.id.clone(),
                )
                .await
                .unwrap(),
            )),
            network,
        )
        .unwrap();

        // Execute test
        let result = midnight_transaction
            .handle_transaction_status(test_tx.clone())
            .await;
        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.status, TransactionStatus::Submitted); // Status unchanged
    }

    #[tokio::test]
    async fn test_handle_transaction_status_final_state() {
        setup_midnight_test();

        let relayer = create_test_relayer();
        let mut test_tx = create_test_transaction(&relayer.id);
        test_tx.status = TransactionStatus::Confirmed; // Already in final state
        let network = create_test_network();

        // Mock repositories
        let mock_relayer_repo = MockRepository::new();
        let mock_transaction_repo = MockTransactionRepository::new();
        let mock_job_producer = MockJobProducerTrait::new();
        let mock_provider = MockMidnightProviderTrait::new();
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let test_signer = TestMidnightSigner { wallet_seed };
        let mock_counter = MockTransactionCounterTrait::new();

        // Create transaction handler
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let midnight_transaction = MidnightTransaction::new(
            relayer.clone(),
            Arc::new(mock_provider),
            Arc::new(mock_relayer_repo),
            Arc::new(mock_transaction_repo),
            Arc::new(mock_job_producer),
            Arc::new(test_signer),
            Arc::new(mock_counter),
            Arc::new(tokio::sync::Mutex::new(
                SyncManager::new(
                    &MidnightIndexerClient::new(network.indexer_urls.clone()),
                    &wallet_seed,
                    midnight_node_ledger_helpers::NetworkId::TestNet,
                    Arc::new(RelayerStateRepositoryStorage::new_in_memory()),
                    relayer.id.clone(),
                )
                .await
                .unwrap(),
            )),
            network,
        )
        .unwrap();

        // Execute test - should return immediately without checking status
        let result = midnight_transaction
            .handle_transaction_status(test_tx.clone())
            .await;
        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.status, TransactionStatus::Confirmed);
    }

    // This test requires a fixture with actual funded wallet (coins in the wallet state)
    // Uses funded wallet fixture from testnet
    #[tokio::test]
    async fn test_convert_offer_request_to_offer_info_success() {
        setup_midnight_test();

        let relayer = create_test_relayer();
        let network = create_test_network();

        // Use funded wallet seed for test
        let wallet_seed = WalletSeed::try_from_hex_str(
            "0e0cc7db98c60a39a6b0888795ba3f1bb1d61298cce264d4beca1529650e9041",
        )
        .unwrap();

        // Check if context fixture exists
        if get_context_fixture_path(&wallet_seed).is_none() {
            eprintln!(
                "Skipping test: No context fixture found for seed {}",
                hex::encode(wallet_seed.as_bytes())
            );
            eprintln!("To run this test, generate a fixture using:");
            eprintln!(
                "  WALLET_SEED={} cargo run --bin generate_midnight_fixtures",
                hex::encode(wallet_seed.as_bytes())
            );
            return;
        }

        // Create offer request (origin must match wallet seed)
        let offer_request = MidnightOfferRequest {
            inputs: vec![MidnightInputRequest {
                origin: hex::encode(wallet_seed.as_bytes()),
                token_type: hex::encode([2u8; 34]),
                value: "100000000".to_string(), // 0.1 tDUST
            }],
            outputs: vec![MidnightOutputRequest {
                destination: hex::encode([2u8; 32]),
                token_type: hex::encode([2u8; 34]),
                value: "30000000".to_string(), // 0.03 tDUST
            }],
        };

        // Mock repositories
        let mock_relayer_repo = MockRepository::new();
        let mock_transaction_repo = MockTransactionRepository::new();
        let mock_job_producer = MockJobProducerTrait::new();
        let mock_provider = MockMidnightProviderTrait::new();
        let test_signer = TestMidnightSigner { wallet_seed };
        let mock_counter = MockTransactionCounterTrait::new();

        // Create transaction handler with fixture-loaded sync manager
        let sync_manager =
            create_sync_manager_with_fixture(&wallet_seed, &network, relayer.id.clone()).await;
        let midnight_transaction = MidnightTransaction::new(
            relayer.clone(),
            Arc::new(mock_provider),
            Arc::new(mock_relayer_repo),
            Arc::new(mock_transaction_repo),
            Arc::new(mock_job_producer),
            Arc::new(test_signer),
            Arc::new(mock_counter),
            Arc::new(tokio::sync::Mutex::new(sync_manager)),
            network.clone(),
        )
        .unwrap();

        // Get the context
        let sync_manager = midnight_transaction.sync_manager();
        let sync_guard = sync_manager.lock().await;
        let context = sync_guard.get_context();
        drop(sync_guard);

        // Execute test
        let result = midnight_transaction
            .convert_offer_request_to_offer_info(&offer_request, wallet_seed, &context)
            .await;

        if result.is_err() {
            eprintln!("Test failed with error: {:?}", result.as_ref().err());
        }
        assert!(result.is_ok());
        let offer_info = result.unwrap();
        assert_eq!(offer_info.inputs.len(), 1);
        assert_eq!(offer_info.outputs.len(), 2); // Original output + change output
    }

    #[tokio::test]
    async fn test_convert_offer_request_to_offer_info_invalid_origin() {
        setup_midnight_test();

        let relayer = create_test_relayer();
        let network = create_test_network();

        // Create offer request with different origin
        let offer_request = MidnightOfferRequest {
            inputs: vec![MidnightInputRequest {
                origin: hex::encode([3u8; 32]), // Different from wallet seed
                token_type: hex::encode([2u8; 34]),
                value: "100000000".to_string(),
            }],
            outputs: vec![MidnightOutputRequest {
                destination: hex::encode([2u8; 32]),
                token_type: hex::encode([2u8; 34]),
                value: "30000000".to_string(),
            }],
        };

        // Mock repositories
        let mock_relayer_repo = MockRepository::new();
        let mock_transaction_repo = MockTransactionRepository::new();
        let mock_job_producer = MockJobProducerTrait::new();
        let mock_provider = MockMidnightProviderTrait::new();
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let test_signer = TestMidnightSigner { wallet_seed };
        let mock_counter = MockTransactionCounterTrait::new();

        // Create transaction handler
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let midnight_transaction = MidnightTransaction::new(
            relayer.clone(),
            Arc::new(mock_provider),
            Arc::new(mock_relayer_repo),
            Arc::new(mock_transaction_repo),
            Arc::new(mock_job_producer),
            Arc::new(test_signer),
            Arc::new(mock_counter),
            Arc::new(tokio::sync::Mutex::new(
                SyncManager::new(
                    &MidnightIndexerClient::new(network.indexer_urls.clone()),
                    &wallet_seed,
                    midnight_node_ledger_helpers::NetworkId::TestNet,
                    Arc::new(RelayerStateRepositoryStorage::new_in_memory()),
                    relayer.id.clone(),
                )
                .await
                .unwrap(),
            )),
            network.clone(),
        )
        .unwrap();

        // Get the context
        let sync_manager = midnight_transaction.sync_manager();
        let sync_guard = sync_manager.lock().await;
        let context = sync_guard.get_context();
        drop(sync_guard);

        // Execute test
        let result = midnight_transaction
            .convert_offer_request_to_offer_info(&offer_request, wallet_seed, &context)
            .await;

        assert!(result.is_err());
        assert!(matches!(result, Err(TransactionError::ValidationError(_))));
    }

    #[tokio::test]
    async fn test_convert_offer_request_empty_inputs() {
        setup_midnight_test();

        let relayer = create_test_relayer();
        let network = create_test_network();

        // Create offer request with no inputs
        let offer_request = MidnightOfferRequest {
            inputs: vec![],
            outputs: vec![MidnightOutputRequest {
                destination: hex::encode([2u8; 32]),
                token_type: hex::encode([2u8; 34]),
                value: "30000000".to_string(),
            }],
        };

        // Mock repositories
        let mock_relayer_repo = MockRepository::new();
        let mock_transaction_repo = MockTransactionRepository::new();
        let mock_job_producer = MockJobProducerTrait::new();
        let mock_provider = MockMidnightProviderTrait::new();
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let test_signer = TestMidnightSigner { wallet_seed };
        let mock_counter = MockTransactionCounterTrait::new();

        // Create transaction handler
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let midnight_transaction = MidnightTransaction::new(
            relayer.clone(),
            Arc::new(mock_provider),
            Arc::new(mock_relayer_repo),
            Arc::new(mock_transaction_repo),
            Arc::new(mock_job_producer),
            Arc::new(test_signer),
            Arc::new(mock_counter),
            Arc::new(tokio::sync::Mutex::new(
                SyncManager::new(
                    &MidnightIndexerClient::new(network.indexer_urls.clone()),
                    &wallet_seed,
                    NetworkId::TestNet,
                    Arc::new(RelayerStateRepositoryStorage::new_in_memory()),
                    relayer.id.clone(),
                )
                .await
                .unwrap(),
            )),
            network.clone(),
        )
        .unwrap();

        // Get the context
        let sync_manager = midnight_transaction.sync_manager();
        let sync_guard = sync_manager.lock().await;
        let context = sync_guard.get_context();
        drop(sync_guard);

        // Execute test
        let result = midnight_transaction
            .convert_offer_request_to_offer_info(&offer_request, wallet_seed, &context)
            .await;

        assert!(result.is_err());
        assert!(matches!(result, Err(TransactionError::ValidationError(_))));
    }

    #[tokio::test]
    async fn test_sign_transaction_success() {
        setup_midnight_test();

        let relayer = create_test_relayer();
        let test_tx = create_test_transaction(&relayer.id);
        let network = create_test_network();

        // Mock repositories
        let mock_relayer_repo = MockRepository::new();
        let mut mock_transaction_repo = MockTransactionRepository::new();
        let mock_job_producer = MockJobProducerTrait::new();
        let mock_provider = MockMidnightProviderTrait::new();
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let test_signer = TestMidnightSigner { wallet_seed };
        let mock_counter = MockTransactionCounterTrait::new();

        // No need to mock sign_transaction - TestMidnightSigner already returns test_signature

        // Mock transaction repository update
        mock_transaction_repo
            .expect_partial_update()
            .withf(|id, update| id == "test-tx-id" && update.network_data.is_some())
            .returning(|id, update| {
                let mut tx = create_test_transaction("test-relayer-id");
                tx.id = id;
                if let Some(NetworkTransactionData::Midnight(mut data)) = update.network_data {
                    data.signature = Some("test_signature".to_string());
                    tx.network_data = NetworkTransactionData::Midnight(data);
                }
                Ok(tx)
            });

        // Create transaction handler
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let midnight_transaction = MidnightTransaction::new(
            relayer.clone(),
            Arc::new(mock_provider),
            Arc::new(mock_relayer_repo),
            Arc::new(mock_transaction_repo),
            Arc::new(mock_job_producer),
            Arc::new(test_signer),
            Arc::new(mock_counter),
            Arc::new(tokio::sync::Mutex::new(
                SyncManager::new(
                    &MidnightIndexerClient::new(network.indexer_urls.clone()),
                    &wallet_seed,
                    NetworkId::TestNet,
                    Arc::new(RelayerStateRepositoryStorage::new_in_memory()),
                    relayer.id.clone(),
                )
                .await
                .unwrap(),
            )),
            network,
        )
        .unwrap();

        // Execute test
        let result = midnight_transaction.sign_transaction(test_tx).await;
        assert!(result.is_ok());
        let signed_tx = result.unwrap();
        if let NetworkTransactionData::Midnight(data) = signed_tx.network_data {
            assert_eq!(data.signature, Some("test_signature".to_string()));
        }
    }

    #[tokio::test]
    async fn test_validate_transaction() {
        setup_midnight_test();

        let relayer = create_test_relayer();
        let test_tx = create_test_transaction(&relayer.id);
        let network = create_test_network();

        // Mock repositories
        let mock_relayer_repo = MockRepository::new();
        let mock_transaction_repo = MockTransactionRepository::new();
        let mock_job_producer = MockJobProducerTrait::new();
        let mock_provider = MockMidnightProviderTrait::new();
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let test_signer = TestMidnightSigner { wallet_seed };
        let mock_counter = MockTransactionCounterTrait::new();

        // Create transaction handler
        let wallet_seed = WalletSeed::Medium([1u8; 32]);
        let midnight_transaction = MidnightTransaction::new(
            relayer.clone(),
            Arc::new(mock_provider),
            Arc::new(mock_relayer_repo),
            Arc::new(mock_transaction_repo),
            Arc::new(mock_job_producer),
            Arc::new(test_signer),
            Arc::new(mock_counter),
            Arc::new(tokio::sync::Mutex::new(
                SyncManager::new(
                    &MidnightIndexerClient::new(network.indexer_urls.clone()),
                    &wallet_seed,
                    NetworkId::TestNet,
                    Arc::new(RelayerStateRepositoryStorage::new_in_memory()),
                    relayer.id.clone(),
                )
                .await
                .unwrap(),
            )),
            network,
        )
        .unwrap();

        // Execute test - validate_transaction always returns true for Midnight
        let result = midnight_transaction.validate_transaction(test_tx).await;
        assert!(result.is_ok());
        assert!(result.unwrap());
    }
}
