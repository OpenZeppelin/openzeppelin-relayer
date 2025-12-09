use crate::constants::get_stellar_sponsored_transaction_validity_duration;
use crate::domain::map_provider_error;
use crate::domain::relayer::evm::create_error_response;
use crate::services::stellar_dex::StellarDexService;
/// This module defines the `StellarRelayer` struct and its associated functionality for
/// interacting with Stellar networks. The `StellarRelayer` is responsible for managing
/// transactions, synchronizing sequence numbers, and ensuring the relayer's state is
/// consistent with the Stellar blockchain.
///
/// # Components
///
/// - `StellarRelayer`: The main struct that encapsulates the relayer's state and operations for Stellar.
/// - `RelayerRepoModel`: Represents the relayer's data model.
/// - `StellarProvider`: Provides blockchain interaction capabilities, such as fetching account details.
/// - `TransactionCounterService`: Manages the sequence number for transactions to ensure correct ordering.
/// - `JobProducer`: Produces jobs for processing transactions and sending notifications.
///
/// # Error Handling
///
/// The module uses the `RelayerError` enum to handle various errors that can occur during
/// operations, such as provider errors, sequence synchronization failures, and transaction failures.
///
/// # Usage
///
/// To use the `StellarRelayer`, create an instance using the `new` method, providing the necessary
/// components. Then, call the appropriate methods to process transactions and manage the relayer's state.
use crate::{
    constants::{STELLAR_SMALLEST_UNIT_NAME, STELLAR_STATUS_CHECK_INITIAL_DELAY_SECONDS},
    domain::{
        create_success_response, transaction::stellar::fetch_next_sequence_from_chain,
        BalanceResponse, SignDataRequest, SignDataResponse, SignTransactionExternalResponse,
        SignTransactionExternalResponseStellar, SignTransactionRequest, SignTypedDataRequest,
    },
    jobs::{JobProducerTrait, RelayerHealthCheck, TransactionRequest, TransactionStatusCheck},
    models::{
        produce_relayer_disabled_payload, DeletePendingTransactionsResponse, DisabledReason,
        HealthCheckFailure, JsonRpcRequest, JsonRpcResponse, NetworkRepoModel, NetworkRpcRequest,
        NetworkRpcResult, NetworkTransactionRequest, NetworkType, RelayerNetworkPolicy,
        RelayerRepoModel, RelayerStatus, RelayerStellarPolicy, RepositoryError, RpcErrorCodes,
        StellarAllowedTokensPolicy, StellarFeePaymentStrategy, StellarNetwork, StellarRpcRequest,
        TransactionRepoModel, TransactionStatus,
    },
    repositories::{NetworkRepository, RelayerRepository, Repository, TransactionRepository},
    services::{
        provider::{StellarProvider, StellarProviderTrait},
        signer::{StellarSignTrait, StellarSigner},
        stellar_dex::StellarDexServiceTrait,
        TransactionCounterService, TransactionCounterServiceTrait,
    },
    utils::calculate_scheduled_timestamp,
};
use async_trait::async_trait;
use eyre::Result;
use futures::future::try_join_all;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::domain::relayer::stellar::xdr_utils::parse_transaction_xdr;
use crate::domain::relayer::{Relayer, RelayerError, StellarRelayerDexTrait};
use crate::domain::transaction::stellar::token::get_token_metadata;
use crate::domain::transaction::stellar::StellarTransactionValidator;

/// Dependencies container for `StellarRelayer` construction.
pub struct StellarRelayerDependencies<RR, NR, TR, J, TCS>
where
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    TCS: TransactionCounterServiceTrait + Send + Sync + 'static,
{
    pub relayer_repository: Arc<RR>,
    pub network_repository: Arc<NR>,
    pub transaction_repository: Arc<TR>,
    pub transaction_counter_service: Arc<TCS>,
    pub job_producer: Arc<J>,
}

impl<RR, NR, TR, J, TCS> StellarRelayerDependencies<RR, NR, TR, J, TCS>
where
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync,
    TCS: TransactionCounterServiceTrait + Send + Sync + 'static,
{
    /// Creates a new dependencies container for `StellarRelayer`.
    ///
    /// # Arguments
    ///
    /// * `relayer_repository` - Repository for managing relayer model persistence
    /// * `network_repository` - Repository for accessing network configuration data (RPC URLs, chain settings)
    /// * `transaction_repository` - Repository for storing and retrieving transaction models
    /// * `transaction_counter_service` - Service for managing sequence numbers to ensure proper transaction ordering
    /// * `job_producer` - Service for creating background jobs for transaction processing and notifications
    ///
    /// # Returns
    ///
    /// Returns a new `StellarRelayerDependencies` instance containing all provided dependencies.
    pub fn new(
        relayer_repository: Arc<RR>,
        network_repository: Arc<NR>,
        transaction_repository: Arc<TR>,
        transaction_counter_service: Arc<TCS>,
        job_producer: Arc<J>,
    ) -> Self {
        Self {
            relayer_repository,
            network_repository,
            transaction_repository,
            transaction_counter_service,
            job_producer,
        }
    }
}

#[allow(dead_code)]
pub struct StellarRelayer<P, RR, NR, TR, J, TCS, S, D>
where
    P: StellarProviderTrait + Send + Sync + 'static,
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    TCS: TransactionCounterServiceTrait + Send + Sync + 'static,
    S: StellarSignTrait + Send + Sync + 'static,
    D: StellarDexServiceTrait + Send + Sync + 'static,
{
    pub(crate) relayer: RelayerRepoModel,
    signer: Arc<S>,
    pub(crate) network: StellarNetwork,
    pub(crate) provider: P,
    pub(crate) relayer_repository: Arc<RR>,
    network_repository: Arc<NR>,
    transaction_repository: Arc<TR>,
    transaction_counter_service: Arc<TCS>,
    pub(crate) job_producer: Arc<J>,
    pub(crate) dex_service: Arc<D>,
}

pub type DefaultStellarRelayer<J, TR, NR, RR, TCR> = StellarRelayer<
    StellarProvider,
    RR,
    NR,
    TR,
    J,
    TransactionCounterService<TCR>,
    StellarSigner,
    StellarDexService<StellarProvider, StellarSigner>,
>;

impl<P, RR, NR, TR, J, TCS, S, D> StellarRelayer<P, RR, NR, TR, J, TCS, S, D>
where
    P: StellarProviderTrait + Send + Sync,
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    TCS: TransactionCounterServiceTrait + Send + Sync + 'static,
    S: StellarSignTrait + Send + Sync + 'static,
    D: StellarDexServiceTrait + Send + Sync + 'static,
{
    /// Creates a new `StellarRelayer` instance.
    ///
    /// This constructor initializes a new Stellar relayer with the provided configuration,
    /// provider, and dependencies. It validates the network configuration and sets up
    /// all necessary components for transaction processing.
    ///
    /// # Arguments
    ///
    /// * `relayer` - The relayer model containing configuration like ID, address, network name, and policies
    /// * `signer` - The Stellar signer for signing transactions
    /// * `provider` - The Stellar provider implementation for blockchain interactions (account queries, transaction submission)
    /// * `dependencies` - Container with all required repositories and services (see [`StellarRelayerDependencies`])
    /// * `dex_service` - The DEX service implementation for swap operations
    ///
    /// # Returns
    ///
    /// * `Ok(StellarRelayer)` - Successfully initialized relayer ready for operation
    /// * `Err(RelayerError)` - If initialization fails due to configuration or validation errors
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        relayer: RelayerRepoModel,
        signer: Arc<S>,
        provider: P,
        dependencies: StellarRelayerDependencies<RR, NR, TR, J, TCS>,
        dex_service: Arc<D>,
    ) -> Result<Self, RelayerError> {
        let network_repo = dependencies
            .network_repository
            .get_by_name(NetworkType::Stellar, &relayer.network)
            .await
            .ok()
            .flatten()
            .ok_or_else(|| {
                RelayerError::NetworkConfiguration(format!("Network {} not found", relayer.network))
            })?;

        let network = StellarNetwork::try_from(network_repo.clone())?;

        Ok(Self {
            relayer,
            signer,
            network,
            provider,
            relayer_repository: dependencies.relayer_repository,
            network_repository: dependencies.network_repository,
            transaction_repository: dependencies.transaction_repository,
            transaction_counter_service: dependencies.transaction_counter_service,
            job_producer: dependencies.job_producer,
            dex_service,
        })
    }

    async fn sync_sequence(&self) -> Result<(), RelayerError> {
        info!(
            "Syncing sequence for relayer: {} ({})",
            self.relayer.id, self.relayer.address
        );

        let next = fetch_next_sequence_from_chain(&self.provider, &self.relayer.address)
            .await
            .map_err(RelayerError::ProviderError)?;

        info!(
            "Setting next sequence {} for relayer {}",
            next, self.relayer.id
        );
        self.transaction_counter_service
            .set(next)
            .await
            .map_err(RelayerError::from)?;
        Ok(())
    }

    /// Populates the allowed tokens metadata for the Stellar relayer policy.
    ///
    /// This method checks whether allowed tokens have been configured in the relayer's policy.
    /// If allowed tokens are provided, it concurrently fetches token metadata for each token,
    /// determines the token kind (Native, Classic, or Contract), and populates metadata including
    /// decimals and canonical asset ID. The updated policy is then stored in the repository.
    ///
    /// If no allowed tokens are specified, it logs an informational message and returns the policy
    /// unchanged.
    async fn populate_allowed_tokens_metadata(&self) -> Result<RelayerStellarPolicy, RelayerError> {
        let mut policy = self.relayer.policies.get_stellar_policy();
        // Check if allowed_tokens is specified; if not, return the policy unchanged.
        let allowed_tokens = match policy.allowed_tokens.as_ref() {
            Some(tokens) if !tokens.is_empty() => tokens,
            _ => {
                info!("No allowed tokens specified; skipping token metadata population.");
                return Ok(policy);
            }
        };

        let token_metadata_futures = allowed_tokens.iter().map(|token| {
            let asset_id = token.asset.clone();
            let provider = &self.provider;
            async move {
                let metadata = get_token_metadata(provider, &asset_id)
                    .await
                    .map_err(RelayerError::from)?;

                Ok::<StellarAllowedTokensPolicy, RelayerError>(StellarAllowedTokensPolicy {
                    asset: asset_id,
                    metadata: Some(metadata),
                    max_allowed_fee: token.max_allowed_fee,
                    swap_config: token.swap_config.clone(),
                })
            }
        });

        let updated_allowed_tokens = try_join_all(token_metadata_futures).await?;

        policy.allowed_tokens = Some(updated_allowed_tokens.clone());

        self.relayer_repository
            .update_policy(
                self.relayer.id.clone(),
                RelayerNetworkPolicy::Stellar(policy.clone()),
            )
            .await?;

        Ok(policy)
    }

    /// Migrates fee_payment_strategy policy for older relayers that don't have it set.
    ///
    /// This migration is needed for relayers that were created before `fee_payment_strategy`
    /// became a required policy. For relayers persisted in Redis storage, this ensures
    /// backward compatibility by setting the policy to `Relayer` (the old default behavior).
    ///
    /// In-memory relayers don't need this migration as they are recreated from config.json
    /// on startup, which would have the policy set if using a newer version.
    async fn migrate_fee_payment_strategy_if_needed(&self) -> Result<(), RelayerError> {
        // Only migrate if using persistent storage (Redis)
        // In-memory relayers are recreated from config.json on startup
        if !self.relayer_repository.is_persistent_storage() {
            debug!(
                relayer_id = %self.relayer.id,
                "Skipping migration: using in-memory storage"
            );
            return Ok(());
        }

        let policy = self.relayer.policies.get_stellar_policy();

        // If fee_payment_strategy is already set, no migration needed
        if policy.fee_payment_strategy.is_some() {
            return Ok(());
        }

        // Migration needed: fee_payment_strategy is missing
        info!(
            relayer_id = %self.relayer.id,
            "Migrating Stellar relayer: setting fee_payment_strategy to 'Relayer' (old default behavior)"
        );

        // Create updated policy with fee_payment_strategy set to Relayer
        let mut updated_policy = policy;
        updated_policy.fee_payment_strategy = Some(StellarFeePaymentStrategy::Relayer);

        // Update the relayer in the repository
        self.relayer_repository
            .update_policy(
                self.relayer.id.clone(),
                RelayerNetworkPolicy::Stellar(updated_policy),
            )
            .await
            .map_err(|e| {
                RelayerError::PolicyConfigurationError(format!(
                    "Failed to migrate fee_payment_strategy policy: {e}"
                ))
            })?;

        debug!(
            relayer_id = %self.relayer.id,
            "Successfully migrated fee_payment_strategy policy"
        );

        Ok(())
    }

    /// Checks the relayer's XLM balance and triggers token swap if it falls below the
    /// specified threshold. Only proceeds with swap if balance is below the configured
    /// min_balance_threshold.
    async fn check_balance_and_trigger_token_swap_if_needed(&self) -> Result<(), RelayerError> {
        let policy = self.relayer.policies.get_stellar_policy();

        // Check if swap config exists
        let swap_config = match policy.get_swap_config() {
            Some(config) => config,
            None => {
                debug!(
                    relayer_id = %self.relayer.id,
                    "No swap configuration specified; skipping balance check"
                );
                return Ok(());
            }
        };

        // Early return if no threshold is configured (mirrors Solana logic)
        let threshold = match swap_config.min_balance_threshold {
            Some(threshold) => threshold,
            None => {
                debug!(
                    relayer_id = %self.relayer.id,
                    "No swap min balance threshold specified; skipping validation"
                );
                return Ok(());
            }
        };

        // Get balance only when threshold is configured
        let balance_response = self.get_balance().await?;
        let current_balance = u64::try_from(balance_response.balance).map_err(|_| {
            RelayerError::Internal("Account balance exceeds u64 maximum value".to_string())
        })?;

        // Only trigger swap if balance is below threshold
        if current_balance < threshold {
            debug!(
                relayer_id = %self.relayer.id,
                balance = current_balance,
                threshold = threshold,
                "XLM balance is below threshold, triggering token swap"
            );

            let _swap_results = self
                .handle_token_swap_request(self.relayer.id.clone())
                .await?;
        } else {
            debug!(
                relayer_id = %self.relayer.id,
                balance = current_balance,
                threshold = threshold,
                "XLM balance is above threshold, no swap needed"
            );
        }

        Ok(())
    }
}

