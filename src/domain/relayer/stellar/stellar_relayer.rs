use crate::domain::map_provider_error;
use crate::domain::relayer::evm::create_error_response;
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
    constants::{
        DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE, STELLAR_SMALLEST_UNIT_NAME,
        STELLAR_STATUS_CHECK_INITIAL_DELAY_SECONDS,
    },
    domain::relayer::SwapResult,
    domain::{
        create_success_response, transaction::stellar::fetch_next_sequence_from_chain,
        BalanceResponse, SignDataRequest, SignDataResponse, SignTransactionExternalResponse,
        SignTransactionExternalResponseStellar, SignTransactionRequest, SignTypedDataRequest,
        StellarRelayerDexTrait,
    },
    jobs::{JobProducerTrait, RelayerHealthCheck, TransactionRequest, TransactionStatusCheck},
    models::{
        produce_relayer_disabled_payload, AssetSpec, DeletePendingTransactionsResponse,
        DisabledReason, HealthCheckFailure, JsonRpcRequest, JsonRpcResponse, NetworkRepoModel,
        NetworkRpcRequest, NetworkRpcResult, NetworkTransactionRequest, NetworkType, OperationSpec,
        RelayerRepoModel, RelayerStatus, RepositoryError, RpcErrorCodes, StellarFeeEstimateResult,
        StellarNetwork, StellarPrepareTransactionResult, StellarRpcRequest, TransactionRepoModel,
        TransactionStatus,
    },
    repositories::{NetworkRepository, RelayerRepository, Repository, TransactionRepository},
    services::{
        provider::{StellarProvider, StellarProviderTrait},
        signer::{StellarSignTrait, StellarSigner},
        stellar_dex::{PathsService, StellarDexServiceTrait},
        TransactionCounterService, TransactionCounterServiceTrait,
    },
    utils::calculate_scheduled_timestamp,
};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use eyre::Result;
use soroban_rs::xdr::{Limits, Operation, WriteXdr};
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::domain::relayer::xdr_utils::extract_source_account;
use crate::domain::relayer::{GasAbstractionTrait, Relayer, RelayerError};
use crate::domain::transaction::stellar::utils::{
    add_operation_to_envelope, amount_to_ui_amount, create_fee_payment_operation,
    estimate_base_fee, parse_transaction_and_count_operations, parse_transaction_envelope,
    set_time_bounds,
};
use crate::domain::transaction::stellar::{
    StellarTransactionValidationError, StellarTransactionValidator,
};

/// Fee quote structure containing fee estimates in both tokens and stroops
struct FeeQuote {
    fee_in_token: u64,
    fee_in_token_ui: String,
    fee_in_stroops: u64,
    conversion_rate: f64,
}

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
pub struct StellarRelayer<P, RR, NR, TR, J, TCS, S>
where
    P: StellarProviderTrait + Send + Sync + 'static,
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    TCS: TransactionCounterServiceTrait + Send + Sync + 'static,
    S: StellarSignTrait + Send + Sync + 'static,
{
    relayer: RelayerRepoModel,
    signer: S,
    network: StellarNetwork,
    provider: P,
    relayer_repository: Arc<RR>,
    network_repository: Arc<NR>,
    transaction_repository: Arc<TR>,
    transaction_counter_service: Arc<TCS>,
    job_producer: Arc<J>,
    dex_service: Arc<dyn StellarDexServiceTrait + Send + Sync>,
}

pub type DefaultStellarRelayer<J, TR, NR, RR, TCR> =
    StellarRelayer<StellarProvider, RR, NR, TR, J, TransactionCounterService<TCR>, StellarSigner>;

