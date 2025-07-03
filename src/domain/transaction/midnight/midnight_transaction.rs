//! Midnight transaction implementation
//!
//! This module provides the core transaction handling logic for Midnight network transactions.

use crate::{
    domain::{to_midnight_network_id, MidnightTransactionBuilder, Transaction, DUST_TOKEN_TYPE},
    jobs::{JobProducer, JobProducerTrait, TransactionSend, TransactionStatusCheck},
    models::{
        produce_transaction_update_notification_payload, MidnightNetwork, MidnightOfferRequest,
        NetworkTransactionData, RelayerRepoModel, TransactionError, TransactionRepoModel,
        TransactionStatus, TransactionUpdateRequest,
    },
    repositories::{
        InMemoryRelayerRepository, InMemoryTransactionCounter, InMemoryTransactionRepository,
        RelayerRepositoryStorage, Repository, TransactionCounterTrait, TransactionRepository,
    },
    services::{
        midnight::handler::{QuickSyncStrategy, SyncManager},
        remote_prover::RemoteProofServer,
        sync::midnight::indexer::ApplyStage,
        MidnightProvider, MidnightProviderTrait, MidnightSigner, MidnightSignerTrait,
        TransactionSubmissionResult,
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
            let origin_seed = WalletSeed(origin_array);

            // Verify the origin matches the relayer's wallet
            if origin_seed != from_wallet_seed {
                return Err(TransactionError::ValidationError(
                    "All input origins must match relayer wallet".to_string(),
                ));
            }
        }

        // Calculate total output amount
        let mut total_output_amount = 0u128;
        let token_type = DUST_TOKEN_TYPE; // Only support DUST_TOKEN_TYPE

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
            let selected_coin = temp_input_info.min_match_coin(&from_wallet.state);
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
            let dest_seed = WalletSeed(dest_array);

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
                hex::encode(dest_seed.0)
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
                TransactionStatusCheck::new(tx.id.clone(), tx.relayer_id.clone()),
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
            .with_proof_provider(proof_provider)
            .with_rng_seed(rng_seed);

        // Convert and add guaranteed offer if present
        if let Some(offer_request) = &midnight_data.guaranteed_offer {
            let offer = self
                .convert_offer_request_to_offer_info(offer_request, *wallet_seed, &context)
                .await?;
            builder = builder.with_guaranteed_offer(offer);
        }

        // TODO: Handle intents and fallible offers when contract support is added
        if !midnight_data.intents.is_empty() || !midnight_data.fallible_offers.is_empty() {
            return Err(TransactionError::NotSupported(
                "Contract interactions not yet supported".to_string(),
            ));
        }

        // Build and prove the transaction
        let proven_transaction = builder.build().await?;

        // Serialize the transaction using Midnight's serialize function
        let serialized_tx = midnight_node_ledger_helpers::serialize(
            &proven_transaction,
            to_midnight_network_id(&self.network.network),
        )
        .map_err(|e| {
            TransactionError::UnexpectedError(format!("Failed to serialize transaction: {:?}", e))
        })?;

        // Update transaction with prepared data
        let mut updated_midnight_data = midnight_data.clone();
        updated_midnight_data.raw = Some(serialized_tx);
        // TODO: Store binding randomness and prover request ID when available

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
        let transaction = midnight_node_ledger_helpers::deserialize::<
            midnight_node_ledger_helpers::Transaction<
                midnight_node_ledger_helpers::Proof,
                DefaultDB,
            >,
            _,
        >(
            &serialized_tx[..],
            to_midnight_network_id(&self.network.network),
        )
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
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        // TODO: Implement transaction replacement
        // Note: Midnight transactions might not be replaceable

        log::debug!("Replacing Midnight transaction: {}", tx.id);

        Err(TransactionError::NotSupported(
            "Transaction replacement is not supported for Midnight".to_string(),
        ))
    }

    async fn sign_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        // TODO: Implement transaction signing
        // 1. Get the transaction data
        // 2. Sign with the signer
        // 3. Update transaction with signature

        log::debug!("Signing Midnight transaction: {}", tx.id);

        // For now, just return the transaction as-is
        Ok(tx)
    }

    async fn validate_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<bool, TransactionError> {
        // TODO: Implement transaction validation
        // 1. Validate transaction structure
        // 2. Check segment sequencing rules
        // 3. Validate proofs if available
        // NOTE: This is already handled by the transaction builder in the prepare_transaction method

        log::debug!("Validating Midnight transaction: {}", tx.id);

        // For now, just return true
        Ok(true)
    }
}

/// Default concrete type for Midnight transactions
pub type DefaultMidnightTransaction = MidnightTransaction<
    MidnightProvider,
    RelayerRepositoryStorage<InMemoryRelayerRepository>,
    InMemoryTransactionRepository,
    JobProducer,
    MidnightSigner,
    InMemoryTransactionCounter,
>;
