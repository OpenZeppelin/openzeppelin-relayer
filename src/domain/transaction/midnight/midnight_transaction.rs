//! Midnight transaction implementation
//!
//! This module provides the core transaction handling logic for Midnight network transactions.

use crate::{
    // constants::MIDNIGHT_DECIMALS,
    domain::{to_midnight_network_id, MidnightTransactionBuilder, Transaction},
    jobs::{JobProducer, JobProducerTrait},
    models::{MidnightNetwork, RelayerRepoModel, TransactionError, TransactionRepoModel},
    repositories::{
        InMemoryRelayerRepository, InMemoryTransactionCounter, InMemoryTransactionRepository,
        RelayerRepositoryStorage, Repository, TransactionCounterTrait, TransactionRepository,
    },
    services::{
        midnight::handler::{QuickSyncStrategy, SyncManager},
        remote_prover::RemoteProofServer,
        MidnightProvider, MidnightProviderTrait, MidnightSigner, MidnightSignerTrait,
    },
};
use async_trait::async_trait;
use log::info;
use midnight_node_ledger_helpers::DefaultDB;
use rand::Rng;
use std::sync::Arc;

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
        // TODO: Implement transaction preparation
        // 1. Build intents and offers from request
        // 2. Request proofs from prover server
        // 3. Construct the full transaction
        // 4. Update transaction data with proof request IDs

        let wallet_seed = self.signer.wallet_seed();

        let indexer_client = self.provider.get_indexer_client();

        // TODO: We should probably move this up one level
        // This still requires `MIDNIGHT_LEDGER_TEST_STATIC_DIR` environment variable to be set (limitation by LedgerContext test resolver)
        // We should check with the Midnight team if we can use a different constructor for LedgerContext
        let mut sync_manager = SyncManager::<QuickSyncStrategy>::new(
            indexer_client,
            wallet_seed,
            to_midnight_network_id(&self.network.network),
        )
        .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        // Synchronises the LedgerContext (including ledger and wallet state) with the network
        // This is required to get the latest merkle tree state before submitting a transaction to ensure the transaction is valid locally AND on-chain
        // We use the QuickSyncStrategy by default which is a lightweight sync that uses the indexer to get the latest wallet-relevant states
        // The main difference between QuickSyncStrategy and FullSyncStrategy is that QuickSyncStrategy only syncs the wallet state and the ledger state, while FullSyncStrategy syncs the entire ledger state
        // This is useful because it's much faster and doesn't require downloading the entire ledger state from genesis
        // However, it requires trusting the indexer with your wallet viewing key (which is a public key).
        sync_manager
            .sync(0)
            .await
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        let context = sync_manager.get_context();

        let balance = self.provider.get_balance(wallet_seed, &context).await?;

        info!("Balance: {}", balance);

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

        // #[allow(clippy::identity_op)]
        // let amount = 1 * 10u128.pow(MIDNIGHT_DECIMALS); // 1 tDUST

        // // Calculate transaction fees first to validate total requirement
        // let estimated_fee =
        // 	midnight_node_ledger_helpers::Wallet::<MidnightDefaultDB>::calculate_fee(1, 1);
        // let total_required = amount + estimated_fee;

        // Calculate fee based on transaction structure
        // For now, assume 2 outputs (recipient + change) in most cases
        // Note: The Wallet::calculate_fee method seems to overestimate fees significantly
        // Based on the error, actual fee for 1 input, 2 outputs is ~60,855 dust
        // let actual_fee = 61000u128; // Conservative estimate for 1 input, 2 outputs (observed: 60,855)
        // let total_required = amount + actual_fee;

        // // Validate total amount including fees
        // if total_required > available_utxo_value {
        // 	return Err(TransactionError::InsufficientBalance(format!(
        // 		"Requested {} tDUST + {} tDUST fee = {} tDUST total, but only {} tDUST available",
        // 		format_token_amount(amount, transaction::MIDNIGHT_TOKEN_DECIMALS),
        // 		format_token_amount(actual_fee, transaction::MIDNIGHT_TOKEN_DECIMALS),
        // 		format_token_amount(total_required, transaction::MIDNIGHT_TOKEN_DECIMALS),
        // 		format_token_amount(available_utxo_value, transaction::MIDNIGHT_TOKEN_DECIMALS),
        // 	)));
        // }
        // info!(
        // 	"Amount validated successfully (including {} tDUST fee)",
        // 	format_token_amount(actual_fee, transaction::MIDNIGHT_TOKEN_DECIMALS)
        // );

        // // First, find the actual UTXO that will be spent to get its exact value
        // let from_wallet = context.wallet_from_seed(from_wallet_seed);

        // // Create a temporary input_info to find the minimum UTXO
        // let temp_input_info = InputInfo::<WalletSeed> {
        // 	origin: from_wallet_seed,
        // 	token_type,
        // 	value: amount + actual_fee, // Minimum amount needed including fees
        // };

        // // Find the actual UTXO that will be selected
        // let selected_coin = temp_input_info.min_match_coin(&from_wallet.state);
        // let actual_utxo_value = selected_coin.value;

        // debug!("Selected UTXO details:");
        // debug!(
        // 	"   - Value: {} tDUST",
        // 	format_token_amount(actual_utxo_value, transaction::MIDNIGHT_TOKEN_DECIMALS)
        // );
        // debug!("   - Type: {:?}", selected_coin.type_);
        // debug!("   - Nonce: {:?}", selected_coin.nonce);

        // // Debug: Check the mt_index
        // if let Some((_, qualified_coin_sp)) = from_wallet
        // 	.state
        // 	.coins
        // 	.iter()
        // 	.find(|(_, coin_sp)| coin_sp.nonce == selected_coin.nonce)
        // {
        // 	debug!("   - MT Index: {}", qualified_coin_sp.mt_index);
        // 	debug!(
        // 		"   - Wallet state first_free: {}",
        // 		from_wallet.state.first_free
        // 	);
        // }

        // // Verify the selected UTXO has sufficient value
        // if actual_utxo_value < amount + actual_fee {
        // 	error!(
        // 	"Selected UTXO value {} tDUST is insufficient for amount {} tDUST + fee {} tDUST = {} tDUST",
        // 	format_token_amount(actual_utxo_value, transaction::MIDNIGHT_TOKEN_DECIMALS),
        // 	format_token_amount(amount, transaction::MIDNIGHT_TOKEN_DECIMALS),
        // 	format_token_amount(actual_fee, transaction::MIDNIGHT_TOKEN_DECIMALS),
        // 	format_token_amount(amount + actual_fee, transaction::MIDNIGHT_TOKEN_DECIMALS)
        // );
        // 	return Err(TransactionError::InsufficientBalance(format!(
        // 	"Selected UTXO value {} tDUST is insufficient for transaction amount {} tDUST + fee {} tDUST = {} tDUST",
        // 	format_token_amount(actual_utxo_value, transaction::MIDNIGHT_TOKEN_DECIMALS),
        // 	format_token_amount(amount, transaction::MIDNIGHT_TOKEN_DECIMALS),
        // 	format_token_amount(actual_fee, transaction::MIDNIGHT_TOKEN_DECIMALS),
        // 	format_token_amount(amount + actual_fee, transaction::MIDNIGHT_TOKEN_DECIMALS)
        // )));
        // }

        // debug!(
        // 	"Found UTXO with value: {} tDUST (requested minimum: {} tDUST including {} tDUST fee)",
        // 	format_token_amount(actual_utxo_value, transaction::MIDNIGHT_TOKEN_DECIMALS),
        // 	format_token_amount(amount + actual_fee, transaction::MIDNIGHT_TOKEN_DECIMALS),
        // 	format_token_amount(actual_fee, transaction::MIDNIGHT_TOKEN_DECIMALS)
        // );

        // // Now create the actual input_info with the exact UTXO value that will be spent
        // // This ensures the zero-knowledge proof references the correct UTXO
        // let input_info = InputInfo::<WalletSeed> {
        // 	origin: from_wallet_seed,
        // 	token_type,
        // 	value: actual_utxo_value, // Use the exact value of the selected UTXO
        // };

        // debug!(
        // 	"Input info: {{origin: {:?}, token_type: {:?}, value: {} tDUST (actual UTXO value)}}",
        // 	hex::encode(from_wallet_seed.0),
        // 	token_type,
        // 	format_token_amount(actual_utxo_value, transaction::MIDNIGHT_TOKEN_DECIMALS),
        // );

        // // Create output to recipient
        // let recipient_output = OutputInfo::<WalletSeed> {
        // 	destination: to_wallet_seed,
        // 	token_type,
        // 	value: amount,
        // };

        // debug!(
        // 	"Recipient output: {{destination: {:?}, token_type: {:?}, value: {} tDUST}}",
        // 	hex::encode(to_wallet_seed.0),
        // 	token_type,
        // 	format_token_amount(amount, transaction::MIDNIGHT_TOKEN_DECIMALS)
        // );

        // // Create the guaranteed offer with input and outputs
        // let mut offer = OfferInfo::default();
        // offer.inputs.push(Box::new(input_info));
        // offer.outputs.push(Box::new(recipient_output));

        // // Calculate change and create change output if needed
        // // This ensures the indexer recognizes the transaction as relevant to the sender
        // let change_amount = actual_utxo_value.saturating_sub(amount + actual_fee);
        // if change_amount > 0 {
        // 	let change_output = OutputInfo::<WalletSeed> {
        // 		destination: from_wallet_seed, // Send change back to sender
        // 		token_type,
        // 		value: change_amount,
        // 	};
        // 	offer.outputs.push(Box::new(change_output));
        // 	debug!(
        // 		"Change output: {{destination: self, token_type: {:?}, value: {} tDUST}}",
        // 		token_type,
        // 		format_token_amount(change_amount, transaction::MIDNIGHT_TOKEN_DECIMALS)
        // 	);
        // } else {
        // 	debug!("No change output needed (exact amount)");
        // }

        let _transaction = MidnightTransactionBuilder::<DefaultDB>::new()
			.with_context(context)
			.with_proof_provider(proof_provider)
			.with_rng_seed(rng_seed)
			// .with_guaranteed_offer(offer)
			.build()
			.await?;

        log::debug!("Preparing Midnight transaction: {}", tx.id);

        // For now, just return the transaction as-is
        Ok(tx)
    }

    async fn submit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        // TODO: Implement transaction submission
        // 1. Check if proofs are ready
        // 2. Sign the transaction
        // 3. Submit to the network
        // 4. Handle partial success results

        log::debug!("Submitting Midnight transaction: {}", tx.id);

        // For now, just return the transaction as-is
        Ok(tx)
    }

    async fn resubmit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        // TODO: Implement transaction resubmission
        // This might involve resubmitting only failed segments

        log::debug!("Resubmitting Midnight transaction: {}", tx.id);

        // For now, just return the transaction as-is
        Ok(tx)
    }

    async fn handle_transaction_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        // TODO: Implement status handling
        // 1. Query transaction status from network
        // 2. Handle partial success (some segments succeeded, others failed)
        // 3. Update transaction status accordingly

        log::debug!("Handling Midnight transaction status: {}", tx.id);

        // For now, just return the transaction as-is
        Ok(tx)
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