impl<P, RR, NR, TR, J, TCS, S> StellarRelayer<P, RR, NR, TR, J, TCS, S>
where
    P: StellarProviderTrait + Send + Sync,
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    TCS: TransactionCounterServiceTrait + Send + Sync + 'static,
    S: StellarSignTrait + Send + Sync + 'static,
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
    ///
    /// # Returns
    ///
    /// * `Ok(StellarRelayer)` - Successfully initialized relayer ready for operation
    /// * `Err(RelayerError)` - If initialization fails due to configuration or validation errors
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        relayer: RelayerRepoModel,
        signer: S,
        provider: P,
        dependencies: StellarRelayerDependencies<RR, NR, TR, J, TCS>,
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

        // Create DEX service for swap operations
        let horizon_base = network.horizon_base_url().map_err(|e| {
            RelayerError::NetworkConfiguration(format!("Failed to get Horizon base URL: {}", e))
        })?;
        let dex_service = Arc::new(PathsService::new(horizon_base).map_err(|e| {
            RelayerError::NetworkConfiguration(format!("Failed to create DEX service: {}", e))
        })?);

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

    /// Estimate fee and convert to token amount using DEX service
    async fn estimate_and_convert_fee(
        &self,
        xlm_fee: u64,
        fee_token: &str,
        fee_margin_percentage: Option<f32>,
    ) -> Result<(FeeQuote, u64), RelayerError> {
        // Handle native XLM - no conversion needed
        if fee_token == "native" || fee_token.is_empty() {
            let buffered_fee = if let Some(margin) = fee_margin_percentage {
                (xlm_fee as f64 * (1.0 + margin as f64 / 100.0)) as u64
            } else {
                xlm_fee
            };

            return Ok((
                FeeQuote {
                    fee_in_token: buffered_fee,
                    fee_in_token_ui: amount_to_ui_amount(buffered_fee, 7),
                    fee_in_stroops: buffered_fee,
                    conversion_rate: 1.0,
                },
                buffered_fee,
            ));
        }

        // Apply fee margin if specified
        let buffered_xlm_fee = if let Some(margin) = fee_margin_percentage {
            (xlm_fee as f64 * (1.0 + margin as f64 / 100.0)) as u64
        } else {
            xlm_fee
        };

        // Get slippage from policy or use default
        let policy = self.relayer.policies.get_stellar_policy();
        let slippage = policy
            .get_allowed_token_entry(fee_token)
            .and_then(|token| {
                token
                    .swap_config
                    .as_ref()
                    .and_then(|config| config.slippage_percentage)
            })
            .or(policy.slippage_percentage)
            .unwrap_or(DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE);

        // Get quote from DEX service
        let quote = self
            .dex_service
            .get_xlm_to_token_quote(fee_token, buffered_xlm_fee, slippage)
            .await
            .map_err(|e| RelayerError::Internal(format!("Failed to get quote: {}", e)))?;

        // Get token decimals from policy or default to 7
        let decimals = policy.get_allowed_token_decimals(fee_token).unwrap_or(7);

        // Calculate conversion rate
        let conversion_rate = if buffered_xlm_fee > 0 {
            quote.out_amount as f64 / buffered_xlm_fee as f64
        } else {
            0.0
        };

        Ok((
            FeeQuote {
                fee_in_token: quote.out_amount,
                fee_in_token_ui: amount_to_ui_amount(quote.out_amount, decimals),
                fee_in_stroops: buffered_xlm_fee,
                conversion_rate,
            },
            buffered_xlm_fee,
        ))
    }
}