#[async_trait]
impl<P, RR, NR, TR, J, TCS, S, D> Relayer for StellarRelayer<P, RR, NR, TR, J, TCS, S, D>
where
    P: StellarProviderTrait + Send + Sync + 'static,
    D: StellarDexServiceTrait + Send + Sync + 'static,
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    TCS: TransactionCounterServiceTrait + Send + Sync + 'static,
    S: StellarSignTrait + Send + Sync + 'static,
{
    async fn process_transaction_request(
        &self,
        network_transaction: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError> {
        let network_model = self
            .network_repository
            .get_by_name(NetworkType::Stellar, &self.relayer.network)
            .await?
            .ok_or_else(|| {
                RelayerError::NetworkConfiguration(format!(
                    "Network {} not found",
                    self.relayer.network
                ))
            })?;
        let transaction =
            TransactionRepoModel::try_from((&network_transaction, &self.relayer, &network_model))?;

        self.transaction_repository
            .create(transaction.clone())
            .await
            .map_err(|e| RepositoryError::TransactionFailure(e.to_string()))?;

        self.job_producer
            .produce_transaction_request_job(
                TransactionRequest::new(transaction.id.clone(), transaction.relayer_id.clone()),
                None,
            )
            .await?;

        self.job_producer
            .produce_check_transaction_status_job(
                TransactionStatusCheck::new(
                    transaction.id.clone(),
                    transaction.relayer_id.clone(),
                    NetworkType::Stellar,
                ),
                Some(calculate_scheduled_timestamp(
                    STELLAR_STATUS_CHECK_INITIAL_DELAY_SECONDS,
                )),
            )
            .await?;

        Ok(transaction)
    }

    async fn get_balance(&self) -> Result<BalanceResponse, RelayerError> {
        let account_entry = self
            .provider
            .get_account(&self.relayer.address)
            .await
            .map_err(|e| {
                RelayerError::ProviderError(format!("Failed to fetch account for balance: {e}"))
            })?;

        Ok(BalanceResponse {
            balance: account_entry.balance as u128,
            unit: STELLAR_SMALLEST_UNIT_NAME.to_string(),
        })
    }

    async fn get_status(&self) -> Result<RelayerStatus, RelayerError> {
        let relayer_model = &self.relayer;

        let account_entry = self
            .provider
            .get_account(&relayer_model.address)
            .await
            .map_err(|e| {
                RelayerError::ProviderError(format!("Failed to get account details: {e}"))
            })?;

        let sequence_number_str = account_entry.seq_num.0.to_string();

        let balance_response = self.get_balance().await?;

        let pending_statuses = [TransactionStatus::Pending, TransactionStatus::Submitted];
        let pending_transactions = self
            .transaction_repository
            .find_by_status(&relayer_model.id, &pending_statuses[..])
            .await
            .map_err(RelayerError::from)?;
        let pending_transactions_count = pending_transactions.len() as u64;

        let confirmed_statuses = [TransactionStatus::Confirmed];
        let confirmed_transactions = self
            .transaction_repository
            .find_by_status(&relayer_model.id, &confirmed_statuses[..])
            .await
            .map_err(RelayerError::from)?;

        let last_confirmed_transaction_timestamp = confirmed_transactions
            .iter()
            .filter_map(|tx| tx.confirmed_at.as_ref())
            .max()
            .cloned();

        Ok(RelayerStatus::Stellar {
            balance: balance_response.balance.to_string(),
            pending_transactions_count,
            last_confirmed_transaction_timestamp,
            system_disabled: relayer_model.system_disabled,
            paused: relayer_model.paused,
            sequence_number: sequence_number_str,
        })
    }

    async fn delete_pending_transactions(
        &self,
    ) -> Result<DeletePendingTransactionsResponse, RelayerError> {
        println!("Stellar delete_pending_transactions...");
        Ok(DeletePendingTransactionsResponse {
            queued_for_cancellation_transaction_ids: vec![],
            failed_to_queue_transaction_ids: vec![],
            total_processed: 0,
        })
    }

    async fn sign_data(&self, _request: SignDataRequest) -> Result<SignDataResponse, RelayerError> {
        Err(RelayerError::NotSupported(
            "Signing data not supported for Stellar".to_string(),
        ))
    }

    async fn sign_typed_data(
        &self,
        _request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, RelayerError> {
        Err(RelayerError::NotSupported(
            "Signing typed data not supported for Stellar".to_string(),
        ))
    }

    async fn rpc(
        &self,
        request: JsonRpcRequest<NetworkRpcRequest>,
    ) -> Result<JsonRpcResponse<NetworkRpcResult>, RelayerError> {
        let JsonRpcRequest { id, params, .. } = request;
        let stellar_request = match params {
            NetworkRpcRequest::Stellar(stellar_req) => stellar_req,
            _ => {
                return Ok(create_error_response(
                    id.clone(),
                    RpcErrorCodes::INVALID_PARAMS,
                    "Invalid params",
                    "Expected Stellar network request",
                ))
            }
        };

        // Parse method and params from the Stellar request (single unified variant)
        let (method, params_json) = match stellar_request {
            StellarRpcRequest::RawRpcRequest { method, params } => (method, params),
        };

        match self
            .provider
            .raw_request_dyn(&method, params_json, id.clone())
            .await
        {
            Ok(result_value) => Ok(create_success_response(id.clone(), result_value)),
            Err(provider_error) => {
                let (error_code, error_message) = map_provider_error(&provider_error);
                Ok(create_error_response(
                    id.clone(),
                    error_code,
                    error_message,
                    &provider_error.to_string(),
                ))
            }
        }
    }

    async fn validate_min_balance(&self) -> Result<(), RelayerError> {
        Ok(())
    }

    async fn initialize_relayer(&self) -> Result<(), RelayerError> {
        debug!("initializing Stellar relayer {}", self.relayer.id);

        // Migration: Check if relayer needs fee_payment_strategy migration
        // Older relayers persisted in Redis may not have this policy set.
        // We automatically set it to "Relayer" (the old default behavior) for backward compatibility.
        self.migrate_fee_payment_strategy_if_needed().await?;

        // Populate model with allowed token metadata and update DB entry
        // Error will be thrown if any of the tokens are not found
        self.populate_allowed_tokens_metadata().await.map_err(|e| {
            RelayerError::PolicyConfigurationError(format!(
                "Error while processing allowed tokens policy: {e}"
            ))
        })?;

        match self.check_health().await {
            Ok(_) => {
                // All checks passed
                if self.relayer.system_disabled {
                    // Silently re-enable if was disabled (startup, not recovery)
                    self.relayer_repository
                        .enable_relayer(self.relayer.id.clone())
                        .await?;
                }
            }
            Err(failures) => {
                // Health checks failed
                let reason = DisabledReason::from_health_failures(failures).unwrap_or_else(|| {
                    DisabledReason::SequenceSyncFailed("Unknown error".to_string())
                });

                warn!(reason = %reason, "disabling relayer");
                let updated_relayer = self
                    .relayer_repository
                    .disable_relayer(self.relayer.id.clone(), reason.clone())
                    .await?;

                // Send notification if configured
                if let Some(notification_id) = &self.relayer.notification_id {
                    self.job_producer
                        .produce_send_notification_job(
                            produce_relayer_disabled_payload(
                                notification_id,
                                &updated_relayer,
                                &reason.safe_description(),
                            ),
                            None,
                        )
                        .await?;
                }

                // Schedule health check to try re-enabling the relayer after 10 seconds
                self.job_producer
                    .produce_relayer_health_check_job(
                        RelayerHealthCheck::new(self.relayer.id.clone()),
                        Some(calculate_scheduled_timestamp(10)),
                    )
                    .await?;
            }
        }
        debug!(
            "Stellar relayer initialized successfully: {}",
            self.relayer.id
        );
        Ok(())
    }

    async fn check_health(&self) -> Result<(), Vec<HealthCheckFailure>> {
        debug!(
            "running health checks for Stellar relayer {}",
            self.relayer.id
        );

        let mut failures = Vec::new();

        // Check sequence synchronization
        match self.sync_sequence().await {
            Ok(_) => {
                debug!(
                    "sequence sync passed for Stellar relayer {}",
                    self.relayer.id
                );
            }
            Err(e) => {
                let reason = HealthCheckFailure::SequenceSyncFailed(e.to_string());
                warn!("sequence sync failed: {:?}", reason);
                failures.push(reason);
            }
        }

        // Check balance and trigger token swap if fee_payment_strategy is User
        // Note: Swap failures are logged but don't cause health check failures
        // to avoid disabling the relayer due to transient swap issues
        let policy = self.relayer.policies.get_stellar_policy();
        if matches!(
            policy.fee_payment_strategy,
            Some(StellarFeePaymentStrategy::User)
        ) {
            debug!(
                "checking balance and attempting token swap for user fee payment strategy relayer {}",
                self.relayer.id
            );
            if let Err(e) = self.check_balance_and_trigger_token_swap_if_needed().await {
                warn!(
                    relayer_id = %self.relayer.id,
                    error = %e,
                    "Balance check or token swap failed, but not treating as health check failure"
                );
            } else {
                debug!(
                    "balance check and token swap completed for Stellar relayer {}",
                    self.relayer.id
                );
            }
        }

        if failures.is_empty() {
            debug!(
                "all health checks passed for Stellar relayer {}",
                self.relayer.id
            );
            Ok(())
        } else {
            warn!(
                "health checks failed for Stellar relayer {}: {:?}",
                self.relayer.id, failures
            );
            Err(failures)
        }
    }

    async fn sign_transaction(
        &self,
        request: &SignTransactionRequest,
    ) -> Result<SignTransactionExternalResponse, RelayerError> {
        let stellar_req = match request {
            SignTransactionRequest::Stellar(req) => req,
            _ => {
                return Err(RelayerError::NotSupported(
                    "Invalid request type for Stellar relayer".to_string(),
                ))
            }
        };

        let policy = self.relayer.policies.get_stellar_policy();
        let user_pays_fee = matches!(
            policy.fee_payment_strategy,
            Some(StellarFeePaymentStrategy::User)
        );

        // For user-paid fees, validate transaction before signing
        if user_pays_fee {
            // Parse the transaction XDR
            let envelope = parse_transaction_xdr(&stellar_req.unsigned_xdr, false)
                .map_err(|e| RelayerError::ValidationError(format!("Failed to parse XDR: {e}")))?;

            // Comprehensive validation for user fee payment transactions when signing
            // This validates: transaction structure, fee payments, allowed tokens, payment amounts, and time bounds
            StellarTransactionValidator::validate_user_fee_payment_transaction(
                &envelope,
                &self.relayer.address,
                &policy,
                &self.provider,
                self.dex_service.as_ref(),
                Some(get_stellar_sponsored_transaction_validity_duration()), // Enforce 1 minute max validity for signing flow
            )
            .await
            .map_err(|e| {
                RelayerError::ValidationError(format!("Failed to validate transaction: {e}"))
            })?;
        }

        // Use the signer's sign_xdr_transaction method
        let response = self
            .signer
            .sign_xdr_transaction(&stellar_req.unsigned_xdr, &self.network.passphrase)
            .await
            .map_err(RelayerError::SignerError)?;

        // Convert DecoratedSignature to base64 string
        let signature_bytes = &response.signature.signature.0;
        let signature_string =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, signature_bytes);

        Ok(SignTransactionExternalResponse::Stellar(
            SignTransactionExternalResponseStellar {
                signed_xdr: response.signed_xdr,
                signature: signature_string,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{NetworkConfigCommon, StellarNetworkConfig},
        constants::STELLAR_SMALLEST_UNIT_NAME,
        domain::{SignTransactionRequestStellar, SignXdrTransactionResponseStellar},
        jobs::MockJobProducerTrait,
        models::{
            NetworkConfigData, NetworkRepoModel, NetworkType, RelayerNetworkPolicy,
            RelayerRepoModel, RelayerStellarPolicy, SignerError,
        },
        repositories::{
            InMemoryNetworkRepository, MockRelayerRepository, MockTransactionRepository,
        },
        services::{
            provider::{MockStellarProviderTrait, ProviderError},
            signer::MockStellarSignTrait,
            stellar_dex::MockStellarDexServiceTrait,
            MockTransactionCounterServiceTrait,
        },
    };
    use mockall::predicate::*;
    use soroban_rs::xdr::{
        AccountEntry, AccountEntryExt, AccountId, DecoratedSignature, PublicKey, SequenceNumber,
        Signature, SignatureHint, String32, Thresholds, Uint256, VecM,
    };
    use std::future::ready;
    use std::sync::Arc;

    /// Helper function to create a mock DEX service for testing
    fn create_mock_dex_service() -> Arc<MockStellarDexServiceTrait> {
        let mut mock_dex = MockStellarDexServiceTrait::new();
        mock_dex.expect_supported_asset_types().returning(|| {
            use crate::services::stellar_dex::AssetType;
            std::collections::HashSet::from([AssetType::Native, AssetType::Classic])
        });
        Arc::new(mock_dex)
    }

    /// Test context structure to manage test dependencies
    struct TestCtx {
        relayer_model: RelayerRepoModel,
        network_repository: Arc<InMemoryNetworkRepository>,
    }

    impl Default for TestCtx {
        fn default() -> Self {
            let network_repository = Arc::new(InMemoryNetworkRepository::new());

            let relayer_model = RelayerRepoModel {
                id: "test-relayer-id".to_string(),
                name: "Test Relayer".to_string(),
                network: "testnet".to_string(),
                paused: false,
                network_type: NetworkType::Stellar,
                signer_id: "signer-id".to_string(),
                policies: RelayerNetworkPolicy::Stellar(RelayerStellarPolicy::default()),
                address: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
                notification_id: Some("notification-id".to_string()),
                system_disabled: false,
                custom_rpc_urls: None,
                ..Default::default()
            };

            TestCtx {
                relayer_model,
                network_repository,
            }
        }
    }

    impl TestCtx {
        async fn setup_network(&self) {
            let test_network = NetworkRepoModel {
                id: "stellar:testnet".to_string(),
                name: "testnet".to_string(),
                network_type: NetworkType::Stellar,
                config: NetworkConfigData::Stellar(StellarNetworkConfig {
                    common: NetworkConfigCommon {
                        network: "testnet".to_string(),
                        from: None,
                        rpc_urls: Some(vec!["https://horizon-testnet.stellar.org".to_string()]),
                        explorer_urls: None,
                        average_blocktime_ms: Some(5000),
                        is_testnet: Some(true),
                        tags: None,
                    },
                    passphrase: Some("Test SDF Network ; September 2015".to_string()),
                    horizon_url: Some("https://horizon-testnet.stellar.org".to_string()),
                }),
            };

            self.network_repository.create(test_network).await.unwrap();
        }
    }

    #[tokio::test]
    async fn test_sync_sequence_success() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let relayer_model = ctx.relayer_model.clone();
        let mut provider = MockStellarProviderTrait::new();
        provider
            .expect_get_account()
            .with(eq(relayer_model.address.clone()))
            .returning(|_| {
                Box::pin(async {
                    Ok(AccountEntry {
                        account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                        balance: 0,
                        ext: AccountEntryExt::V0,
                        flags: 0,
                        home_domain: String32::default(),
                        inflation_dest: None,
                        seq_num: SequenceNumber(5),
                        num_sub_entries: 0,
                        signers: VecM::default(),
                        thresholds: Thresholds([0, 0, 0, 0]),
                    })
                })
            });
        let mut counter = MockTransactionCounterServiceTrait::new();
        counter
            .expect_set()
            .with(eq(6u64))
            .returning(|_| Box::pin(async { Ok(()) }));
        let relayer_repo = MockRelayerRepository::new();
        let tx_repo = MockTransactionRepository::new();
        let job_producer = MockJobProducerTrait::new();
        let signer = Arc::new(MockStellarSignTrait::new());
        let dex_service = create_mock_dex_service();

        let relayer = StellarRelayer::new(
            relayer_model.clone(),
            signer,
            provider,
            StellarRelayerDependencies::new(
                Arc::new(relayer_repo),
                ctx.network_repository.clone(),
                Arc::new(tx_repo),
                Arc::new(counter),
                Arc::new(job_producer),
            ),
            dex_service,
        )
        .await
        .unwrap();

        let result = relayer.sync_sequence().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sync_sequence_provider_error() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let relayer_model = ctx.relayer_model.clone();
        let mut provider = MockStellarProviderTrait::new();
        provider
            .expect_get_account()
            .with(eq(relayer_model.address.clone()))
            .returning(|_| Box::pin(async { Err(ProviderError::Other("fail".to_string())) }));
        let counter = MockTransactionCounterServiceTrait::new();
        let relayer_repo = MockRelayerRepository::new();
        let tx_repo = MockTransactionRepository::new();
        let job_producer = MockJobProducerTrait::new();
        let signer = Arc::new(MockStellarSignTrait::new());
        let dex_service = create_mock_dex_service();

        let relayer = StellarRelayer::new(
            relayer_model.clone(),
            signer,
            provider,
            StellarRelayerDependencies::new(
                Arc::new(relayer_repo),
                ctx.network_repository.clone(),
                Arc::new(tx_repo),
                Arc::new(counter),
                Arc::new(job_producer),
            ),
            dex_service,
        )
        .await
        .unwrap();

        let result = relayer.sync_sequence().await;
        assert!(matches!(result, Err(RelayerError::ProviderError(_))));
    }

    #[tokio::test]
    async fn test_get_status_success_stellar() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let relayer_model = ctx.relayer_model.clone();
        let mut provider_mock = MockStellarProviderTrait::new();
        let mut tx_repo_mock = MockTransactionRepository::new();
        let relayer_repo_mock = MockRelayerRepository::new();
        let job_producer_mock = MockJobProducerTrait::new();
        let counter_mock = MockTransactionCounterServiceTrait::new();

        provider_mock.expect_get_account().times(2).returning(|_| {
            Box::pin(ready(Ok(AccountEntry {
                account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                balance: 10000000,
                seq_num: SequenceNumber(12345),
                ext: AccountEntryExt::V0,
                flags: 0,
                home_domain: String32::default(),
                inflation_dest: None,
                num_sub_entries: 0,
                signers: VecM::default(),
                thresholds: Thresholds([0, 0, 0, 0]),
            })))
        });

        tx_repo_mock
            .expect_find_by_status()
            .withf(|relayer_id, statuses| {
                relayer_id == "test-relayer-id"
                    && statuses == [TransactionStatus::Pending, TransactionStatus::Submitted]
            })
            .returning(|_, _| Ok(vec![]) as Result<Vec<TransactionRepoModel>, RepositoryError>)
            .once();

        let confirmed_tx = TransactionRepoModel {
            id: "tx1_stellar".to_string(),
            relayer_id: relayer_model.id.clone(),
            status: TransactionStatus::Confirmed,
            confirmed_at: Some("2023-02-01T12:00:00Z".to_string()),
            ..TransactionRepoModel::default()
        };
        tx_repo_mock
            .expect_find_by_status()
            .withf(|relayer_id, statuses| {
                relayer_id == "test-relayer-id" && statuses == [TransactionStatus::Confirmed]
            })
            .returning(move |_, _| {
                Ok(vec![confirmed_tx.clone()]) as Result<Vec<TransactionRepoModel>, RepositoryError>
            })
            .once();
        let signer = Arc::new(MockStellarSignTrait::new());
        let dex_service = create_mock_dex_service();

        let stellar_relayer = StellarRelayer::new(
            relayer_model.clone(),
            signer,
            provider_mock,
            StellarRelayerDependencies::new(
                Arc::new(relayer_repo_mock),
                ctx.network_repository.clone(),
                Arc::new(tx_repo_mock),
                Arc::new(counter_mock),
                Arc::new(job_producer_mock),
            ),
            dex_service,
        )
        .await
        .unwrap();

        let status = stellar_relayer.get_status().await.unwrap();

        match status {
            RelayerStatus::Stellar {
                balance,
                pending_transactions_count,
                last_confirmed_transaction_timestamp,
                system_disabled,
                paused,
                sequence_number,
            } => {
                assert_eq!(balance, "10000000");
                assert_eq!(pending_transactions_count, 0);
                assert_eq!(
                    last_confirmed_transaction_timestamp,
                    Some("2023-02-01T12:00:00Z".to_string())
                );
                assert_eq!(system_disabled, relayer_model.system_disabled);
                assert_eq!(paused, relayer_model.paused);
                assert_eq!(sequence_number, "12345");
            }
            _ => panic!("Expected Stellar RelayerStatus"),
        }
    }

    #[tokio::test]
    async fn test_get_status_stellar_provider_error() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let relayer_model = ctx.relayer_model.clone();
        let mut provider_mock = MockStellarProviderTrait::new();
        let tx_repo_mock = MockTransactionRepository::new();
        let relayer_repo_mock = MockRelayerRepository::new();
        let job_producer_mock = MockJobProducerTrait::new();
        let counter_mock = MockTransactionCounterServiceTrait::new();

        provider_mock
            .expect_get_account()
            .with(eq(relayer_model.address.clone()))
            .returning(|_| {
                Box::pin(async { Err(ProviderError::Other("Stellar provider down".to_string())) })
            });
        let signer = Arc::new(MockStellarSignTrait::new());
        let dex_service = create_mock_dex_service();

        let stellar_relayer = StellarRelayer::new(
            relayer_model.clone(),
            signer,
            provider_mock,
            StellarRelayerDependencies::new(
                Arc::new(relayer_repo_mock),
                ctx.network_repository.clone(),
                Arc::new(tx_repo_mock),
                Arc::new(counter_mock),
                Arc::new(job_producer_mock),
            ),
            dex_service,
        )
        .await
        .unwrap();

        let result = stellar_relayer.get_status().await;
        assert!(result.is_err());
        match result.err().unwrap() {
            RelayerError::ProviderError(msg) => {
                assert!(msg.contains("Failed to get account details"))
            }
            _ => panic!("Expected ProviderError for get_account failure"),
        }
    }

    #[tokio::test]
    async fn test_get_balance_success() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let relayer_model = ctx.relayer_model.clone();
        let mut provider = MockStellarProviderTrait::new();
        let expected_balance = 100_000_000i64; // 10 XLM in stroops

        provider
            .expect_get_account()
            .with(eq(relayer_model.address.clone()))
            .returning(move |_| {
                Box::pin(async move {
                    Ok(AccountEntry {
                        account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                        balance: expected_balance,
                        ext: AccountEntryExt::V0,
                        flags: 0,
                        home_domain: String32::default(),
                        inflation_dest: None,
                        seq_num: SequenceNumber(5),
                        num_sub_entries: 0,
                        signers: VecM::default(),
                        thresholds: Thresholds([0, 0, 0, 0]),
                    })
                })
            });

        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());
        let counter = Arc::new(MockTransactionCounterServiceTrait::new());
        let signer = Arc::new(MockStellarSignTrait::new());
        let dex_service = create_mock_dex_service();

        let relayer = StellarRelayer::new(
            relayer_model,
            signer,
            provider,
            StellarRelayerDependencies::new(
                relayer_repo,
                ctx.network_repository.clone(),
                tx_repo,
                counter,
                job_producer,
            ),
            dex_service,
        )
        .await
        .unwrap();

        let result = relayer.get_balance().await;
        assert!(result.is_ok());
        let balance_response = result.unwrap();
        assert_eq!(balance_response.balance, expected_balance as u128);
        assert_eq!(balance_response.unit, STELLAR_SMALLEST_UNIT_NAME);
    }

    #[tokio::test]
    async fn test_get_balance_provider_error() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let relayer_model = ctx.relayer_model.clone();
        let mut provider = MockStellarProviderTrait::new();

        provider
            .expect_get_account()
            .with(eq(relayer_model.address.clone()))
            .returning(|_| {
                Box::pin(async { Err(ProviderError::Other("provider failed".to_string())) })
            });

        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());
        let counter = Arc::new(MockTransactionCounterServiceTrait::new());
        let signer = Arc::new(MockStellarSignTrait::new());
        let dex_service = create_mock_dex_service();

        let relayer = StellarRelayer::new(
            relayer_model,
            signer,
            provider,
            StellarRelayerDependencies::new(
                relayer_repo,
                ctx.network_repository.clone(),
                tx_repo,
                counter,
                job_producer,
            ),
            dex_service,
        )
        .await
        .unwrap();

        let result = relayer.get_balance().await;
        assert!(result.is_err());
        match result.err().unwrap() {
            RelayerError::ProviderError(msg) => {
                assert!(msg.contains("Failed to fetch account for balance"));
            }
            _ => panic!("Unexpected error type"),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_success() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let relayer_model = ctx.relayer_model.clone();
        let provider = MockStellarProviderTrait::new();
        let mut signer = MockStellarSignTrait::new();

        let unsigned_xdr = "AAAAAgAAAAD///8AAAAAAAAAAQAAAAAAAAACAAAAAQAAAAAAAAAB";
        let expected_signed_xdr =
            "AAAAAgAAAAD///8AAAAAAAABAAAAAAAAAAIAAAABAAAAAAAAAAEAAAABAAAAA...";
        let expected_signature = DecoratedSignature {
            hint: SignatureHint([1, 2, 3, 4]),
            signature: Signature([5u8; 64].try_into().unwrap()),
        };
        let expected_signature_for_closure = expected_signature.clone();

        signer
            .expect_sign_xdr_transaction()
            .with(eq(unsigned_xdr), eq("Test SDF Network ; September 2015"))
            .returning(move |_, _| {
                Ok(SignXdrTransactionResponseStellar {
                    signed_xdr: expected_signed_xdr.to_string(),
                    signature: expected_signature_for_closure.clone(),
                })
            });

        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());
        let counter = Arc::new(MockTransactionCounterServiceTrait::new());
        let dex_service = create_mock_dex_service();

        let relayer = StellarRelayer::new(
            relayer_model,
            Arc::new(signer),
            provider,
            StellarRelayerDependencies::new(
                relayer_repo,
                ctx.network_repository.clone(),
                tx_repo,
                counter,
                job_producer,
            ),
            dex_service,
        )
        .await
        .unwrap();

        let request = SignTransactionRequest::Stellar(SignTransactionRequestStellar {
            unsigned_xdr: unsigned_xdr.to_string(),
        });
        let result = relayer.sign_transaction(&request).await;
        assert!(result.is_ok());

        match result.unwrap() {
            SignTransactionExternalResponse::Stellar(response) => {
                assert_eq!(response.signed_xdr, expected_signed_xdr);
                // Compare the base64 encoded signature
                let expected_signature_base64 = base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &expected_signature.signature.0,
                );
                assert_eq!(response.signature, expected_signature_base64);
            }
            _ => panic!("Expected Stellar response"),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_signer_error() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let relayer_model = ctx.relayer_model.clone();
        let provider = MockStellarProviderTrait::new();
        let mut signer = MockStellarSignTrait::new();

        let unsigned_xdr = "INVALID_XDR";

        signer
            .expect_sign_xdr_transaction()
            .with(eq(unsigned_xdr), eq("Test SDF Network ; September 2015"))
            .returning(|_, _| Err(SignerError::SigningError("Invalid XDR format".to_string())));

        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());
        let counter = Arc::new(MockTransactionCounterServiceTrait::new());
        let dex_service = create_mock_dex_service();

        let relayer = StellarRelayer::new(
            relayer_model,
            Arc::new(signer),
            provider,
            StellarRelayerDependencies::new(
                relayer_repo,
                ctx.network_repository.clone(),
                tx_repo,
                counter,
                job_producer,
            ),
            dex_service,
        )
        .await
        .unwrap();

        let request = SignTransactionRequest::Stellar(SignTransactionRequestStellar {
            unsigned_xdr: unsigned_xdr.to_string(),
        });
        let result = relayer.sign_transaction(&request).await;
        assert!(result.is_err());

        match result.err().unwrap() {
            RelayerError::SignerError(err) => match err {
                SignerError::SigningError(msg) => {
                    assert_eq!(msg, "Invalid XDR format");
                }
                _ => panic!("Expected SigningError"),
            },
            _ => panic!("Expected RelayerError::SignerError"),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_with_different_network_passphrase() {
        let ctx = TestCtx::default();
        // Create a custom network with a different passphrase
        let custom_network = NetworkRepoModel {
            id: "stellar:mainnet".to_string(),
            name: "mainnet".to_string(),
            network_type: NetworkType::Stellar,
            config: NetworkConfigData::Stellar(StellarNetworkConfig {
                common: NetworkConfigCommon {
                    network: "mainnet".to_string(),
                    from: None,
                    rpc_urls: Some(vec!["https://horizon.stellar.org".to_string()]),
                    explorer_urls: None,
                    average_blocktime_ms: Some(5000),
                    is_testnet: Some(false),
                    tags: None,
                },
                passphrase: Some("Public Global Stellar Network ; September 2015".to_string()),
                horizon_url: Some("https://horizon.stellar.org".to_string()),
            }),
        };
        ctx.network_repository.create(custom_network).await.unwrap();

        let mut relayer_model = ctx.relayer_model.clone();
        relayer_model.network = "mainnet".to_string();

        let provider = MockStellarProviderTrait::new();
        let mut signer = MockStellarSignTrait::new();

        let unsigned_xdr = "AAAAAgAAAAD///8AAAAAAAAAAQAAAAAAAAACAAAAAQAAAAAAAAAB";
        let expected_signature = DecoratedSignature {
            hint: SignatureHint([10, 20, 30, 40]),
            signature: Signature([15u8; 64].try_into().unwrap()),
        };
        let expected_signature_for_closure = expected_signature.clone();

        signer
            .expect_sign_xdr_transaction()
            .with(
                eq(unsigned_xdr),
                eq("Public Global Stellar Network ; September 2015"),
            )
            .returning(move |_, _| {
                Ok(SignXdrTransactionResponseStellar {
                    signed_xdr: "mainnet_signed_xdr".to_string(),
                    signature: expected_signature_for_closure.clone(),
                })
            });

        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());
        let counter = Arc::new(MockTransactionCounterServiceTrait::new());
        let dex_service = create_mock_dex_service();

        let relayer = StellarRelayer::new(
            relayer_model,
            Arc::new(signer),
            provider,
            StellarRelayerDependencies::new(
                relayer_repo,
                ctx.network_repository.clone(),
                tx_repo,
                counter,
                job_producer,
            ),
            dex_service,
        )
        .await
        .unwrap();

        let request = SignTransactionRequest::Stellar(SignTransactionRequestStellar {
            unsigned_xdr: unsigned_xdr.to_string(),
        });
        let result = relayer.sign_transaction(&request).await;
        assert!(result.is_ok());

        match result.unwrap() {
            SignTransactionExternalResponse::Stellar(response) => {
                assert_eq!(response.signed_xdr, "mainnet_signed_xdr");
                // Convert expected signature to base64 for comparison (just the signature bytes, not the whole struct)
                let expected_signature_string = base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &expected_signature.signature.0,
                );
                assert_eq!(response.signature, expected_signature_string);
            }
            _ => panic!("Expected Stellar response"),
        }
    }

    #[tokio::test]
    async fn test_initialize_relayer_disables_when_validation_fails() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let mut relayer_model = ctx.relayer_model.clone();
        relayer_model.system_disabled = false; // Start as enabled
        relayer_model.notification_id = Some("test-notification-id".to_string());

        let mut provider = MockStellarProviderTrait::new();
        let mut relayer_repo = MockRelayerRepository::new();
        let mut job_producer = MockJobProducerTrait::new();

        relayer_repo
            .expect_is_persistent_storage()
            .returning(|| false);

        // Mock validation failure - sequence sync fails
        provider
            .expect_get_account()
            .returning(|_| Box::pin(ready(Err(ProviderError::Other("RPC error".to_string())))));

        // Mock disable_relayer call
        let mut disabled_relayer = relayer_model.clone();
        disabled_relayer.system_disabled = true;
        relayer_repo
            .expect_disable_relayer()
            .withf(|id, reason| {
                id == "test-relayer-id"
                    && matches!(reason, crate::models::DisabledReason::SequenceSyncFailed(_))
            })
            .returning(move |_, _| Ok(disabled_relayer.clone()));

        // Mock notification job production
        job_producer
            .expect_produce_send_notification_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        // Mock health check job scheduling
        job_producer
            .expect_produce_relayer_health_check_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let tx_repo = MockTransactionRepository::new();
        let counter = MockTransactionCounterServiceTrait::new();
        let signer = Arc::new(MockStellarSignTrait::new());
        let dex_service = create_mock_dex_service();

        let relayer = StellarRelayer::new(
            relayer_model.clone(),
            signer,
            provider,
            StellarRelayerDependencies::new(
                Arc::new(relayer_repo),
                ctx.network_repository.clone(),
                Arc::new(tx_repo),
                Arc::new(counter),
                Arc::new(job_producer),
            ),
            dex_service,
        )
        .await
        .unwrap();

        let result = relayer.initialize_relayer().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_initialize_relayer_enables_when_validation_passes_and_was_disabled() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let mut relayer_model = ctx.relayer_model.clone();
        relayer_model.system_disabled = true; // Start as disabled

        let mut provider = MockStellarProviderTrait::new();
        let mut relayer_repo = MockRelayerRepository::new();

        relayer_repo
            .expect_is_persistent_storage()
            .returning(|| false);

        // Mock successful validations - sequence sync succeeds
        provider.expect_get_account().returning(|_| {
            Box::pin(ready(Ok(AccountEntry {
                account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                balance: 1000000000, // 100 XLM
                seq_num: SequenceNumber(1),
                num_sub_entries: 0,
                inflation_dest: None,
                flags: 0,
                home_domain: String32::default(),
                thresholds: Thresholds([0; 4]),
                signers: VecM::default(),
                ext: AccountEntryExt::V0,
            })))
        });

        // Mock enable_relayer call
        let mut enabled_relayer = relayer_model.clone();
        enabled_relayer.system_disabled = false;
        relayer_repo
            .expect_enable_relayer()
            .with(eq("test-relayer-id".to_string()))
            .returning(move |_| Ok(enabled_relayer.clone()));

        let tx_repo = MockTransactionRepository::new();
        let mut counter = MockTransactionCounterServiceTrait::new();
        counter
            .expect_set()
            .returning(|_| Box::pin(async { Ok(()) }));
        let signer = Arc::new(MockStellarSignTrait::new());
        let dex_service = create_mock_dex_service();
        let job_producer = MockJobProducerTrait::new();

        let relayer = StellarRelayer::new(
            relayer_model.clone(),
            signer,
            provider,
            StellarRelayerDependencies::new(
                Arc::new(relayer_repo),
                ctx.network_repository.clone(),
                Arc::new(tx_repo),
                Arc::new(counter),
                Arc::new(job_producer),
            ),
            dex_service,
        )
        .await
        .unwrap();

        let result = relayer.initialize_relayer().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_initialize_relayer_no_action_when_enabled_and_validation_passes() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let mut relayer_model = ctx.relayer_model.clone();
        relayer_model.system_disabled = false; // Start as enabled

        let mut provider = MockStellarProviderTrait::new();

        // Mock successful validations - sequence sync succeeds
        provider.expect_get_account().returning(|_| {
            Box::pin(ready(Ok(AccountEntry {
                account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                balance: 1000000000, // 100 XLM
                seq_num: SequenceNumber(1),
                num_sub_entries: 0,
                inflation_dest: None,
                flags: 0,
                home_domain: String32::default(),
                thresholds: Thresholds([0; 4]),
                signers: VecM::default(),
                ext: AccountEntryExt::V0,
            })))
        });

        // No repository calls should be made since relayer is already enabled

        let tx_repo = MockTransactionRepository::new();
        let mut counter = MockTransactionCounterServiceTrait::new();
        counter
            .expect_set()
            .returning(|_| Box::pin(async { Ok(()) }));
        let signer = Arc::new(MockStellarSignTrait::new());
        let dex_service = create_mock_dex_service();
        let job_producer = MockJobProducerTrait::new();
        let mut relayer_repo = MockRelayerRepository::new();

        relayer_repo
            .expect_is_persistent_storage()
            .returning(|| false);

        let relayer = StellarRelayer::new(
            relayer_model.clone(),
            signer,
            provider,
            StellarRelayerDependencies::new(
                Arc::new(relayer_repo),
                ctx.network_repository.clone(),
                Arc::new(tx_repo),
                Arc::new(counter),
                Arc::new(job_producer),
            ),
            dex_service,
        )
        .await
        .unwrap();

        let result = relayer.initialize_relayer().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_initialize_relayer_sends_notification_when_disabled() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let mut relayer_model = ctx.relayer_model.clone();
        relayer_model.system_disabled = false; // Start as enabled
        relayer_model.notification_id = Some("test-notification-id".to_string());

        let mut provider = MockStellarProviderTrait::new();
        let mut relayer_repo = MockRelayerRepository::new();
        let mut job_producer = MockJobProducerTrait::new();

        relayer_repo
            .expect_is_persistent_storage()
            .returning(|| false);

        // Mock validation failure - sequence sync fails
        provider.expect_get_account().returning(|_| {
            Box::pin(ready(Err(ProviderError::Other(
                "Sequence sync failed".to_string(),
            ))))
        });

        // Mock disable_relayer call
        let mut disabled_relayer = relayer_model.clone();
        disabled_relayer.system_disabled = true;
        relayer_repo
            .expect_disable_relayer()
            .withf(|id, reason| {
                id == "test-relayer-id"
                    && matches!(reason, crate::models::DisabledReason::SequenceSyncFailed(_))
            })
            .returning(move |_, _| Ok(disabled_relayer.clone()));

        // Mock notification job production - verify it's called
        job_producer
            .expect_produce_send_notification_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        // Mock health check job scheduling
        job_producer
            .expect_produce_relayer_health_check_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let tx_repo = MockTransactionRepository::new();
        let counter = MockTransactionCounterServiceTrait::new();
        let signer = Arc::new(MockStellarSignTrait::new());
        let dex_service = create_mock_dex_service();

        let relayer = StellarRelayer::new(
            relayer_model.clone(),
            signer,
            provider,
            StellarRelayerDependencies::new(
                Arc::new(relayer_repo),
                ctx.network_repository.clone(),
                Arc::new(tx_repo),
                Arc::new(counter),
                Arc::new(job_producer),
            ),
            dex_service,
        )
        .await
        .unwrap();

        let result = relayer.initialize_relayer().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_initialize_relayer_no_notification_when_no_notification_id() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let mut relayer_model = ctx.relayer_model.clone();
        relayer_model.system_disabled = false; // Start as enabled
        relayer_model.notification_id = None; // No notification ID

        let mut provider = MockStellarProviderTrait::new();
        let mut relayer_repo = MockRelayerRepository::new();
        relayer_repo
            .expect_is_persistent_storage()
            .returning(|| false);

        // Mock validation failure - sequence sync fails
        provider.expect_get_account().returning(|_| {
            Box::pin(ready(Err(ProviderError::Other(
                "Sequence sync failed".to_string(),
            ))))
        });

        // Mock disable_relayer call
        let mut disabled_relayer = relayer_model.clone();
        disabled_relayer.system_disabled = true;
        relayer_repo
            .expect_disable_relayer()
            .withf(|id, reason| {
                id == "test-relayer-id"
                    && matches!(reason, crate::models::DisabledReason::SequenceSyncFailed(_))
            })
            .returning(move |_, _| Ok(disabled_relayer.clone()));

        // No notification job should be produced since notification_id is None
        // But health check job should still be scheduled
        let mut job_producer = MockJobProducerTrait::new();
        job_producer
            .expect_produce_relayer_health_check_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let tx_repo = MockTransactionRepository::new();
        let counter = MockTransactionCounterServiceTrait::new();
        let signer = Arc::new(MockStellarSignTrait::new());
        let dex_service = create_mock_dex_service();

        let relayer = StellarRelayer::new(
            relayer_model.clone(),
            signer,
            provider,
            StellarRelayerDependencies::new(
                Arc::new(relayer_repo),
                ctx.network_repository.clone(),
                Arc::new(tx_repo),
                Arc::new(counter),
                Arc::new(job_producer),
            ),
            dex_service,
        )
        .await
        .unwrap();

        let result = relayer.initialize_relayer().await;
        assert!(result.is_ok());
    }

    mod process_transaction_request_tests {
        use super::*;
        use crate::constants::STELLAR_STATUS_CHECK_INITIAL_DELAY_SECONDS;
        use crate::models::{
            NetworkTransactionRequest, NetworkType, StellarTransactionRequest, TransactionStatus,
        };
        use chrono::Utc;

        // Helper function to create a valid test transaction request
        fn create_test_transaction_request() -> NetworkTransactionRequest {
            NetworkTransactionRequest::Stellar(StellarTransactionRequest {
                source_account: None,
                network: "testnet".to_string(),
                operations: None,
                memo: None,
                valid_until: None,
                transaction_xdr: Some("AAAAAgAAAACige4lTdwSB/sto4SniEdJ2kOa2X65s5bqkd40J4DjSwAAAAEAAHAkAAAADwAAAAAAAAAAAAAAAQAAAAAAAAABAAAAAKKB7iVN3BIH+y2jhKeIR0naQ5rZfrmzluqR3jQngONLAAAAAAAAAAAAD0JAAAAAAAAAAAA=".to_string()),
                fee_bump: None,
                max_fee: None,
            })
        }

        #[tokio::test]
        async fn test_process_transaction_request_calls_job_producer_methods() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let relayer_model = ctx.relayer_model.clone();

            let provider = MockStellarProviderTrait::new();
            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            // Create a test transaction request
            let tx_request = create_test_transaction_request();

            // Mock transaction repository - we expect it to create a transaction
            let mut tx_repo = MockTransactionRepository::new();
            tx_repo.expect_create().returning(|t| Ok(t.clone()));

            // Mock job producer to verify both methods are called
            let mut job_producer = MockJobProducerTrait::new();

            // Verify produce_transaction_request_job is called
            job_producer
                .expect_produce_transaction_request_job()
                .withf(|req, delay| {
                    !req.transaction_id.is_empty() && !req.relayer_id.is_empty() && delay.is_none()
                })
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Verify produce_check_transaction_status_job is called with correct parameters
            job_producer
                .expect_produce_check_transaction_status_job()
                .withf(|check, delay| {
                    !check.transaction_id.is_empty()
                        && !check.relayer_id.is_empty()
                        && check.network_type == Some(NetworkType::Stellar)
                        && delay.is_some()
                })
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let relayer_repo = Arc::new(MockRelayerRepository::new());
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model,
                signer,
                provider,
                StellarRelayerDependencies::new(
                    relayer_repo,
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer.process_transaction_request(tx_request).await;
            if let Err(e) = &result {
                panic!("process_transaction_request failed: {}", e);
            }
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_process_transaction_request_with_scheduled_delay() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let relayer_model = ctx.relayer_model.clone();

            let provider = MockStellarProviderTrait::new();
            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let tx_request = create_test_transaction_request();

            let mut tx_repo = MockTransactionRepository::new();
            tx_repo.expect_create().returning(|t| Ok(t.clone()));

            let mut job_producer = MockJobProducerTrait::new();

            job_producer
                .expect_produce_transaction_request_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Verify that the status check is scheduled with the initial delay
            job_producer
                .expect_produce_check_transaction_status_job()
                .withf(|_, delay| {
                    // Should have a delay timestamp
                    if let Some(scheduled_at) = delay {
                        // The scheduled time should be approximately STELLAR_STATUS_CHECK_INITIAL_DELAY_SECONDS from now
                        let now = Utc::now().timestamp();
                        let diff = scheduled_at - now;
                        // Allow some tolerance (within 2 seconds)
                        diff >= (STELLAR_STATUS_CHECK_INITIAL_DELAY_SECONDS - 2)
                            && diff <= (STELLAR_STATUS_CHECK_INITIAL_DELAY_SECONDS + 2)
                    } else {
                        false
                    }
                })
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let relayer_repo = Arc::new(MockRelayerRepository::new());
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model,
                signer,
                provider,
                StellarRelayerDependencies::new(
                    relayer_repo,
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer.process_transaction_request(tx_request).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_process_transaction_request_repository_failure() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let relayer_model = ctx.relayer_model.clone();

            let provider = MockStellarProviderTrait::new();
            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let tx_request = create_test_transaction_request();

            // Mock repository failure
            let mut tx_repo = MockTransactionRepository::new();
            tx_repo.expect_create().returning(|_| {
                Err(RepositoryError::TransactionFailure(
                    "Database connection failed".to_string(),
                ))
            });

            // Job producer should NOT be called when repository fails
            let job_producer = MockJobProducerTrait::new();

            let relayer_repo = Arc::new(MockRelayerRepository::new());
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model,
                signer,
                provider,
                StellarRelayerDependencies::new(
                    relayer_repo,
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer.process_transaction_request(tx_request).await;
            assert!(result.is_err());
            // RepositoryError is converted to RelayerError::NetworkConfiguration
            let err_msg = result.err().unwrap().to_string();
            assert!(
                err_msg.contains("Database connection failed"),
                "Error was: {}",
                err_msg
            );
        }

        #[tokio::test]
        async fn test_process_transaction_request_job_producer_request_failure() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let relayer_model = ctx.relayer_model.clone();

            let provider = MockStellarProviderTrait::new();
            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let tx_request = create_test_transaction_request();

            let mut tx_repo = MockTransactionRepository::new();
            tx_repo.expect_create().returning(|t| Ok(t.clone()));

            // Mock produce_transaction_request_job to fail
            let mut job_producer = MockJobProducerTrait::new();
            job_producer
                .expect_produce_transaction_request_job()
                .returning(|_, _| {
                    Box::pin(async {
                        Err(crate::jobs::JobProducerError::QueueError(
                            "Queue is full".to_string(),
                        ))
                    })
                });

            // Status check job should NOT be called if request job fails

            let relayer_repo = Arc::new(MockRelayerRepository::new());
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model,
                signer,
                provider,
                StellarRelayerDependencies::new(
                    relayer_repo,
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer.process_transaction_request(tx_request).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn test_process_transaction_request_job_producer_status_check_failure() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let relayer_model = ctx.relayer_model.clone();

            let provider = MockStellarProviderTrait::new();
            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let tx_request = create_test_transaction_request();

            let mut tx_repo = MockTransactionRepository::new();
            tx_repo.expect_create().returning(|t| Ok(t.clone()));

            let mut job_producer = MockJobProducerTrait::new();

            // Request job succeeds
            job_producer
                .expect_produce_transaction_request_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Status check job fails
            job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| {
                    Box::pin(async {
                        Err(crate::jobs::JobProducerError::QueueError(
                            "Failed to queue job".to_string(),
                        ))
                    })
                });

            let relayer_repo = Arc::new(MockRelayerRepository::new());
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model,
                signer,
                provider,
                StellarRelayerDependencies::new(
                    relayer_repo,
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer.process_transaction_request(tx_request).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn test_process_transaction_request_preserves_transaction_data() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let relayer_model = ctx.relayer_model.clone();

            let provider = MockStellarProviderTrait::new();
            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let tx_request = create_test_transaction_request();

            let mut tx_repo = MockTransactionRepository::new();
            tx_repo.expect_create().returning(|t| Ok(t.clone()));

            let mut job_producer = MockJobProducerTrait::new();
            job_producer
                .expect_produce_transaction_request_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let relayer_repo = Arc::new(MockRelayerRepository::new());
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model.clone(),
                signer,
                provider,
                StellarRelayerDependencies::new(
                    relayer_repo,
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer.process_transaction_request(tx_request).await;
            assert!(result.is_ok());

            let returned_tx = result.unwrap();
            assert_eq!(returned_tx.relayer_id, relayer_model.id);
            assert_eq!(returned_tx.network_type, NetworkType::Stellar);
            assert_eq!(returned_tx.status, TransactionStatus::Pending);
        }
    }

    // Tests for populate_allowed_tokens_metadata
    mod populate_allowed_tokens_metadata_tests {
        use super::*;
        use crate::models::StellarTokenKind;

        #[tokio::test]
        async fn test_populate_allowed_tokens_metadata_no_tokens() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let relayer_model = ctx.relayer_model.clone();

            let provider = MockStellarProviderTrait::new();
            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let mut relayer_repo = MockRelayerRepository::new();
            // Should not be called since no tokens
            relayer_repo.expect_update_policy().times(0);

            let tx_repo = MockTransactionRepository::new();
            let job_producer = MockJobProducerTrait::new();
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model.clone(),
                signer,
                provider,
                StellarRelayerDependencies::new(
                    Arc::new(relayer_repo),
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer.populate_allowed_tokens_metadata().await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_populate_allowed_tokens_metadata_empty_tokens() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let mut relayer_model = ctx.relayer_model.clone();

            // Set up empty allowed tokens
            let mut policy = RelayerStellarPolicy::default();
            policy.allowed_tokens = Some(vec![]);
            relayer_model.policies = RelayerNetworkPolicy::Stellar(policy);

            let provider = MockStellarProviderTrait::new();
            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let mut relayer_repo = MockRelayerRepository::new();
            // Should not be called since tokens list is empty
            relayer_repo.expect_update_policy().times(0);

            let tx_repo = MockTransactionRepository::new();
            let job_producer = MockJobProducerTrait::new();
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model.clone(),
                signer,
                provider,
                StellarRelayerDependencies::new(
                    Arc::new(relayer_repo),
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer.populate_allowed_tokens_metadata().await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_populate_allowed_tokens_metadata_classic_asset_success() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let mut relayer_model = ctx.relayer_model.clone();

            // Set up allowed tokens with a classic asset (USDC)
            let mut policy = RelayerStellarPolicy::default();
            policy.allowed_tokens = Some(vec![crate::models::StellarAllowedTokensPolicy {
                asset: "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5".to_string(),
                metadata: None,
                max_allowed_fee: None,
                swap_config: None,
            }]);
            relayer_model.policies = RelayerNetworkPolicy::Stellar(policy);

            let provider = MockStellarProviderTrait::new();
            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let mut relayer_repo = MockRelayerRepository::new();
            relayer_repo
                .expect_update_policy()
                .times(1)
                .returning(|_, _| Ok(RelayerRepoModel::default()));

            let tx_repo = MockTransactionRepository::new();
            let job_producer = MockJobProducerTrait::new();
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model.clone(),
                signer,
                provider,
                StellarRelayerDependencies::new(
                    Arc::new(relayer_repo),
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer.populate_allowed_tokens_metadata().await;
            assert!(result.is_ok());

            let updated_policy = result.unwrap();
            assert!(updated_policy.allowed_tokens.is_some());

            let tokens = updated_policy.allowed_tokens.unwrap();
            assert_eq!(tokens.len(), 1);

            // Verify metadata was populated
            let token = &tokens[0];
            assert!(token.metadata.is_some());

            let metadata = token.metadata.as_ref().unwrap();
            assert_eq!(metadata.decimals, 7); // Default Stellar decimals
            assert_eq!(
                metadata.canonical_asset_id,
                "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
            );

            // Verify it's a classic asset
            match &metadata.kind {
                StellarTokenKind::Classic { code, issuer } => {
                    assert_eq!(code, "USDC");
                    assert_eq!(
                        issuer,
                        "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
                    );
                }
                _ => panic!("Expected Classic token kind"),
            }
        }

        #[tokio::test]
        async fn test_populate_allowed_tokens_metadata_multiple_tokens() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let mut relayer_model = ctx.relayer_model.clone();

            // Set up multiple allowed tokens
            let mut policy = RelayerStellarPolicy::default();
            policy.allowed_tokens = Some(vec![
                crate::models::StellarAllowedTokensPolicy {
                    asset: "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
                        .to_string(),
                    metadata: None,
                    max_allowed_fee: None,
                    swap_config: None,
                },
                crate::models::StellarAllowedTokensPolicy {
                    asset: "AQUA:GAHPYWLK6YRN7CVYZOO4H3VDRZ7PVF5UJGLZCSPAEIKJE2XSWF5LAGER"
                        .to_string(),
                    metadata: None,
                    max_allowed_fee: Some(1000000),
                    swap_config: None,
                },
            ]);
            relayer_model.policies = RelayerNetworkPolicy::Stellar(policy);

            let provider = MockStellarProviderTrait::new();
            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let mut relayer_repo = MockRelayerRepository::new();
            relayer_repo
                .expect_update_policy()
                .times(1)
                .returning(|_, _| Ok(RelayerRepoModel::default()));

            let tx_repo = MockTransactionRepository::new();
            let job_producer = MockJobProducerTrait::new();
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model.clone(),
                signer,
                provider,
                StellarRelayerDependencies::new(
                    Arc::new(relayer_repo),
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer.populate_allowed_tokens_metadata().await;
            assert!(result.is_ok());

            let updated_policy = result.unwrap();
            let tokens = updated_policy.allowed_tokens.unwrap();
            assert_eq!(tokens.len(), 2);

            // Verify both tokens have metadata
            assert!(tokens[0].metadata.is_some());
            assert!(tokens[1].metadata.is_some());

            // Verify first token (USDC)
            let usdc_metadata = tokens[0].metadata.as_ref().unwrap();
            match &usdc_metadata.kind {
                StellarTokenKind::Classic { code, .. } => {
                    assert_eq!(code, "USDC");
                }
                _ => panic!("Expected Classic token kind for USDC"),
            }

            // Verify second token (AQUA)
            let aqua_metadata = tokens[1].metadata.as_ref().unwrap();
            match &aqua_metadata.kind {
                StellarTokenKind::Classic { code, .. } => {
                    assert_eq!(code, "AQUA");
                }
                _ => panic!("Expected Classic token kind for AQUA"),
            }

            // Verify max_allowed_fee is preserved
            assert_eq!(tokens[1].max_allowed_fee, Some(1000000));
        }

        #[tokio::test]
        async fn test_populate_allowed_tokens_metadata_invalid_asset() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let mut relayer_model = ctx.relayer_model.clone();

            // Set up allowed tokens with invalid asset format
            let mut policy = RelayerStellarPolicy::default();
            policy.allowed_tokens = Some(vec![crate::models::StellarAllowedTokensPolicy {
                asset: "INVALID_FORMAT".to_string(), // Missing issuer
                metadata: None,
                max_allowed_fee: None,
                swap_config: None,
            }]);
            relayer_model.policies = RelayerNetworkPolicy::Stellar(policy);

            let provider = MockStellarProviderTrait::new();
            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let relayer_repo = MockRelayerRepository::new();
            let tx_repo = MockTransactionRepository::new();
            let job_producer = MockJobProducerTrait::new();
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model.clone(),
                signer,
                provider,
                StellarRelayerDependencies::new(
                    Arc::new(relayer_repo),
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer.populate_allowed_tokens_metadata().await;
            assert!(result.is_err());
        }
    }

    // Tests for migrate_fee_payment_strategy_if_needed
    mod migrate_fee_payment_strategy_tests {
        use super::*;

        #[tokio::test]
        async fn test_migrate_fee_payment_strategy_in_memory_storage() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let relayer_model = ctx.relayer_model.clone();

            let provider = MockStellarProviderTrait::new();
            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let mut relayer_repo = MockRelayerRepository::new();
            // Mock in-memory storage
            relayer_repo
                .expect_is_persistent_storage()
                .returning(|| false);
            // Should not call update_policy for in-memory storage
            relayer_repo.expect_update_policy().times(0);

            let tx_repo = MockTransactionRepository::new();
            let job_producer = MockJobProducerTrait::new();
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model.clone(),
                signer,
                provider,
                StellarRelayerDependencies::new(
                    Arc::new(relayer_repo),
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer.migrate_fee_payment_strategy_if_needed().await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_migrate_fee_payment_strategy_already_set() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let mut relayer_model = ctx.relayer_model.clone();

            // Set fee_payment_strategy
            let mut policy = RelayerStellarPolicy::default();
            policy.fee_payment_strategy = Some(StellarFeePaymentStrategy::User);
            relayer_model.policies = RelayerNetworkPolicy::Stellar(policy);

            let provider = MockStellarProviderTrait::new();
            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let mut relayer_repo = MockRelayerRepository::new();
            relayer_repo
                .expect_is_persistent_storage()
                .returning(|| true);
            // Should not call update_policy since already set
            relayer_repo.expect_update_policy().times(0);

            let tx_repo = MockTransactionRepository::new();
            let job_producer = MockJobProducerTrait::new();
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model.clone(),
                signer,
                provider,
                StellarRelayerDependencies::new(
                    Arc::new(relayer_repo),
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer.migrate_fee_payment_strategy_if_needed().await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_migrate_fee_payment_strategy_migration_needed() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let relayer_model = ctx.relayer_model.clone();

            let provider = MockStellarProviderTrait::new();
            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let mut relayer_repo = MockRelayerRepository::new();
            relayer_repo
                .expect_is_persistent_storage()
                .returning(|| true);
            relayer_repo
                .expect_update_policy()
                .times(1)
                .returning(|_, policy| {
                    // Verify the policy is set to Relayer
                    if let RelayerNetworkPolicy::Stellar(stellar_policy) = &policy {
                        assert_eq!(
                            stellar_policy.fee_payment_strategy,
                            Some(StellarFeePaymentStrategy::Relayer)
                        );
                    }
                    Ok(RelayerRepoModel::default())
                });

            let tx_repo = MockTransactionRepository::new();
            let job_producer = MockJobProducerTrait::new();
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model.clone(),
                signer,
                provider,
                StellarRelayerDependencies::new(
                    Arc::new(relayer_repo),
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer.migrate_fee_payment_strategy_if_needed().await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_migrate_fee_payment_strategy_update_fails() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let relayer_model = ctx.relayer_model.clone();

            let provider = MockStellarProviderTrait::new();
            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let mut relayer_repo = MockRelayerRepository::new();
            relayer_repo
                .expect_is_persistent_storage()
                .returning(|| true);
            relayer_repo
                .expect_update_policy()
                .times(1)
                .returning(|_, _| {
                    Err(RepositoryError::TransactionFailure(
                        "Database error".to_string(),
                    ))
                });

            let tx_repo = MockTransactionRepository::new();
            let job_producer = MockJobProducerTrait::new();
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model.clone(),
                signer,
                provider,
                StellarRelayerDependencies::new(
                    Arc::new(relayer_repo),
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer.migrate_fee_payment_strategy_if_needed().await;
            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                RelayerError::PolicyConfigurationError(_)
            ));
        }
    }

    // Tests for check_balance_and_trigger_token_swap_if_needed
    mod check_balance_and_trigger_token_swap_tests {
        use super::*;
        use crate::models::RelayerStellarSwapConfig;

        #[tokio::test]
        async fn test_check_balance_no_swap_config() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let relayer_model = ctx.relayer_model.clone();

            let provider = MockStellarProviderTrait::new();
            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let relayer_repo = MockRelayerRepository::new();
            let tx_repo = MockTransactionRepository::new();
            let job_producer = MockJobProducerTrait::new();
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model.clone(),
                signer,
                provider,
                StellarRelayerDependencies::new(
                    Arc::new(relayer_repo),
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer
                .check_balance_and_trigger_token_swap_if_needed()
                .await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_check_balance_no_threshold() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let mut relayer_model = ctx.relayer_model.clone();

            // Set up swap config without threshold
            let mut policy = RelayerStellarPolicy::default();
            policy.swap_config = Some(RelayerStellarSwapConfig {
                strategies: vec![],
                min_balance_threshold: None,
                cron_schedule: None,
            });
            relayer_model.policies = RelayerNetworkPolicy::Stellar(policy);

            let provider = MockStellarProviderTrait::new();
            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let relayer_repo = MockRelayerRepository::new();
            let tx_repo = MockTransactionRepository::new();
            let job_producer = MockJobProducerTrait::new();
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model.clone(),
                signer,
                provider,
                StellarRelayerDependencies::new(
                    Arc::new(relayer_repo),
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer
                .check_balance_and_trigger_token_swap_if_needed()
                .await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_check_balance_above_threshold() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let mut relayer_model = ctx.relayer_model.clone();

            // Set up swap config with threshold
            let mut policy = RelayerStellarPolicy::default();
            policy.swap_config = Some(RelayerStellarSwapConfig {
                strategies: vec![],
                min_balance_threshold: Some(1000000), // 1 XLM
                cron_schedule: None,
            });
            relayer_model.policies = RelayerNetworkPolicy::Stellar(policy);

            let mut provider = MockStellarProviderTrait::new();
            // Mock get_account to return balance above threshold
            provider.expect_get_account().returning(|_| {
                Box::pin(async {
                    Ok(AccountEntry {
                        account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                        balance: 10000000, // 10 XLM (above threshold)
                        ext: AccountEntryExt::V0,
                        flags: 0,
                        home_domain: String32::default(),
                        inflation_dest: None,
                        seq_num: SequenceNumber(5),
                        num_sub_entries: 0,
                        signers: VecM::default(),
                        thresholds: Thresholds([0, 0, 0, 0]),
                    })
                })
            });

            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let relayer_repo = MockRelayerRepository::new();
            let tx_repo = MockTransactionRepository::new();
            let job_producer = MockJobProducerTrait::new();
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model.clone(),
                signer,
                provider,
                StellarRelayerDependencies::new(
                    Arc::new(relayer_repo),
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer
                .check_balance_and_trigger_token_swap_if_needed()
                .await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_check_balance_provider_error() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let mut relayer_model = ctx.relayer_model.clone();

            // Set up swap config with threshold
            let mut policy = RelayerStellarPolicy::default();
            policy.swap_config = Some(RelayerStellarSwapConfig {
                strategies: vec![],
                min_balance_threshold: Some(1000000),
                cron_schedule: None,
            });
            relayer_model.policies = RelayerNetworkPolicy::Stellar(policy);

            let mut provider = MockStellarProviderTrait::new();
            provider.expect_get_account().returning(|_| {
                Box::pin(async { Err(ProviderError::Other("Network error".to_string())) })
            });

            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let relayer_repo = MockRelayerRepository::new();
            let tx_repo = MockTransactionRepository::new();
            let job_producer = MockJobProducerTrait::new();
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model.clone(),
                signer,
                provider,
                StellarRelayerDependencies::new(
                    Arc::new(relayer_repo),
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer
                .check_balance_and_trigger_token_swap_if_needed()
                .await;
            assert!(result.is_err());
        }
    }

    // Tests for check_health
    mod check_health_tests {
        use super::*;
        use crate::models::RelayerStellarSwapConfig;

        #[tokio::test]
        async fn test_check_health_success() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let relayer_model = ctx.relayer_model.clone();

            let mut provider = MockStellarProviderTrait::new();
            provider.expect_get_account().returning(|_| {
                Box::pin(async {
                    Ok(AccountEntry {
                        account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                        balance: 10000000,
                        ext: AccountEntryExt::V0,
                        flags: 0,
                        home_domain: String32::default(),
                        inflation_dest: None,
                        seq_num: SequenceNumber(5),
                        num_sub_entries: 0,
                        signers: VecM::default(),
                        thresholds: Thresholds([0, 0, 0, 0]),
                    })
                })
            });

            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let relayer_repo = MockRelayerRepository::new();
            let tx_repo = MockTransactionRepository::new();
            let job_producer = MockJobProducerTrait::new();

            let mut counter = MockTransactionCounterServiceTrait::new();
            counter
                .expect_set()
                .returning(|_| Box::pin(async { Ok(()) }));

            let relayer = StellarRelayer::new(
                relayer_model.clone(),
                signer,
                provider,
                StellarRelayerDependencies::new(
                    Arc::new(relayer_repo),
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer.check_health().await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_check_health_sequence_sync_fails() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let relayer_model = ctx.relayer_model.clone();

            let mut provider = MockStellarProviderTrait::new();
            provider.expect_get_account().returning(|_| {
                Box::pin(async { Err(ProviderError::Other("Network error".to_string())) })
            });

            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let relayer_repo = MockRelayerRepository::new();
            let tx_repo = MockTransactionRepository::new();
            let job_producer = MockJobProducerTrait::new();
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model.clone(),
                signer,
                provider,
                StellarRelayerDependencies::new(
                    Arc::new(relayer_repo),
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer.check_health().await;
            assert!(result.is_err());
            let failures = result.unwrap_err();
            assert_eq!(failures.len(), 1);
            assert!(matches!(
                failures[0],
                HealthCheckFailure::SequenceSyncFailed(_)
            ));
        }

        #[tokio::test]
        async fn test_check_health_with_user_fee_strategy() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let mut relayer_model = ctx.relayer_model.clone();

            // Set up user fee payment strategy
            let mut policy = RelayerStellarPolicy::default();
            policy.fee_payment_strategy = Some(StellarFeePaymentStrategy::User);
            policy.swap_config = Some(RelayerStellarSwapConfig {
                strategies: vec![],
                min_balance_threshold: Some(1000000),
                cron_schedule: None,
            });
            relayer_model.policies = RelayerNetworkPolicy::Stellar(policy);

            let mut provider = MockStellarProviderTrait::new();
            provider.expect_get_account().returning(|_| {
                Box::pin(async {
                    Ok(AccountEntry {
                        account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                        balance: 10000000, // Above threshold
                        ext: AccountEntryExt::V0,
                        flags: 0,
                        home_domain: String32::default(),
                        inflation_dest: None,
                        seq_num: SequenceNumber(5),
                        num_sub_entries: 0,
                        signers: VecM::default(),
                        thresholds: Thresholds([0, 0, 0, 0]),
                    })
                })
            });

            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let relayer_repo = MockRelayerRepository::new();
            let tx_repo = MockTransactionRepository::new();
            let job_producer = MockJobProducerTrait::new();

            let mut counter = MockTransactionCounterServiceTrait::new();
            counter
                .expect_set()
                .returning(|_| Box::pin(async { Ok(()) }));

            let relayer = StellarRelayer::new(
                relayer_model.clone(),
                signer,
                provider,
                StellarRelayerDependencies::new(
                    Arc::new(relayer_repo),
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let result = relayer.check_health().await;
            // Should pass even with user fee strategy
            assert!(result.is_ok());
        }
    }

    // Tests for RPC method
    mod rpc_tests {
        use super::*;
        use crate::models::{JsonRpcId, StellarRpcRequest};

        #[tokio::test]
        async fn test_rpc_invalid_network_request() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let relayer_model = ctx.relayer_model.clone();

            let provider = MockStellarProviderTrait::new();
            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let relayer_repo = MockRelayerRepository::new();
            let tx_repo = MockTransactionRepository::new();
            let job_producer = MockJobProducerTrait::new();
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model.clone(),
                signer,
                provider,
                StellarRelayerDependencies::new(
                    Arc::new(relayer_repo),
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            // Create a request with wrong network type
            let request = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: Some(JsonRpcId::Number(1)),
                params: NetworkRpcRequest::Evm(crate::models::EvmRpcRequest::RawRpcRequest {
                    method: "eth_blockNumber".to_string(),
                    params: serde_json::Value::Null,
                }),
            };

            let result = relayer.rpc(request).await;
            assert!(result.is_ok());
            let response = result.unwrap();
            // Should return an error response for invalid network type
            assert!(response.error.is_some());
        }

        #[tokio::test]
        async fn test_rpc_provider_error() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let relayer_model = ctx.relayer_model.clone();

            let mut provider = MockStellarProviderTrait::new();
            provider.expect_raw_request_dyn().returning(|_, _, _| {
                Box::pin(async { Err(ProviderError::Other("RPC error".to_string())) })
            });

            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let relayer_repo = MockRelayerRepository::new();
            let tx_repo = MockTransactionRepository::new();
            let job_producer = MockJobProducerTrait::new();
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model.clone(),
                signer,
                provider,
                StellarRelayerDependencies::new(
                    Arc::new(relayer_repo),
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let request = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: Some(JsonRpcId::Number(1)),
                params: NetworkRpcRequest::Stellar(StellarRpcRequest::RawRpcRequest {
                    method: "getHealth".to_string(),
                    params: serde_json::Value::Null,
                }),
            };

            let result = relayer.rpc(request).await;
            assert!(result.is_ok());
            let response = result.unwrap();
            // Should return an error response for provider error
            assert!(response.error.is_some());
        }

        #[tokio::test]
        async fn test_rpc_success() {
            let ctx = TestCtx::default();
            ctx.setup_network().await;
            let relayer_model = ctx.relayer_model.clone();

            let mut provider = MockStellarProviderTrait::new();
            provider.expect_raw_request_dyn().returning(|_, _, _| {
                Box::pin(async { Ok(serde_json::json!({"status": "healthy"})) })
            });

            let signer = Arc::new(MockStellarSignTrait::new());
            let dex_service = create_mock_dex_service();

            let relayer_repo = MockRelayerRepository::new();
            let tx_repo = MockTransactionRepository::new();
            let job_producer = MockJobProducerTrait::new();
            let counter = MockTransactionCounterServiceTrait::new();

            let relayer = StellarRelayer::new(
                relayer_model.clone(),
                signer,
                provider,
                StellarRelayerDependencies::new(
                    Arc::new(relayer_repo),
                    ctx.network_repository.clone(),
                    Arc::new(tx_repo),
                    Arc::new(counter),
                    Arc::new(job_producer),
                ),
                dex_service,
            )
            .await
            .unwrap();

            let request = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: Some(JsonRpcId::Number(1)),
                params: NetworkRpcRequest::Stellar(StellarRpcRequest::RawRpcRequest {
                    method: "getHealth".to_string(),
                    params: serde_json::Value::Null,
                }),
            };

            let result = relayer.rpc(request).await;
            assert!(result.is_ok());
            let response = result.unwrap();
            assert!(response.error.is_none());
            assert!(response.result.is_some());
        }
    }
}