#[async_trait]
impl<P, RR, NR, TR, J, TCS, S> Relayer for StellarRelayer<P, RR, NR, TR, J, TCS, S>
where
    P: StellarProviderTrait + Send + Sync + 'static,
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
                    crate::models::NetworkType::Stellar,
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
                RelayerError::ProviderError(format!("Failed to fetch account for balance: {}", e))
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
                RelayerError::ProviderError(format!("Failed to get account details: {}", e))
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

        match self.check_health().await {
            Ok(_) => {
                // All checks passed
                if self.relayer.system_disabled {
                    // Silently re-enable if was disabled (startup, not recovery)
                    self.relayer_repository
                        .enable_relayer(self.relayer.id.clone())
                        .await?;
                }

                info!(
                    "Stellar relayer initialized successfully: {}",
                    self.relayer.id
                );
                Ok(())
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

                Ok(())
            }
        }
    }

    async fn check_health(&self) -> Result<(), Vec<HealthCheckFailure>> {
        debug!(
            "running health checks for Stellar relayer {}",
            self.relayer.id
        );

        match self.sync_sequence().await {
            Ok(_) => {
                debug!(
                    "all health checks passed for Stellar relayer {}",
                    self.relayer.id
                );
                Ok(())
            }
            Err(e) => {
                let reason = HealthCheckFailure::SequenceSyncFailed(e.to_string());
                warn!("health checks failed: {:?}", reason);
                Err(vec![reason])
            }
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

#[async_trait]
impl<P, RR, NR, TR, J, TCS, S> GasAbstractionTrait for StellarRelayer<P, RR, NR, TR, J, TCS, S>
where
    P: StellarProviderTrait + Send + Sync,
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    TCS: TransactionCounterServiceTrait + Send + Sync + 'static,
    S: StellarSignTrait + Send + Sync + 'static,
{
    async fn estimate_fee(
        &self,
        params: crate::models::StellarFeeEstimateRequestParams,
    ) -> Result<crate::models::StellarFeeEstimateResult, RelayerError> {
        info!(
            "Processing fee estimate request for token: {}",
            params.fee_token
        );

        // Validate allowed token
        let policy = self.relayer.policies.get_stellar_policy();
        StellarTransactionValidator::validate_allowed_token(&params.fee_token, &policy).map_err(
            |e| RelayerError::Internal(format!("Failed to validate allowed token: {}", e)),
        )?;

        // Parse transaction and count operations
        let num_operations =
            parse_transaction_and_count_operations(&params.transaction).map_err(|e| {
                RelayerError::Internal(format!(
                    "Failed to parse transaction and count operations: {}",
                    e
                ))
            })?;

        // Estimate base XLM fee
        let xlm_fee = estimate_base_fee(num_operations);

        // Convert to token amount via DEX service
        let (fee_quote, _) = self
            .estimate_and_convert_fee(xlm_fee, &params.fee_token, policy.fee_margin_percentage)
            .await
            .map_err(|e| {
                RelayerError::Internal(format!("Failed to estimate and convert fee: {}", e))
            })?;

        // Validate max fee
        StellarTransactionValidator::validate_max_fee(fee_quote.fee_in_stroops, &policy)
            .map_err(|e| RelayerError::Internal(format!("Failed to validate max fee: {}", e)))?;

        // Validate token-specific max fee
        StellarTransactionValidator::validate_token_max_fee(
            &params.fee_token,
            fee_quote.fee_in_token,
            &policy,
        )
        .map_err(|e| {
            RelayerError::Internal(format!("Failed to validate token-specific max fee: {}", e))
        })?;

        Ok(StellarFeeEstimateResult {
            estimated_fee: fee_quote.fee_in_token_ui,
            conversion_rate: fee_quote.conversion_rate.to_string(),
        })
    }

    async fn prepare_transaction(
        &self,
        params: crate::models::StellarPrepareTransactionRequestParams,
    ) -> Result<crate::models::StellarPrepareTransactionResult, RelayerError> {
        info!(
            "Processing prepare transaction request for token: {}",
            params.fee_token
        );

        // Validate allowed token
        let policy = self.relayer.policies.get_stellar_policy();
        StellarTransactionValidator::validate_allowed_token(&params.fee_token, &policy).map_err(
            |e| RelayerError::Internal(format!("Failed to validate allowed token: {}", e)),
        )?;

        // Parse transaction and count operations
        let num_operations =
            parse_transaction_and_count_operations(&params.transaction).map_err(|e| {
                RelayerError::Internal(format!(
                    "Failed to parse transaction and count operations: {}",
                    e
                ))
            })?;

        // Estimate base XLM fee
        let xlm_fee = estimate_base_fee(num_operations);

        // Convert to token amount via DEX service
        let (fee_quote, buffered_xlm_fee) = self
            .estimate_and_convert_fee(xlm_fee, &params.fee_token, policy.fee_margin_percentage)
            .await
            .map_err(|e| {
                RelayerError::Internal(format!("Failed to estimate and convert fee: {}", e))
            })?;

        // Validate max fee
        StellarTransactionValidator::validate_max_fee(fee_quote.fee_in_stroops, &policy)
            .map_err(|e| RelayerError::Internal(format!("Failed to validate max fee: {}", e)))?;

        // Validate token-specific max fee
        StellarTransactionValidator::validate_token_max_fee(
            &params.fee_token,
            fee_quote.fee_in_token,
            &policy,
        )
        .map_err(|e| {
            RelayerError::Internal(format!("Failed to validate token-specific max fee: {}", e))
        })?;

        // Parse transaction to add payment operation
        let mut envelope = parse_transaction_envelope(&params.transaction).map_err(|e| {
            RelayerError::Internal(format!("Failed to parse transaction envelope: {}", e))
        })?;

        // Extract source account from envelope
        let source_account = extract_source_account(&envelope).map_err(|e| {
            RelayerError::Internal(format!(
                "Failed to extract source account from envelope: {}",
                e
            ))
        })?;

        // Create payment operation for fee payment
        let payment_op_spec = create_fee_payment_operation(
            &self.relayer.address,
            &params.fee_token,
            fee_quote.fee_in_token as i64, // PaymentOp uses i64
        )
        .map_err(|e| {
            RelayerError::Internal(format!("Failed to create fee payment operation: {}", e))
        })?;

        // Convert OperationSpec to XDR Operation
        let payment_op = Operation::try_from(payment_op_spec).map_err(|e| {
            RelayerError::Internal(format!("Failed to convert payment operation: {}", e))
        })?;

        // Add payment operation to transaction
        add_operation_to_envelope(&mut envelope, payment_op)
            .map_err(|e| RelayerError::Internal(format!("Failed to add operation: {}", e)))?;

        // Set time bounds (valid for 5 minutes due to quote volatility)
        let valid_until = Utc::now() + Duration::minutes(5);
        set_time_bounds(&mut envelope, valid_until)
            .map_err(|e| RelayerError::Internal(format!("Failed to set time bounds: {}", e)))?;

        // Serialize extended transaction
        let extended_xdr = envelope
            .to_xdr_base64(Limits::none())
            .map_err(|e| RelayerError::Internal(format!("Failed to serialize XDR: {}", e)))?;

        // Construct Stellar StellarPrepareTransactionResult explicitly to avoid ambiguity with Solana type
        // Use serde_json to ensure we get the correct Stellar struct
        let result_json = serde_json::json!({
            "transaction": extended_xdr,
            "fee_in_token": fee_quote.fee_in_token_ui,
            "fee_in_stroops": buffered_xlm_fee.to_string(),
            "fee_token": params.fee_token,
            "valid_until": valid_until.to_rfc3339(),
        });
        let result: StellarPrepareTransactionResult =
            serde_json::from_value(result_json).map_err(|e| {
                RelayerError::Internal(format!(
                    "Failed to create StellarPrepareTransactionResult: {}",
                    e
                ))
            })?;
        Ok(result)
    }
}

#[async_trait]
impl<P, RR, NR, TR, J, TCS, S> StellarRelayerDexTrait for StellarRelayer<P, RR, NR, TR, J, TCS, S>
where
    P: StellarProviderTrait + Send + Sync,
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    TCS: TransactionCounterServiceTrait + Send + Sync + 'static,
    S: StellarSignTrait + Send + Sync + 'static,
{
    /// Processes a token swap request for the given relayer ID:
    ///
    /// 1. Loads the relayer's policy (must include swap_config & strategy).
    /// 2. Checks XLM balance - if below threshold, swaps collected tokens to XLM.
    /// 3. Iterates allowed tokens, checking balances and calculating swap amounts.
    /// 4. Executes swaps through the DEX service (Paths service).
    /// 5. Collects and returns all `SwapResult`s (empty if no swaps were needed).
    ///
    /// Returns a `RelayerError` on any repository, provider, or swap execution failure.
    async fn handle_token_swap_request(
        &self,
        relayer_id: String,
    ) -> Result<Vec<SwapResult>, RelayerError> {
        debug!("handling token swap request for relayer {}", relayer_id);
        let relayer = self
            .relayer_repository
            .get_by_id(relayer_id.clone())
            .await?;

        let policy = relayer.policies.get_stellar_policy();

        let swap_config = match policy.get_swap_config() {
            Some(config) => config,
            None => {
                debug!(%relayer_id, "No swap configuration specified for relayer; Exiting.");
                return Ok(vec![]);
            }
        };

        match swap_config.strategy {
            Some(_strategy) => {
                // Strategy is set, proceed with swap
            }
            None => {
                debug!(%relayer_id, "No swap strategy specified for relayer; Exiting.");
                return Ok(vec![]);
            }
        }

        // Check XLM balance
        let account_entry = self
            .provider
            .get_account(&relayer.address)
            .await
            .map_err(|e| RelayerError::ProviderError(format!("Failed to get account: {}", e)))?;

        let xlm_balance = account_entry.balance as u64;

        // Check if balance is below threshold
        if let Some(threshold) = swap_config.min_balance_threshold {
            if xlm_balance >= threshold {
                debug!(
                    %relayer_id,
                    balance = xlm_balance,
                    threshold = threshold,
                    "XLM balance above threshold, no swap needed"
                );
                return Ok(vec![]);
            }
        }

        info!(
            %relayer_id,
            balance = xlm_balance,
            "XLM balance below threshold, checking tokens for swap"
        );

        // Get allowed tokens
        let allowed_tokens = policy.get_allowed_tokens();
        if allowed_tokens.is_empty() {
            debug!(%relayer_id, "No allowed tokens configured for swap");
            return Ok(vec![]);
        }

        // Note: For Stellar, token balances are stored in account trustlines
        // This requires Horizon API integration to fetch balances for each asset
        // For now, we'll create a placeholder implementation that can be extended
        // TODO: Implement token balance fetching from Horizon API
        // TODO: Calculate swap amounts based on min/max/retain settings
        // TODO: Execute swaps using DEX service

        warn!(
            %relayer_id,
            "Token swap implementation requires Horizon API integration for balance fetching"
        );

        // Return empty results for now - implementation can be extended later
        Ok(vec![])
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
        let signer = MockStellarSignTrait::new();

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
        let signer = MockStellarSignTrait::new();

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
        let signer = MockStellarSignTrait::new();

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
        let signer = MockStellarSignTrait::new();

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
        let signer = MockStellarSignTrait::new();

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
        let signer = MockStellarSignTrait::new();

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
        let signer = MockStellarSignTrait::new();

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
        let signer = MockStellarSignTrait::new();
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
        let signer = MockStellarSignTrait::new();
        let job_producer = MockJobProducerTrait::new();
        let relayer_repo = MockRelayerRepository::new();

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
        let signer = MockStellarSignTrait::new();

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
        let signer = MockStellarSignTrait::new();

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
            let signer = MockStellarSignTrait::new();

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
            let signer = MockStellarSignTrait::new();

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
            let signer = MockStellarSignTrait::new();

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
            let signer = MockStellarSignTrait::new();

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
            let signer = MockStellarSignTrait::new();

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
            let signer = MockStellarSignTrait::new();

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
}
