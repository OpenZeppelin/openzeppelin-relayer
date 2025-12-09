//! # Solana Relayer Module
//!
//! This module implements a relayer for the Solana network. It defines a trait
//! `SolanaRelayerTrait` for common operations such as sending JSON RPC requests,
//! fetching balance information, signing transactions, etc. The module uses a
//! SolanaProvider for making RPC calls.
//!
//! It integrates with other parts of the system including the job queue ([`JobProducer`]),
//! in-memory repositories, and the application's domain models.
use std::{str::FromStr, sync::Arc};

use crate::constants::SOLANA_STATUS_CHECK_INITIAL_DELAY_SECONDS;
use crate::domain::relayer::solana::rpc::SolanaRpcMethods;
use crate::domain::{
    create_error_response, GasAbstractionTrait, Relayer, SignDataRequest,
    SignTransactionExternalResponse, SignTransactionRequest, SignTransactionResponse,
    SignTransactionResponseSolana, SignTypedDataRequest, SolanaRpcHandlerType, SwapParams,
};
use crate::jobs::{TransactionRequest, TransactionStatusCheck};
use crate::models::transaction::request::{
    SponsoredTransactionBuildRequest, SponsoredTransactionQuoteRequest,
};
use crate::models::{
    DeletePendingTransactionsResponse, JsonRpcRequest, JsonRpcResponse, NetworkRpcRequest,
    NetworkRpcResult, NetworkTransactionRequest, RelayerStatus, RepositoryError, RpcErrorCodes,
    SolanaRpcRequest, SolanaRpcResult, SolanaSignAndSendTransactionRequestParams,
    SolanaSignTransactionRequestParams, SponsoredTransactionBuildResponse,
    SponsoredTransactionQuoteResponse,
};
use crate::utils::calculate_scheduled_timestamp;
use crate::{
    constants::{
        DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE, DEFAULT_SOLANA_MIN_BALANCE,
        SOLANA_SMALLEST_UNIT_NAME, WRAPPED_SOL_MINT,
    },
    domain::{relayer::RelayerError, BalanceResponse, DexStrategy, SolanaRelayerDexTrait},
    jobs::{JobProducerTrait, RelayerHealthCheck, TokenSwapRequest},
    models::{
        produce_relayer_disabled_payload, produce_solana_dex_webhook_payload, DisabledReason,
        HealthCheckFailure, NetworkRepoModel, NetworkTransactionData, NetworkType,
        RelayerNetworkPolicy, RelayerRepoModel, RelayerSolanaPolicy, SolanaAllowedTokensPolicy,
        SolanaDexPayload, SolanaFeePaymentStrategy, SolanaNetwork, SolanaTransactionData,
        TransactionRepoModel, TransactionStatus,
    },
    repositories::{NetworkRepository, RelayerRepository, Repository, TransactionRepository},
    services::{
        provider::{SolanaProvider, SolanaProviderTrait},
        signer::{Signer, SolanaSignTrait, SolanaSigner},
        JupiterService, JupiterServiceTrait,
    },
};

use async_trait::async_trait;
use eyre::Result;
use futures::future::try_join_all;
use solana_sdk::{account::Account, pubkey::Pubkey};
use tracing::{debug, error, info, warn};

use super::{NetworkDex, SolanaRpcError, SolanaTokenProgram, SwapResult, TokenAccount};

#[allow(dead_code)]
struct TokenSwapCandidate<'a> {
    policy: &'a SolanaAllowedTokensPolicy,
    account: TokenAccount,
    swap_amount: u64,
}

#[allow(dead_code)]
pub struct SolanaRelayer<RR, TR, J, S, JS, SP, NR>
where
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: SolanaSignTrait + Signer + Send + Sync + 'static,
    JS: JupiterServiceTrait + Send + Sync + 'static,
    SP: SolanaProviderTrait + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
{
    relayer: RelayerRepoModel,
    signer: Arc<S>,
    network: SolanaNetwork,
    provider: Arc<SP>,
    rpc_handler: SolanaRpcHandlerType<SP, S, JS, J, TR>,
    relayer_repository: Arc<RR>,
    transaction_repository: Arc<TR>,
    job_producer: Arc<J>,
    dex_service: Arc<NetworkDex<SP, S, JS>>,
    network_repository: Arc<NR>,
}

pub type DefaultSolanaRelayer<J, TR, RR, NR> =
    SolanaRelayer<RR, TR, J, SolanaSigner, JupiterService, SolanaProvider, NR>;

impl<RR, TR, J, S, JS, SP, NR> SolanaRelayer<RR, TR, J, S, JS, SP, NR>
where
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: SolanaSignTrait + Signer + Send + Sync + 'static,
    JS: JupiterServiceTrait + Send + Sync + 'static,
    SP: SolanaProviderTrait + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
{
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        relayer: RelayerRepoModel,
        signer: Arc<S>,
        relayer_repository: Arc<RR>,
        network_repository: Arc<NR>,
        provider: Arc<SP>,
        rpc_handler: SolanaRpcHandlerType<SP, S, JS, J, TR>,
        transaction_repository: Arc<TR>,
        job_producer: Arc<J>,
        dex_service: Arc<NetworkDex<SP, S, JS>>,
    ) -> Result<Self, RelayerError> {
        let network_repo = network_repository
            .get_by_name(NetworkType::Solana, &relayer.network)
            .await
            .ok()
            .flatten()
            .ok_or_else(|| {
                RelayerError::NetworkConfiguration(format!("Network {} not found", relayer.network))
            })?;

        let network = SolanaNetwork::try_from(network_repo)?;

        Ok(Self {
            relayer,
            signer,
            network,
            provider,
            rpc_handler,
            relayer_repository,
            transaction_repository,
            job_producer,
            dex_service,
            network_repository,
        })
    }

    /// Validates the RPC connection by fetching the latest blockhash.
    ///
    /// This method sends a request to the Solana RPC to obtain the latest blockhash.
    /// If the call fails, it returns a `RelayerError::ProviderError` containing the error message.
    async fn validate_rpc(&self) -> Result<(), RelayerError> {
        self.provider
            .get_latest_blockhash()
            .await
            .map_err(|e| RelayerError::ProviderError(e.to_string()))?;

        Ok(())
    }

    /// Populates the allowed tokens metadata for the Solana relayer policy.
    ///
    /// This method checks whether allowed tokens have been configured in the relayer's policy.
    /// If allowed tokens are provided, it concurrently fetches token metadata from the Solana
    /// provider for each token using its mint address, maps the metadata into instances of
    /// `SolanaAllowedTokensPolicy`, and then updates the relayer policy with the new metadata.
    ///
    /// If no allowed tokens are specified, it logs an informational message and returns the policy
    /// unchanged.
    ///
    /// Finally, the updated policy is stored in the repository.
    async fn populate_allowed_tokens_metadata(&self) -> Result<RelayerSolanaPolicy, RelayerError> {
        let mut policy = self.relayer.policies.get_solana_policy();
        // Check if allowed_tokens is specified; if not, return the policy unchanged.
        let allowed_tokens = match policy.allowed_tokens.as_ref() {
            Some(tokens) if !tokens.is_empty() => tokens,
            _ => {
                info!("No allowed tokens specified; skipping token metadata population.");
                return Ok(policy);
            }
        };

        let token_metadata_futures = allowed_tokens.iter().map(|token| async {
            // Propagate errors from get_token_metadata_from_pubkey instead of panicking.
            let token_metadata = self
                .provider
                .get_token_metadata_from_pubkey(&token.mint)
                .await
                .map_err(|e| RelayerError::ProviderError(e.to_string()))?;
            Ok::<SolanaAllowedTokensPolicy, RelayerError>(SolanaAllowedTokensPolicy {
                mint: token_metadata.mint,
                decimals: Some(token_metadata.decimals as u8),
                symbol: Some(token_metadata.symbol.to_string()),
                max_allowed_fee: token.max_allowed_fee,
                swap_config: token.swap_config.clone(),
            })
        });

        let updated_allowed_tokens = try_join_all(token_metadata_futures).await?;

        policy.allowed_tokens = Some(updated_allowed_tokens);

        self.relayer_repository
            .update_policy(
                self.relayer.id.clone(),
                RelayerNetworkPolicy::Solana(policy.clone()),
            )
            .await?;

        Ok(policy)
    }

    /// Validates the allowed programs policy.
    ///
    /// This method retrieves the allowed programs specified in the Solana relayer policy.
    /// For each allowed program, it fetches the associated account data from the provider and
    /// verifies that the program is executable.
    /// If any of the programs are not executable, it returns a
    /// `RelayerError::PolicyConfigurationError`.
    async fn validate_program_policy(&self) -> Result<(), RelayerError> {
        let policy = self.relayer.policies.get_solana_policy();
        let allowed_programs = match policy.allowed_programs.as_ref() {
            Some(programs) if !programs.is_empty() => programs,
            _ => {
                info!("No allowed programs specified; skipping program validation.");
                return Ok(());
            }
        };
        let account_info_futures = allowed_programs.iter().map(|program| {
            let program = program.clone();
            async move {
                let account = self
                    .provider
                    .get_account_from_str(&program)
                    .await
                    .map_err(|e| RelayerError::ProviderError(e.to_string()))?;
                Ok::<Account, RelayerError>(account)
            }
        });

        let accounts = try_join_all(account_info_futures).await?;

        for account in accounts {
            if !account.executable {
                return Err(RelayerError::PolicyConfigurationError(
                    "Policy Program is not executable".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Checks the relayer's balance and triggers a token swap if the balance is below the
    /// specified threshold.
    async fn check_balance_and_trigger_token_swap_if_needed(&self) -> Result<(), RelayerError> {
        let policy = self.relayer.policies.get_solana_policy();
        let swap_config = match policy.get_swap_config() {
            Some(config) => config,
            None => {
                info!("No swap configuration specified; skipping validation.");
                return Ok(());
            }
        };
        let swap_min_balance_threshold = match swap_config.min_balance_threshold {
            Some(threshold) => threshold,
            None => {
                info!("No swap min balance threshold specified; skipping validation.");
                return Ok(());
            }
        };

        let balance = self
            .provider
            .get_balance(&self.relayer.address)
            .await
            .map_err(|e| RelayerError::ProviderError(e.to_string()))?;

        if balance < swap_min_balance_threshold {
            info!(
                "Sending job request for for relayer  {} swapping tokens due to relayer swap_min_balance_threshold: Balance: {}, swap_min_balance_threshold: {}",
                self.relayer.id, balance, swap_min_balance_threshold
            );

            self.job_producer
                .produce_token_swap_request_job(
                    TokenSwapRequest {
                        relayer_id: self.relayer.id.clone(),
                    },
                    None,
                )
                .await?;
        }

        Ok(())
    }

    // Helper function to calculate swap amount
    fn calculate_swap_amount(
        &self,
        current_balance: u64,
        min_amount: Option<u64>,
        max_amount: Option<u64>,
        retain_min: Option<u64>,
    ) -> Result<u64, RelayerError> {
        // Cap the swap amount at the maximum if specified
        let mut amount = max_amount
            .map(|max| std::cmp::min(current_balance, max))
            .unwrap_or(current_balance);

        // Adjust for retain minimum if specified
        if let Some(retain) = retain_min {
            if current_balance > retain {
                amount = std::cmp::min(amount, current_balance - retain);
            } else {
                // Not enough to retain the minimum after swap
                return Ok(0);
            }
        }

        // Check if we have enough tokens to meet minimum swap requirement
        if let Some(min) = min_amount {
            if amount < min {
                return Ok(0); // Not enough tokens to swap
            }
        }

        Ok(amount)
    }
}

#[async_trait]
impl<RR, TR, J, S, JS, SP, NR> SolanaRelayerDexTrait for SolanaRelayer<RR, TR, J, S, JS, SP, NR>
where
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: SolanaSignTrait + Signer + Send + Sync + 'static,
    JS: JupiterServiceTrait + Send + Sync + 'static,
    SP: SolanaProviderTrait + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
{
    /// Processes a token‐swap request for the given relayer ID:
    ///
    /// 1. Loads the relayer's on‐chain policy (must include swap_config & strategy).
    /// 2. Iterates allowed tokens, fetching each SPL token account and calculating how much
    ///    to swap based on min, max, and retain settings.
    /// 3. Executes each swap through the DEX service (e.g. Jupiter).
    /// 4. Collects and returns all `SwapResult`s (empty if no swaps were needed).
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

        let policy = relayer.policies.get_solana_policy();

        let swap_config = match policy.get_swap_config() {
            Some(config) => config,
            None => {
                debug!(%relayer_id, "No swap configuration specified for relayer; Exiting.");
                return Ok(vec![]);
            }
        };

        match swap_config.strategy {
            Some(strategy) => strategy,
            None => {
                debug!(%relayer_id, "No swap strategy specified for relayer; Exiting.");
                return Ok(vec![]);
            }
        };

        let relayer_pubkey = Pubkey::from_str(&relayer.address)
            .map_err(|e| RelayerError::ProviderError(format!("Invalid relayer address: {e}")))?;

        let tokens_to_swap = {
            let mut eligible_tokens = Vec::<TokenSwapCandidate>::new();

            if let Some(allowed_tokens) = policy.allowed_tokens.as_ref() {
                for token in allowed_tokens {
                    let token_mint = Pubkey::from_str(&token.mint).map_err(|e| {
                        RelayerError::ProviderError(format!("Invalid token mint: {e}"))
                    })?;
                    let token_account = SolanaTokenProgram::get_and_unpack_token_account(
                        &*self.provider,
                        &relayer_pubkey,
                        &token_mint,
                    )
                    .await
                    .map_err(|e| {
                        RelayerError::ProviderError(format!("Failed to get token account: {e}"))
                    })?;

                    let swap_amount = self
                        .calculate_swap_amount(
                            token_account.amount,
                            token
                                .swap_config
                                .as_ref()
                                .and_then(|config| config.min_amount),
                            token
                                .swap_config
                                .as_ref()
                                .and_then(|config| config.max_amount),
                            token
                                .swap_config
                                .as_ref()
                                .and_then(|config| config.retain_min_amount),
                        )
                        .unwrap_or(0);

                    if swap_amount > 0 {
                        debug!(%relayer_id, token = ?token, "token swap eligible for token");

                        // Add the token to the list of eligible tokens for swapping
                        eligible_tokens.push(TokenSwapCandidate {
                            policy: token,
                            account: token_account,
                            swap_amount,
                        });
                    }
                }
            }

            eligible_tokens
        };

        // Execute swap for every eligible token
        let swap_futures = tokens_to_swap.iter().map(|candidate| {
            let token = candidate.policy;
            let swap_amount = candidate.swap_amount;
            let dex = &self.dex_service;
            let relayer_address = self.relayer.address.clone();
            let token_mint = token.mint.clone();
            let relayer_id_clone = relayer_id.clone();
            let slippage_percent = token
                .swap_config
                .as_ref()
                .and_then(|config| config.slippage_percentage)
                .unwrap_or(DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE)
                as f64;

            async move {
                info!(
                    "Swapping {} tokens of type {} for relayer: {}",
                    swap_amount, token_mint, relayer_id_clone
                );

                let swap_result = dex
                    .execute_swap(SwapParams {
                        owner_address: relayer_address,
                        source_mint: token_mint.clone(),
                        destination_mint: WRAPPED_SOL_MINT.to_string(), // SOL mint
                        amount: swap_amount,
                        slippage_percent,
                    })
                    .await;

                match swap_result {
                    Ok(swap_result) => {
                        info!(
                            "Swap successful for relayer: {}. Amount: {}, Destination amount: {}",
                            relayer_id_clone, swap_amount, swap_result.destination_amount
                        );
                        Ok::<SwapResult, RelayerError>(swap_result)
                    }
                    Err(e) => {
                        error!(
                            "Error during token swap for relayer: {}. Error: {}",
                            relayer_id_clone, e
                        );
                        Ok::<SwapResult, RelayerError>(SwapResult {
                            mint: token_mint.clone(),
                            source_amount: swap_amount,
                            destination_amount: 0,
                            transaction_signature: "".to_string(),
                            error: Some(e.to_string()),
                        })
                    }
                }
            }
        });

        let swap_results = try_join_all(swap_futures).await?;

        if !swap_results.is_empty() {
            let total_sol_received: u64 = swap_results
                .iter()
                .map(|result| result.destination_amount)
                .sum();

            info!(
                "Completed {} token swaps for relayer {}, total SOL received: {}",
                swap_results.len(),
                relayer_id,
                total_sol_received
            );

            if let Some(notification_id) = &self.relayer.notification_id {
                let webhook_result = self
                    .job_producer
                    .produce_send_notification_job(
                        produce_solana_dex_webhook_payload(
                            notification_id,
                            "solana_dex".to_string(),
                            SolanaDexPayload {
                                swap_results: swap_results.clone(),
                            },
                        ),
                        None,
                    )
                    .await;

                if let Err(e) = webhook_result {
                    error!(error = %e, "failed to produce notification job");
                }
            }
        }

        Ok(swap_results)
    }
}

#[async_trait]
impl<RR, TR, J, S, JS, SP, NR> Relayer for SolanaRelayer<RR, TR, J, S, JS, SP, NR>
where
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: SolanaSignTrait + Signer + Send + Sync + 'static,
    JS: JupiterServiceTrait + Send + Sync + 'static,
    SP: SolanaProviderTrait + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
{
    async fn process_transaction_request(
        &self,
        network_transaction: crate::models::NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError> {
        let policy = self.relayer.policies.get_solana_policy();
        let user_pays_fee = matches!(
            policy.fee_payment_strategy.unwrap_or_default(),
            SolanaFeePaymentStrategy::User
        );

        // For user-paid fees, delegate to RPC handler (similar to build/quote)
        if user_pays_fee {
            let solana_request = match &network_transaction {
                NetworkTransactionRequest::Solana(req) => req,
                _ => {
                    return Err(RelayerError::ValidationError(
                        "Expected Solana transaction request".to_string(),
                    ));
                }
            };

            // For user-paid fees, we need a pre-built transaction (not instructions)
            let transaction = solana_request.transaction.as_ref().ok_or_else(|| {
                RelayerError::ValidationError(
                    "User-paid fees require a pre-built transaction. Use prepareTransaction RPC method first to build the transaction from instructions.".to_string(),
                )
            })?;

            let params = SolanaSignAndSendTransactionRequestParams {
                transaction: transaction.clone(),
            };

            let result = self
                .rpc_handler
                .rpc_methods()
                .sign_and_send_transaction(params)
                .await
                .map_err(|e| RelayerError::Internal(e.to_string()))?;

            // Fetch the transaction from repository using the ID returned by sign_and_send_transaction
            let transaction = self
                .transaction_repository
                .get_by_id(result.id.clone())
                .await
                .map_err(|e| {
                    RelayerError::Internal(format!(
                        "Failed to fetch transaction after sign and send: {e}"
                    ))
                })?;

            Ok(transaction)
        } else {
            // Relayer-paid fees: use the original flow
            let network_model = self
                .network_repository
                .get_by_name(NetworkType::Solana, &self.relayer.network)
                .await?
                .ok_or_else(|| {
                    RelayerError::NetworkConfiguration(format!(
                        "Network {} not found",
                        self.relayer.network
                    ))
                })?;

            let transaction = TransactionRepoModel::try_from((
                &network_transaction,
                &self.relayer,
                &network_model,
            ))?;

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

            // Queue status check job (with initial delay)
            self.job_producer
                .produce_check_transaction_status_job(
                    TransactionStatusCheck::new(
                        transaction.id.clone(),
                        transaction.relayer_id.clone(),
                        NetworkType::Solana,
                    ),
                    Some(calculate_scheduled_timestamp(
                        SOLANA_STATUS_CHECK_INITIAL_DELAY_SECONDS,
                    )),
                )
                .await?;

            Ok(transaction)
        }
    }

    async fn get_balance(&self) -> Result<BalanceResponse, RelayerError> {
        let address = &self.relayer.address;
        let balance = self.provider.get_balance(address).await?;

        Ok(BalanceResponse {
            balance: balance as u128,
            unit: SOLANA_SMALLEST_UNIT_NAME.to_string(),
        })
    }

    async fn delete_pending_transactions(
        &self,
    ) -> Result<DeletePendingTransactionsResponse, RelayerError> {
        Err(RelayerError::NotSupported(
            "Delete pending transactions not supported for Solana relayers".to_string(),
        ))
    }

    async fn sign_data(
        &self,
        _request: SignDataRequest,
    ) -> Result<crate::domain::relayer::SignDataResponse, RelayerError> {
        Err(RelayerError::NotSupported(
            "Sign data not supported for Solana relayers".to_string(),
        ))
    }

    async fn sign_typed_data(
        &self,
        _request: SignTypedDataRequest,
    ) -> Result<crate::domain::relayer::SignDataResponse, RelayerError> {
        Err(RelayerError::NotSupported(
            "Sign typed data not supported for Solana relayers".to_string(),
        ))
    }

    async fn sign_transaction(
        &self,
        request: &SignTransactionRequest,
    ) -> Result<SignTransactionExternalResponse, RelayerError> {
        let policy = self.relayer.policies.get_solana_policy();
        let user_pays_fee = matches!(
            policy.fee_payment_strategy.unwrap_or_default(),
            SolanaFeePaymentStrategy::User
        );

        // For user-paid fees, delegate to RPC handler (similar to process_transaction_request)
        if user_pays_fee {
            let solana_request = match request {
                SignTransactionRequest::Solana(req) => req,
                _ => {
                    error!(
                        id = %self.relayer.id,
                        "Invalid request type for Solana relayer",
                    );
                    return Err(RelayerError::NotSupported(
                        "Invalid request type for Solana relayer".to_string(),
                    ));
                }
            };

            let params = SolanaSignTransactionRequestParams {
                transaction: solana_request.transaction.clone(),
            };

            let result = self
                .rpc_handler
                .rpc_methods()
                .sign_transaction(params)
                .await
                .map_err(|e| RelayerError::Internal(e.to_string()))?;

            Ok(SignTransactionExternalResponse::Solana(
                SignTransactionResponseSolana {
                    transaction: result.transaction,
                    signature: result.signature,
                },
            ))
        } else {
            // Relayer-paid fees: use the original flow
            let transaction_bytes = match request {
                SignTransactionRequest::Solana(req) => &req.transaction,
                _ => {
                    error!(
                        id = %self.relayer.id,
                        "Invalid request type for Solana relayer",
                    );
                    return Err(RelayerError::NotSupported(
                        "Invalid request type for Solana relayer".to_string(),
                    ));
                }
            };

            // Prepare transaction data for signing
            let transaction_data = NetworkTransactionData::Solana(SolanaTransactionData {
                transaction: Some(transaction_bytes.clone().into_inner()),
                ..Default::default()
            });

            // Sign the transaction using the signer trait
            let response = self
                .signer
                .sign_transaction(transaction_data)
                .await
                .map_err(|e| {
                    error!(
                        %e,
                        id = %self.relayer.id,
                        "Failed to sign transaction",
                    );
                    RelayerError::SignerError(e)
                })?;

            // Extract Solana-specific response
            let solana_response = match response {
                SignTransactionResponse::Solana(resp) => resp,
                _ => {
                    return Err(RelayerError::ProviderError(
                        "Unexpected response type from Solana signer".to_string(),
                    ))
                }
            };

            Ok(SignTransactionExternalResponse::Solana(solana_response))
        }
    }

    async fn rpc(
        &self,
        request: JsonRpcRequest<NetworkRpcRequest>,
    ) -> Result<JsonRpcResponse<NetworkRpcResult>, RelayerError> {
        let JsonRpcRequest {
            jsonrpc: _,
            id,
            params,
        } = request;
        let solana_request = match params {
            NetworkRpcRequest::Solana(sol_req) => sol_req,
            _ => {
                return Ok(create_error_response(
                    id.clone(),
                    RpcErrorCodes::INVALID_PARAMS,
                    "Invalid params",
                    "Expected Solana network request",
                ))
            }
        };

        match solana_request {
            SolanaRpcRequest::RawRpcRequest { method, params } => {
                // Handle raw JSON-RPC requests by forwarding to provider
                let response = self.provider.raw_request_dyn(&method, params).await?;

                Ok(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: Some(NetworkRpcResult::Solana(SolanaRpcResult::RawRpc(response))),
                    error: None,
                    id: id.clone(),
                })
            }
            _ => {
                // Handle typed requests using the existing rpc_handler
                let response = self
                    .rpc_handler
                    .handle_request(JsonRpcRequest {
                        jsonrpc: request.jsonrpc,
                        params: NetworkRpcRequest::Solana(solana_request),
                        id: id.clone(),
                    })
                    .await;

                match response {
                    Ok(response) => Ok(response),
                    Err(e) => {
                        error!(error = %e, "error while processing RPC request");
                        let error_response = match e {
                            SolanaRpcError::UnsupportedMethod(msg) => {
                                JsonRpcResponse::error(32000, "UNSUPPORTED_METHOD", &msg)
                            }
                            SolanaRpcError::FeatureFetch(msg) => JsonRpcResponse::error(
                                -32008,
                                "FEATURE_FETCH_ERROR",
                                &format!("Failed to retrieve the list of enabled features: {msg}"),
                            ),
                            SolanaRpcError::InvalidParams(msg) => {
                                JsonRpcResponse::error(-32602, "INVALID_PARAMS", &msg)
                            }
                            SolanaRpcError::UnsupportedFeeToken(msg) => JsonRpcResponse::error(
                                -32000,
                                "UNSUPPORTED_FEE_TOKEN",
                                &format!(
                                    "The provided fee_token is not supported by the relayer: {msg}"
                                ),
                            ),
                            SolanaRpcError::Estimation(msg) => JsonRpcResponse::error(
                                -32001,
                                "ESTIMATION_ERROR",
                                &format!(
                                    "Failed to estimate the fee due to internal or network issues: {msg}"
                                ),
                            ),
                            SolanaRpcError::InsufficientFunds(msg) => {
                                // Trigger a token swap request if the relayer has insufficient funds
                                self.check_balance_and_trigger_token_swap_if_needed()
                                    .await?;

                                JsonRpcResponse::error(
                                    -32002,
                                    "INSUFFICIENT_FUNDS",
                                    &format!(
                                        "The sender does not have enough funds for the transfer: {msg}"
                                    ),
                                )
                            }
                            SolanaRpcError::TransactionPreparation(msg) => JsonRpcResponse::error(
                                -32003,
                                "TRANSACTION_PREPARATION_ERROR",
                                &format!("Failed to prepare the transfer transaction: {msg}"),
                            ),
                            SolanaRpcError::Preparation(msg) => JsonRpcResponse::error(
                                -32013,
                                "PREPARATION_ERROR",
                                &format!("Failed to prepare the transfer transaction: {msg}"),
                            ),
                            SolanaRpcError::Signature(msg) => JsonRpcResponse::error(
                                -32005,
                                "SIGNATURE_ERROR",
                                &format!("Failed to sign the transaction: {msg}"),
                            ),
                            SolanaRpcError::Signing(msg) => JsonRpcResponse::error(
                                -32005,
                                "SIGNATURE_ERROR",
                                &format!("Failed to sign the transaction: {msg}"),
                            ),
                            SolanaRpcError::TokenFetch(msg) => JsonRpcResponse::error(
                                -32007,
                                "TOKEN_FETCH_ERROR",
                                &format!("Failed to retrieve the list of supported tokens: {msg}"),
                            ),
                            SolanaRpcError::BadRequest(msg) => JsonRpcResponse::error(
                                -32007,
                                "BAD_REQUEST",
                                &format!("Bad request: {msg}"),
                            ),
                            SolanaRpcError::Send(msg) => JsonRpcResponse::error(
                                -32006,
                                "SEND_ERROR",
                                &format!(
                                    "Failed to submit the transaction to the blockchain: {msg}"
                                ),
                            ),
                            SolanaRpcError::SolanaTransactionValidation(msg) => JsonRpcResponse::error(
                                -32013,
                                "PREPARATION_ERROR",
                                &format!("Failed to prepare the transfer transaction: {msg}"),
                            ),
                            SolanaRpcError::Encoding(msg) => JsonRpcResponse::error(
                                -32601,
                                "INVALID_PARAMS",
                                &format!("The transaction parameter is invalid or missing: {msg}"),
                            ),
                            SolanaRpcError::TokenAccount(msg) => JsonRpcResponse::error(
                                -32601,
                                "PREPARATION_ERROR",
                                &format!("Invalid Token Account: {msg}"),
                            ),
                            SolanaRpcError::Token(msg) => JsonRpcResponse::error(
                                -32601,
                                "PREPARATION_ERROR",
                                &format!("Invalid Token Account: {msg}"),
                            ),
                            SolanaRpcError::Provider(msg) => JsonRpcResponse::error(
                                -32006,
                                "PREPARATION_ERROR",
                                &format!("Failed to prepare the transfer transaction: {msg}"),
                            ),
                            SolanaRpcError::Internal(_) => {
                                JsonRpcResponse::error(-32000, "INTERNAL_ERROR", "Internal error")
                            }
                        };
                        Ok(error_response)
                    }
                }
            }
        }
    }

    async fn get_status(&self) -> Result<RelayerStatus, RelayerError> {
        let address = &self.relayer.address;
        let balance = self.provider.get_balance(address).await?;

        let pending_statuses = [TransactionStatus::Pending, TransactionStatus::Submitted];
        let pending_transactions = self
            .transaction_repository
            .find_by_status(&self.relayer.id, &pending_statuses[..])
            .await
            .map_err(RelayerError::from)?;
        let pending_transactions_count = pending_transactions.len() as u64;

        let confirmed_statuses = [TransactionStatus::Confirmed];
        let confirmed_transactions = self
            .transaction_repository
            .find_by_status(&self.relayer.id, &confirmed_statuses[..])
            .await
            .map_err(RelayerError::from)?;

        let last_confirmed_transaction_timestamp = confirmed_transactions
            .iter()
            .filter_map(|tx| tx.confirmed_at.as_ref())
            .max()
            .cloned();

        Ok(RelayerStatus::Solana {
            balance: (balance as u128).to_string(),
            pending_transactions_count,
            last_confirmed_transaction_timestamp,
            system_disabled: self.relayer.system_disabled,
            paused: self.relayer.paused,
        })
    }

    async fn initialize_relayer(&self) -> Result<(), RelayerError> {
        debug!("initializing Solana relayer {}", self.relayer.id);

        // Populate model with allowed token metadata and update DB entry
        // Error will be thrown if any of the tokens are not found
        self.populate_allowed_tokens_metadata().await.map_err(|_| {
            RelayerError::PolicyConfigurationError(
                "Error while processing allowed tokens policy".into(),
            )
        })?;

        // Validate relayer allowed programs policy
        // Error will be thrown if any of the programs are not executable
        self.validate_program_policy().await.map_err(|_| {
            RelayerError::PolicyConfigurationError(
                "Error while validating allowed programs policy".into(),
            )
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
                    DisabledReason::RpcValidationFailed("Unknown error".to_string())
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

        self.check_balance_and_trigger_token_swap_if_needed()
            .await?;

        Ok(())
    }

    async fn check_health(&self) -> Result<(), Vec<HealthCheckFailure>> {
        debug!(
            "running health checks for Solana relayer {}",
            self.relayer.id
        );

        let validate_rpc_result = self.validate_rpc().await;
        let validate_min_balance_result = self.validate_min_balance().await;

        // Collect all failures
        let failures: Vec<HealthCheckFailure> = vec![
            validate_rpc_result
                .err()
                .map(|e| HealthCheckFailure::RpcValidationFailed(e.to_string())),
            validate_min_balance_result
                .err()
                .map(|e| HealthCheckFailure::BalanceCheckFailed(e.to_string())),
        ]
        .into_iter()
        .flatten()
        .collect();

        if failures.is_empty() {
            info!("all health checks passed");
            Ok(())
        } else {
            warn!("health checks failed: {:?}", failures);
            Err(failures)
        }
    }

    async fn validate_min_balance(&self) -> Result<(), RelayerError> {
        let balance = self
            .provider
            .get_balance(&self.relayer.address)
            .await
            .map_err(|e| RelayerError::ProviderError(e.to_string()))?;

        debug!(balance = %balance, "balance for relayer");

        let policy = self.relayer.policies.get_solana_policy();

        if balance < policy.min_balance.unwrap_or(DEFAULT_SOLANA_MIN_BALANCE) {
            return Err(RelayerError::InsufficientBalanceError(
                "Insufficient balance".to_string(),
            ));
        }

        Ok(())
    }
}

#[async_trait]
impl<RR, TR, J, S, JS, SP, NR> GasAbstractionTrait for SolanaRelayer<RR, TR, J, S, JS, SP, NR>
where
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: SolanaSignTrait + Signer + Send + Sync + 'static,
    JS: JupiterServiceTrait + Send + Sync + 'static,
    SP: SolanaProviderTrait + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
{
    async fn quote_sponsored_transaction(
        &self,
        params: SponsoredTransactionQuoteRequest,
    ) -> Result<SponsoredTransactionQuoteResponse, RelayerError> {
        let params = match params {
            SponsoredTransactionQuoteRequest::Solana(p) => p,
            _ => {
                return Err(RelayerError::ValidationError(
                    "Expected Solana fee estimate request parameters".to_string(),
                ));
            }
        };

        let result = self
            .rpc_handler
            .rpc_methods()
            .fee_estimate(params)
            .await
            .map_err(|e| RelayerError::Internal(e.to_string()))?;

        Ok(SponsoredTransactionQuoteResponse::Solana(result))
    }

    async fn build_sponsored_transaction(
        &self,
        params: SponsoredTransactionBuildRequest,
    ) -> Result<SponsoredTransactionBuildResponse, RelayerError> {
        let params = match params {
            SponsoredTransactionBuildRequest::Solana(p) => p,
            _ => {
                return Err(RelayerError::ValidationError(
                    "Expected Solana prepare transaction request parameters".to_string(),
                ));
            }
        };

        let result = self
            .rpc_handler
            .rpc_methods()
            .prepare_transaction(params)
            .await
            .map_err(|e| {
                let error_msg = format!("{e}");
                RelayerError::Internal(error_msg)
            })?;

        Ok(SponsoredTransactionBuildResponse::Solana(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{NetworkConfigCommon, SolanaNetworkConfig},
        domain::{
            create_network_dex_generic, Relayer, SignTransactionRequestSolana, SolanaRpcHandler,
            SolanaRpcMethodsImpl,
        },
        jobs::MockJobProducerTrait,
        models::{
            EncodedSerializedTransaction, JsonRpcId, NetworkConfigData, NetworkRepoModel,
            RelayerSolanaSwapConfig, SolanaAllowedTokensSwapConfig, SolanaFeeEstimateRequestParams,
            SolanaGetFeaturesEnabledRequestParams, SolanaRpcResult, SolanaSwapStrategy,
        },
        repositories::{MockNetworkRepository, MockRelayerRepository, MockTransactionRepository},
        services::{
            provider::{MockSolanaProviderTrait, SolanaProviderError},
            signer::MockSolanaSignTrait,
            MockJupiterServiceTrait, QuoteResponse, RoutePlan, SwapEvents, SwapInfo, SwapResponse,
            UltraExecuteResponse, UltraOrderResponse,
        },
        utils::mocks::mockutils::create_mock_solana_network,
    };
    use chrono::Utc;
    use mockall::predicate::*;
    use solana_sdk::{hash::Hash, program_pack::Pack, signature::Signature};
    use spl_token_interface::state::Account as SplAccount;

    /// Bundles all the pieces you need to instantiate a SolanaRelayer.
    /// Default::default gives you fresh mocks, but you can override any of them.
    #[allow(dead_code)]
    struct TestCtx {
        relayer_model: RelayerRepoModel,
        mock_repo: MockRelayerRepository,
        network_repository: Arc<MockNetworkRepository>,
        provider: Arc<MockSolanaProviderTrait>,
        signer: Arc<MockSolanaSignTrait>,
        jupiter: Arc<MockJupiterServiceTrait>,
        job_producer: Arc<MockJobProducerTrait>,
        tx_repo: Arc<MockTransactionRepository>,
        dex: Arc<NetworkDex<MockSolanaProviderTrait, MockSolanaSignTrait, MockJupiterServiceTrait>>,
        rpc_handler: SolanaRpcHandlerType<
            MockSolanaProviderTrait,
            MockSolanaSignTrait,
            MockJupiterServiceTrait,
            MockJobProducerTrait,
            MockTransactionRepository,
        >,
    }

    impl Default for TestCtx {
        fn default() -> Self {
            let mock_repo = MockRelayerRepository::new();
            let provider = Arc::new(MockSolanaProviderTrait::new());
            let signer = Arc::new(MockSolanaSignTrait::new());
            let jupiter = Arc::new(MockJupiterServiceTrait::new());
            let job = Arc::new(MockJobProducerTrait::new());
            let tx_repo = Arc::new(MockTransactionRepository::new());
            let mut network_repository = MockNetworkRepository::new();
            let transaction_repository = Arc::new(MockTransactionRepository::new());

            let relayer_model = RelayerRepoModel {
                id: "test-id".to_string(),
                address: "...".to_string(),
                network: "devnet".to_string(),
                ..Default::default()
            };

            let dex = Arc::new(
                create_network_dex_generic(
                    &relayer_model,
                    provider.clone(),
                    signer.clone(),
                    jupiter.clone(),
                )
                .unwrap(),
            );

            let test_network = create_mock_solana_network();

            let rpc_handler = Arc::new(SolanaRpcHandler::new(SolanaRpcMethodsImpl::new_mock(
                relayer_model.clone(),
                test_network.clone(),
                provider.clone(),
                signer.clone(),
                jupiter.clone(),
                job.clone(),
                transaction_repository.clone(),
            )));

            let test_network = NetworkRepoModel {
                id: "solana:devnet".to_string(),
                name: "devnet".to_string(),
                network_type: NetworkType::Solana,
                config: NetworkConfigData::Solana(SolanaNetworkConfig {
                    common: NetworkConfigCommon {
                        network: "devnet".to_string(),
                        from: None,
                        rpc_urls: Some(vec!["https://api.devnet.solana.com".to_string()]),
                        explorer_urls: None,
                        average_blocktime_ms: Some(400),
                        is_testnet: Some(true),
                        tags: None,
                    },
                }),
            };

            network_repository
                .expect_get_by_name()
                .returning(move |_, _| Ok(Some(test_network.clone())));

            TestCtx {
                relayer_model,
                mock_repo,
                network_repository: Arc::new(network_repository),
                provider,
                signer,
                jupiter,
                job_producer: job,
                tx_repo,
                dex,
                rpc_handler,
            }
        }
    }

    impl TestCtx {
        async fn into_relayer(
            self,
        ) -> SolanaRelayer<
            MockRelayerRepository,
            MockTransactionRepository,
            MockJobProducerTrait,
            MockSolanaSignTrait,
            MockJupiterServiceTrait,
            MockSolanaProviderTrait,
            MockNetworkRepository,
        > {
            // Get the network from the repository
            let network_repo = self
                .network_repository
                .get_by_name(NetworkType::Solana, "devnet")
                .await
                .unwrap()
                .unwrap();
            let network = SolanaNetwork::try_from(network_repo).unwrap();

            SolanaRelayer {
                relayer: self.relayer_model.clone(),
                signer: self.signer,
                network,
                provider: self.provider,
                rpc_handler: self.rpc_handler,
                relayer_repository: Arc::new(self.mock_repo),
                transaction_repository: self.tx_repo,
                job_producer: self.job_producer,
                dex_service: self.dex,
                network_repository: self.network_repository,
            }
        }
    }

    fn create_test_relayer() -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test-relayer-id".to_string(),
            address: "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin".to_string(),
            notification_id: Some("test-notification-id".to_string()),
            network_type: NetworkType::Solana,
            policies: RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
                min_balance: Some(0), // No minimum balance requirement
                swap_config: None,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn create_token_policy(
        mint: &str,
        min_amount: Option<u64>,
        max_amount: Option<u64>,
        retain_min: Option<u64>,
        slippage: Option<u64>,
    ) -> SolanaAllowedTokensPolicy {
        let mut token = SolanaAllowedTokensPolicy {
            mint: mint.to_string(),
            max_allowed_fee: Some(0),
            swap_config: None,
            decimals: Some(9),
            symbol: Some("SOL".to_string()),
        };

        let swap_config = SolanaAllowedTokensSwapConfig {
            min_amount,
            max_amount,
            retain_min_amount: retain_min,
            slippage_percentage: slippage.map(|s| s as f32),
        };

        token.swap_config = Some(swap_config);
        token
    }

    #[tokio::test]
    async fn test_calculate_swap_amount_no_limits() {
        let ctx = TestCtx::default();
        let solana_relayer = ctx.into_relayer().await;

        assert_eq!(
            solana_relayer
                .calculate_swap_amount(100, None, None, None)
                .unwrap(),
            100
        );
    }

    #[tokio::test]
    async fn test_calculate_swap_amount_with_max() {
        let ctx = TestCtx::default();
        let solana_relayer = ctx.into_relayer().await;

        assert_eq!(
            solana_relayer
                .calculate_swap_amount(100, None, Some(60), None)
                .unwrap(),
            60
        );
    }

    #[tokio::test]
    async fn test_calculate_swap_amount_with_retain() {
        let ctx = TestCtx::default();
        let solana_relayer = ctx.into_relayer().await;

        assert_eq!(
            solana_relayer
                .calculate_swap_amount(100, None, None, Some(30))
                .unwrap(),
            70
        );

        assert_eq!(
            solana_relayer
                .calculate_swap_amount(20, None, None, Some(30))
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn test_calculate_swap_amount_with_min() {
        let ctx = TestCtx::default();
        let solana_relayer = ctx.into_relayer().await;

        assert_eq!(
            solana_relayer
                .calculate_swap_amount(40, Some(50), None, None)
                .unwrap(),
            0
        );

        assert_eq!(
            solana_relayer
                .calculate_swap_amount(100, Some(50), None, None)
                .unwrap(),
            100
        );
    }

    #[tokio::test]
    async fn test_calculate_swap_amount_combined() {
        let ctx = TestCtx::default();
        let solana_relayer = ctx.into_relayer().await;

        assert_eq!(
            solana_relayer
                .calculate_swap_amount(100, None, Some(50), Some(30))
                .unwrap(),
            50
        );

        assert_eq!(
            solana_relayer
                .calculate_swap_amount(100, Some(20), Some(50), Some(30))
                .unwrap(),
            50
        );

        assert_eq!(
            solana_relayer
                .calculate_swap_amount(100, Some(60), Some(50), Some(30))
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn test_handle_token_swap_request_successful_swap_jupiter_swap_strategy() {
        let mut relayer_model = create_test_relayer();

        let mut mock_relayer_repo = MockRelayerRepository::new();
        let id = relayer_model.id.clone();

        relayer_model.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            swap_config: Some(RelayerSolanaSwapConfig {
                strategy: Some(SolanaSwapStrategy::JupiterSwap),
                cron_schedule: None,
                min_balance_threshold: None,
                jupiter_swap_options: None,
            }),
            allowed_tokens: Some(vec![create_token_policy(
                "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                Some(1),
                None,
                None,
                Some(50),
            )]),
            ..Default::default()
        });
        let cloned = relayer_model.clone();

        mock_relayer_repo
            .expect_get_by_id()
            .with(eq(id.clone()))
            .times(1)
            .returning(move |_| Ok(cloned.clone()));

        let mut raw_provider = MockSolanaProviderTrait::new();

        raw_provider
            .expect_get_account_from_pubkey()
            .returning(|_| {
                Box::pin(async {
                    let mut account_data = vec![0; SplAccount::LEN];

                    let token_account = spl_token_interface::state::Account {
                        mint: Pubkey::new_unique(),
                        owner: Pubkey::new_unique(),
                        amount: 10000000,
                        state: spl_token_interface::state::AccountState::Initialized,
                        ..Default::default()
                    };
                    spl_token_interface::state::Account::pack(token_account, &mut account_data)
                        .unwrap();

                    Ok(solana_sdk::account::Account {
                        lamports: 1_000_000,
                        data: account_data,
                        owner: spl_token_interface::id(),
                        executable: false,
                        rent_epoch: 0,
                    })
                })
            });

        let mut jupiter_mock = MockJupiterServiceTrait::new();

        jupiter_mock.expect_get_quote().returning(|_| {
            Box::pin(async {
                Ok(QuoteResponse {
                    input_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                    output_mint: WRAPPED_SOL_MINT.to_string(),
                    in_amount: 10,
                    out_amount: 10,
                    other_amount_threshold: 1,
                    swap_mode: "ExactIn".to_string(),
                    price_impact_pct: 0.0,
                    route_plan: vec![RoutePlan {
                        percent: 100,
                        swap_info: SwapInfo {
                            amm_key: "mock_amm_key".to_string(),
                            label: "mock_label".to_string(),
                            input_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                            output_mint: WRAPPED_SOL_MINT.to_string(),
                            in_amount: "1000".to_string(),
                            out_amount: "1000".to_string(),
                            fee_amount: "0".to_string(),
                            fee_mint: "mock_fee_mint".to_string(),
                        },
                    }],
                    slippage_bps: 0,
                })
            })
        });

        jupiter_mock.expect_get_swap_transaction().returning(|_| {
            Box::pin(async {
                Ok(SwapResponse {
                    swap_transaction: "AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAACAAQAKEZhsMunBegjHhwObzSrJeKhnl3sehIwqA8OCTejBJ/Z+O7sAR2gDS0+R1HXkqqjr0Wo3+auYeJQtq0il4DAumgiiHZpJZ1Uy9xq1yiOta3BcBOI7Dv+jmETs0W7Leny+AsVIwZWPN51bjn3Xk4uSzTFeAEom3HHY/EcBBpOfm7HkzWyukBvmNY5l9pnNxB/lTC52M7jy0Pxg6NhYJ37e1WXRYOFdoHOThs0hoFy/UG3+mVBbkR4sB9ywdKopv6IHO9+wuF/sV/02h9w+AjIBszK2bmCBPIrCZH4mqBdRcBFVAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABPS2wOQQj9KmokeOrgrMWdshu07fURwWLPYC0eDAkB+1Jh0UqsxbwO7GNdqHBaH3CjnuNams8L+PIsxs5JAZ16jJclj04kifG7PRApFI4NgwtaE5na/xCEBI572Nvp+FmsH4P9uc5VDeldVYzceVRhzPQ3SsaI7BOphAAiCnjaBgMGRm/lIRcy/+ytunLDm+e8jOW7xfcSayxDmzpAAAAAtD/6J/XX9kp0wJsfKVh53ksJqzbfyd1RSzIap7OM5ejnStls42Wf0xNRAChL93gEW4UQqPNOSYySLu5vwwX4aQR51VvyMcBu7nTFbs5oFQf9sbLeo/SOUQKxzaJWvBOPBt324ddloZPZy+FGzut5rBy0he1fWzeROoz1hX7/AKkGtJJ5s3DlXjsp517KoA8Lg71wC+tMHoDO9HDeQbotrwUMAAUCwFwVAAwACQOhzhsAAAAAAAoGAAQAIgcQAQEPOxAIAAUGAgQgIg8PDQ8hEg4JExEGARQUFAgQKAgmKgEDFhgXFSUnJCkQIywQIysIHSIqAh8DHhkbGhwLL8EgmzNB1pyBBwMAAAA6AWQAAU9kAQIvAABkAgNAQg8AAAAAAE3WYgAAAAAADwAAEAMEAAABCQMW8exZwhONJLLrrr9eKTOouI7XVrRLBjytPl3cL6rziwS+v7vCBB+8CQctooGHnRbQ3aoExfOLSH0uJhZijTPAKrJbYSJJ5hP1VwRmY2FlBkRkC2JtQsJRwDIR3Tbag/HLEdZxTPfqLWdCCyd0nco65bHdIoy/ByorMycoLzADMiYs".to_string(),
                    last_valid_block_height: 100,
                    prioritization_fee_lamports: None,
                    compute_unit_limit: None,
                    simulation_error: None,
                })
            })
        });

        let mut signer = MockSolanaSignTrait::new();
        let test_signature = Signature::from_str("2jg9xbGLtZRsiJBrDWQnz33JuLjDkiKSZuxZPdjJ3qrJbMeTEerXFAKynkPW63J88nq63cvosDNRsg9VqHtGixvP").unwrap();

        signer
            .expect_sign()
            .times(1)
            .returning(move |_| Box::pin(async move { Ok(test_signature) }));

        raw_provider
            .expect_send_versioned_transaction()
            .times(1)
            .returning(move |_| Box::pin(async move { Ok(test_signature) }));

        raw_provider
            .expect_confirm_transaction()
            .times(1)
            .returning(move |_| Box::pin(async move { Ok(true) }));

        let provider_arc = Arc::new(raw_provider);
        let jupiter_arc = Arc::new(jupiter_mock);
        let signer_arc = Arc::new(signer);

        let dex = Arc::new(
            create_network_dex_generic(
                &relayer_model,
                provider_arc.clone(),
                signer_arc.clone(),
                jupiter_arc.clone(),
            )
            .unwrap(),
        );

        let mut job_producer = MockJobProducerTrait::new();
        job_producer
            .expect_produce_send_notification_job()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let job_producer_arc = Arc::new(job_producer);

        let ctx = TestCtx {
            relayer_model,
            mock_repo: mock_relayer_repo,
            provider: provider_arc.clone(),
            jupiter: jupiter_arc.clone(),
            signer: signer_arc.clone(),
            dex,
            job_producer: job_producer_arc.clone(),
            ..Default::default()
        };
        let solana_relayer = ctx.into_relayer().await;
        let res = solana_relayer
            .handle_token_swap_request(create_test_relayer().id)
            .await
            .unwrap();
        assert_eq!(res.len(), 1);
        let swap = &res[0];
        assert_eq!(swap.source_amount, 10000000);
        assert_eq!(swap.destination_amount, 10);
        assert_eq!(swap.transaction_signature, test_signature.to_string());
    }

    #[tokio::test]
    async fn test_handle_token_swap_request_successful_swap_jupiter_ultra_strategy() {
        let mut relayer_model = create_test_relayer();

        let mut mock_relayer_repo = MockRelayerRepository::new();
        let id = relayer_model.id.clone();

        relayer_model.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            swap_config: Some(RelayerSolanaSwapConfig {
                strategy: Some(SolanaSwapStrategy::JupiterUltra),
                cron_schedule: None,
                min_balance_threshold: None,
                jupiter_swap_options: None,
            }),
            allowed_tokens: Some(vec![create_token_policy(
                "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                Some(1),
                None,
                None,
                Some(50),
            )]),
            ..Default::default()
        });
        let cloned = relayer_model.clone();

        mock_relayer_repo
            .expect_get_by_id()
            .with(eq(id.clone()))
            .times(1)
            .returning(move |_| Ok(cloned.clone()));

        let mut raw_provider = MockSolanaProviderTrait::new();

        raw_provider
            .expect_get_account_from_pubkey()
            .returning(|_| {
                Box::pin(async {
                    let mut account_data = vec![0; SplAccount::LEN];

                    let token_account = spl_token_interface::state::Account {
                        mint: Pubkey::new_unique(),
                        owner: Pubkey::new_unique(),
                        amount: 10000000,
                        state: spl_token_interface::state::AccountState::Initialized,
                        ..Default::default()
                    };
                    spl_token_interface::state::Account::pack(token_account, &mut account_data)
                        .unwrap();

                    Ok(solana_sdk::account::Account {
                        lamports: 1_000_000,
                        data: account_data,
                        owner: spl_token_interface::id(),
                        executable: false,
                        rent_epoch: 0,
                    })
                })
            });

        let mut jupiter_mock = MockJupiterServiceTrait::new();
        jupiter_mock.expect_get_ultra_order().returning(|_| {
            Box::pin(async {
                Ok(UltraOrderResponse {
                    transaction: Some("AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAACAAQAKEZhsMunBegjHhwObzSrJeKhnl3sehIwqA8OCTejBJ/Z+O7sAR2gDS0+R1HXkqqjr0Wo3+auYeJQtq0il4DAumgiiHZpJZ1Uy9xq1yiOta3BcBOI7Dv+jmETs0W7Leny+AsVIwZWPN51bjn3Xk4uSzTFeAEom3HHY/EcBBpOfm7HkzWyukBvmNY5l9pnNxB/lTC52M7jy0Pxg6NhYJ37e1WXRYOFdoHOThs0hoFy/UG3+mVBbkR4sB9ywdKopv6IHO9+wuF/sV/02h9w+AjIBszK2bmCBPIrCZH4mqBdRcBFVAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABPS2wOQQj9KmokeOrgrMWdshu07fURwWLPYC0eDAkB+1Jh0UqsxbwO7GNdqHBaH3CjnuNams8L+PIsxs5JAZ16jJclj04kifG7PRApFI4NgwtaE5na/xCEBI572Nvp+FmsH4P9uc5VDeldVYzceVRhzPQ3SsaI7BOphAAiCnjaBgMGRm/lIRcy/+ytunLDm+e8jOW7xfcSayxDmzpAAAAAtD/6J/XX9kp0wJsfKVh53ksJqzbfyd1RSzIap7OM5ejnStls42Wf0xNRAChL93gEW4UQqPNOSYySLu5vwwX4aQR51VvyMcBu7nTFbs5oFQf9sbLeo/SOUQKxzaJWvBOPBt324ddloZPZy+FGzut5rBy0he1fWzeROoz1hX7/AKkGtJJ5s3DlXjsp517KoA8Lg71wC+tMHoDO9HDeQbotrwUMAAUCwFwVAAwACQOhzhsAAAAAAAoGAAQAIgcQAQEPOxAIAAUGAgQgIg8PDQ8hEg4JExEGARQUFAgQKAgmKgEDFhgXFSUnJCkQIywQIysIHSIqAh8DHhkbGhwLL8EgmzNB1pyBBwMAAAA6AWQAAU9kAQIvAABkAgNAQg8AAAAAAE3WYgAAAAAADwAAEAMEAAABCQMW8exZwhONJLLrrr9eKTOouI7XVrRLBjytPl3cL6rziwS+v7vCBB+8CQctooGHnRbQ3aoExfOLSH0uJhZijTPAKrJbYSJJ5hP1VwRmY2FlBkRkC2JtQsJRwDIR3Tbag/HLEdZxTPfqLWdCCyd0nco65bHdIoy/ByorMycoLzADMiYs".to_string()),
                    input_mint: "PjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                    output_mint: WRAPPED_SOL_MINT.to_string(),
                    in_amount: 10,
                    out_amount: 10,
                    other_amount_threshold: 1,
                    swap_mode: "ExactIn".to_string(),
                    price_impact_pct: 0.0,
                    route_plan: vec![RoutePlan {
                        percent: 100,
                        swap_info: SwapInfo {
                            amm_key: "mock_amm_key".to_string(),
                            label: "mock_label".to_string(),
                            input_mint: "PjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                            output_mint: WRAPPED_SOL_MINT.to_string(),
                            in_amount: "1000".to_string(),
                            out_amount: "1000".to_string(),
                            fee_amount: "0".to_string(),
                            fee_mint: "mock_fee_mint".to_string(),
                        },
                    }],
                    prioritization_fee_lamports: 0,
                    request_id: "mock_request_id".to_string(),
                    slippage_bps: 0,
                })
            })
        });

        jupiter_mock.expect_execute_ultra_order().returning(|_| {
            Box::pin(async {
               Ok(UltraExecuteResponse {
                    signature: Some("2jg9xbGLtZRsiJBrDWQnz33JuLjDkiKSZuxZPdjJ3qrJbMeTEerXFAKynkPW63J88nq63cvosDNRsg9VqHtGixvP".to_string()),
                    status: "success".to_string(),
                    slot: Some("123456789".to_string()),
                    error: None,
                    code: 0,
                    total_input_amount: Some("1000000".to_string()),
                    total_output_amount: Some("1000000".to_string()),
                    input_amount_result: Some("1000000".to_string()),
                    output_amount_result: Some("1000000".to_string()),
                    swap_events: Some(vec![SwapEvents {
                        input_mint: "mock_input_mint".to_string(),
                        output_mint: "mock_output_mint".to_string(),
                        input_amount: "1000000".to_string(),
                        output_amount: "1000000".to_string(),
                    }]),
                })
            })
        });

        let mut signer = MockSolanaSignTrait::new();
        let test_signature = Signature::from_str("2jg9xbGLtZRsiJBrDWQnz33JuLjDkiKSZuxZPdjJ3qrJbMeTEerXFAKynkPW63J88nq63cvosDNRsg9VqHtGixvP").unwrap();

        signer
            .expect_sign()
            .times(1)
            .returning(move |_| Box::pin(async move { Ok(test_signature) }));

        let provider_arc = Arc::new(raw_provider);
        let jupiter_arc = Arc::new(jupiter_mock);
        let signer_arc = Arc::new(signer);

        let dex = Arc::new(
            create_network_dex_generic(
                &relayer_model,
                provider_arc.clone(),
                signer_arc.clone(),
                jupiter_arc.clone(),
            )
            .unwrap(),
        );

        let mut job_producer = MockJobProducerTrait::new();
        job_producer
            .expect_produce_send_notification_job()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let job_producer_arc = Arc::new(job_producer);

        let ctx = TestCtx {
            relayer_model,
            mock_repo: mock_relayer_repo,
            provider: provider_arc.clone(),
            jupiter: jupiter_arc.clone(),
            signer: signer_arc.clone(),
            dex,
            job_producer: job_producer_arc.clone(),
            ..Default::default()
        };
        let solana_relayer = ctx.into_relayer().await;

        let res = solana_relayer
            .handle_token_swap_request(create_test_relayer().id)
            .await
            .unwrap();
        assert_eq!(res.len(), 1);
        let swap = &res[0];
        assert_eq!(swap.source_amount, 10000000);
        assert_eq!(swap.destination_amount, 10);
        assert_eq!(swap.transaction_signature, test_signature.to_string());
    }

    #[tokio::test]
    async fn test_handle_token_swap_request_no_swap_config() {
        let mut relayer_model = create_test_relayer();

        let mut mock_relayer_repo = MockRelayerRepository::new();
        let id = relayer_model.id.clone();
        let cloned = relayer_model.clone();
        mock_relayer_repo
            .expect_get_by_id()
            .with(eq(id.clone()))
            .times(1)
            .returning(move |_| Ok(cloned.clone()));

        relayer_model.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            swap_config: Some(RelayerSolanaSwapConfig {
                strategy: Some(SolanaSwapStrategy::JupiterSwap),
                cron_schedule: None,
                min_balance_threshold: None,
                jupiter_swap_options: None,
            }),
            allowed_tokens: Some(vec![create_token_policy(
                "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                Some(1),
                None,
                None,
                Some(50),
            )]),
            ..Default::default()
        });
        let mut job_producer = MockJobProducerTrait::new();
        job_producer.expect_produce_send_notification_job().times(0);

        let job_producer_arc = Arc::new(job_producer);

        let ctx = TestCtx {
            relayer_model,
            mock_repo: mock_relayer_repo,
            job_producer: job_producer_arc,
            ..Default::default()
        };
        let solana_relayer = ctx.into_relayer().await;

        let res = solana_relayer.handle_token_swap_request(id).await;
        assert!(res.is_ok());
        assert!(res.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_handle_token_swap_request_no_strategy() {
        let mut relayer_model: RelayerRepoModel = create_test_relayer();

        let mut mock_relayer_repo = MockRelayerRepository::new();
        let id = relayer_model.id.clone();
        let cloned = relayer_model.clone();
        mock_relayer_repo
            .expect_get_by_id()
            .with(eq(id.clone()))
            .times(1)
            .returning(move |_| Ok(cloned.clone()));

        relayer_model.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            swap_config: Some(RelayerSolanaSwapConfig {
                strategy: None,
                cron_schedule: None,
                min_balance_threshold: Some(1),
                jupiter_swap_options: None,
            }),
            ..Default::default()
        });

        let ctx = TestCtx {
            relayer_model,
            mock_repo: mock_relayer_repo,
            ..Default::default()
        };
        let solana_relayer = ctx.into_relayer().await;

        let res = solana_relayer.handle_token_swap_request(id).await.unwrap();
        assert!(res.is_empty(), "should return empty when no strategy");
    }

    #[tokio::test]
    async fn test_handle_token_swap_request_no_allowed_tokens() {
        let mut relayer_model: RelayerRepoModel = create_test_relayer();
        let mut mock_relayer_repo = MockRelayerRepository::new();
        let id = relayer_model.id.clone();
        let cloned = relayer_model.clone();
        mock_relayer_repo
            .expect_get_by_id()
            .with(eq(id.clone()))
            .times(1)
            .returning(move |_| Ok(cloned.clone()));

        relayer_model.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            swap_config: Some(RelayerSolanaSwapConfig {
                strategy: Some(SolanaSwapStrategy::JupiterSwap),
                cron_schedule: None,
                min_balance_threshold: Some(1),
                jupiter_swap_options: None,
            }),
            allowed_tokens: None,
            ..Default::default()
        });

        let ctx = TestCtx {
            relayer_model,
            mock_repo: mock_relayer_repo,
            ..Default::default()
        };
        let solana_relayer = ctx.into_relayer().await;

        let res = solana_relayer.handle_token_swap_request(id).await.unwrap();
        assert!(res.is_empty(), "should return empty when no allowed_tokens");
    }

    #[tokio::test]
    async fn test_validate_rpc_success() {
        let mut raw_provider = MockSolanaProviderTrait::new();
        raw_provider
            .expect_get_latest_blockhash()
            .times(1)
            .returning(|| Box::pin(async { Ok(Hash::new_unique()) }));

        let ctx = TestCtx {
            provider: Arc::new(raw_provider),
            ..Default::default()
        };
        let solana_relayer = ctx.into_relayer().await;
        let res = solana_relayer.validate_rpc().await;

        assert!(
            res.is_ok(),
            "validate_rpc should succeed when blockhash fetch succeeds"
        );
    }

    #[tokio::test]
    async fn test_validate_rpc_provider_error() {
        let mut raw_provider = MockSolanaProviderTrait::new();
        raw_provider
            .expect_get_latest_blockhash()
            .times(1)
            .returning(|| {
                Box::pin(async { Err(SolanaProviderError::RpcError("rpc failure".to_string())) })
            });

        let ctx = TestCtx {
            provider: Arc::new(raw_provider),
            ..Default::default()
        };

        let solana_relayer = ctx.into_relayer().await;
        let err = solana_relayer.validate_rpc().await.unwrap_err();

        match err {
            RelayerError::ProviderError(msg) => {
                assert!(msg.contains("rpc failure"));
            }
            other => panic!("expected ProviderError, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_check_balance_no_swap_config() {
        // default ctx has no swap_config
        let ctx = TestCtx::default();
        let solana_relayer = ctx.into_relayer().await;

        // should do nothing and succeed
        assert!(solana_relayer
            .check_balance_and_trigger_token_swap_if_needed()
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn test_check_balance_no_threshold() {
        // override policy to have a swap_config with no min_balance_threshold
        let mut ctx = TestCtx::default();
        let mut model = ctx.relayer_model.clone();
        model.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            swap_config: Some(RelayerSolanaSwapConfig {
                strategy: Some(SolanaSwapStrategy::JupiterSwap),
                cron_schedule: None,
                min_balance_threshold: None,
                jupiter_swap_options: None,
            }),
            ..Default::default()
        });
        ctx.relayer_model = model;
        let solana_relayer = ctx.into_relayer().await;

        assert!(solana_relayer
            .check_balance_and_trigger_token_swap_if_needed()
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn test_check_balance_above_threshold() {
        let mut raw_provider = MockSolanaProviderTrait::new();
        raw_provider
            .expect_get_balance()
            .times(1)
            .returning(|_| Box::pin(async { Ok(20_u64) }));
        let provider = Arc::new(raw_provider);
        let mut raw_job = MockJobProducerTrait::new();
        raw_job
            .expect_produce_token_swap_request_job()
            .withf(move |req, _opts| req.relayer_id == "test-id")
            .times(0);
        let job_producer = Arc::new(raw_job);

        let ctx = TestCtx {
            provider,
            job_producer,
            ..Default::default()
        };
        // set threshold to 10
        let mut model = ctx.relayer_model.clone();
        model.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            swap_config: Some(RelayerSolanaSwapConfig {
                strategy: Some(SolanaSwapStrategy::JupiterSwap),
                cron_schedule: None,
                min_balance_threshold: Some(10),
                jupiter_swap_options: None,
            }),
            ..Default::default()
        });
        let mut ctx = ctx;
        ctx.relayer_model = model;

        let solana_relayer = ctx.into_relayer().await;
        assert!(solana_relayer
            .check_balance_and_trigger_token_swap_if_needed()
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn test_check_balance_below_threshold_triggers_job() {
        let mut raw_provider = MockSolanaProviderTrait::new();
        raw_provider
            .expect_get_balance()
            .times(1)
            .returning(|_| Box::pin(async { Ok(5_u64) }));

        let mut raw_job = MockJobProducerTrait::new();
        raw_job
            .expect_produce_token_swap_request_job()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));
        let job_producer = Arc::new(raw_job);

        let mut model = create_test_relayer();
        model.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            swap_config: Some(RelayerSolanaSwapConfig {
                strategy: Some(SolanaSwapStrategy::JupiterSwap),
                cron_schedule: None,
                min_balance_threshold: Some(10),
                jupiter_swap_options: None,
            }),
            ..Default::default()
        });

        let ctx = TestCtx {
            relayer_model: model,
            provider: Arc::new(raw_provider),
            job_producer,
            ..Default::default()
        };

        let solana_relayer = ctx.into_relayer().await;
        assert!(solana_relayer
            .check_balance_and_trigger_token_swap_if_needed()
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn test_get_balance_success() {
        let mut raw_provider = MockSolanaProviderTrait::new();
        raw_provider
            .expect_get_balance()
            .times(1)
            .returning(|_| Box::pin(async { Ok(42_u64) }));
        let ctx = TestCtx {
            provider: Arc::new(raw_provider),
            ..Default::default()
        };
        let solana_relayer = ctx.into_relayer().await;

        let res = solana_relayer.get_balance().await.unwrap();

        assert_eq!(res.balance, 42_u128);
        assert_eq!(res.unit, SOLANA_SMALLEST_UNIT_NAME);
    }

    #[tokio::test]
    async fn test_get_balance_provider_error() {
        let mut raw_provider = MockSolanaProviderTrait::new();
        raw_provider
            .expect_get_balance()
            .times(1)
            .returning(|_| Box::pin(async { Err(SolanaProviderError::RpcError("oops".into())) }));
        let ctx = TestCtx {
            provider: Arc::new(raw_provider),
            ..Default::default()
        };
        let solana_relayer = ctx.into_relayer().await;

        let err = solana_relayer.get_balance().await.unwrap_err();

        match err {
            RelayerError::UnderlyingSolanaProvider(err) => {
                assert!(err.to_string().contains("oops"));
            }
            other => panic!("expected ProviderError, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_validate_min_balance_success() {
        let mut raw_provider = MockSolanaProviderTrait::new();
        raw_provider
            .expect_get_balance()
            .times(1)
            .returning(|_| Box::pin(async { Ok(100_u64) }));

        let mut model = create_test_relayer();
        model.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            min_balance: Some(50),
            ..Default::default()
        });

        let ctx = TestCtx {
            relayer_model: model,
            provider: Arc::new(raw_provider),
            ..Default::default()
        };

        let solana_relayer = ctx.into_relayer().await;
        assert!(solana_relayer.validate_min_balance().await.is_ok());
    }

    #[tokio::test]
    async fn test_validate_min_balance_insufficient() {
        let mut raw_provider = MockSolanaProviderTrait::new();
        raw_provider
            .expect_get_balance()
            .times(1)
            .returning(|_| Box::pin(async { Ok(10_u64) }));

        let mut model = create_test_relayer();
        model.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            min_balance: Some(50),
            ..Default::default()
        });

        let ctx = TestCtx {
            relayer_model: model,
            provider: Arc::new(raw_provider),
            ..Default::default()
        };

        let solana_relayer = ctx.into_relayer().await;
        let err = solana_relayer.validate_min_balance().await.unwrap_err();
        match err {
            RelayerError::InsufficientBalanceError(msg) => {
                assert_eq!(msg, "Insufficient balance");
            }
            other => panic!("expected InsufficientBalanceError, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_validate_min_balance_provider_error() {
        let mut raw_provider = MockSolanaProviderTrait::new();
        raw_provider
            .expect_get_balance()
            .times(1)
            .returning(|_| Box::pin(async { Err(SolanaProviderError::RpcError("fail".into())) }));
        let ctx = TestCtx {
            provider: Arc::new(raw_provider),
            ..Default::default()
        };

        let solana_relayer = ctx.into_relayer().await;
        let err = solana_relayer.validate_min_balance().await.unwrap_err();
        match err {
            RelayerError::ProviderError(msg) => {
                assert!(msg.contains("fail"));
            }
            other => panic!("expected ProviderError, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_rpc_invalid_params() {
        let ctx = TestCtx::default();
        let solana_relayer = ctx.into_relayer().await;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: NetworkRpcRequest::Solana(crate::models::SolanaRpcRequest::FeeEstimate(
                SolanaFeeEstimateRequestParams {
                    transaction: EncodedSerializedTransaction::new("".to_string()),
                    fee_token: "".to_string(),
                },
            )),
            id: Some(JsonRpcId::Number(1)),
        };
        let resp = solana_relayer.rpc(req).await.unwrap();

        assert!(resp.error.is_some(), "expected an error object");
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "INVALID_PARAMS");
    }

    #[tokio::test]
    async fn test_rpc_success() {
        let ctx = TestCtx::default();
        let solana_relayer = ctx.into_relayer().await;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: NetworkRpcRequest::Solana(crate::models::SolanaRpcRequest::GetFeaturesEnabled(
                SolanaGetFeaturesEnabledRequestParams {},
            )),
            id: Some(JsonRpcId::Number(1)),
        };
        let resp = solana_relayer.rpc(req).await.unwrap();

        assert!(resp.error.is_none(), "error should be None");
        let data = resp.result.unwrap();
        let sol_res = match data {
            NetworkRpcResult::Solana(inner) => inner,
            other => panic!("expected Solana, got {:?}", other),
        };
        let features = match sol_res {
            SolanaRpcResult::GetFeaturesEnabled(f) => f,
            other => panic!("expected GetFeaturesEnabled, got {:?}", other),
        };
        assert_eq!(features.features, vec!["gasless".to_string()]);
    }

    #[tokio::test]
    async fn test_initialize_relayer_disables_when_validation_fails() {
        let mut raw_provider = MockSolanaProviderTrait::new();
        let mut mock_repo = MockRelayerRepository::new();
        let mut job_producer = MockJobProducerTrait::new();

        let mut relayer_model = create_test_relayer();
        relayer_model.system_disabled = false; // Start as enabled
        relayer_model.notification_id = Some("test-notification-id".to_string());

        // Mock validation failure - RPC validation fails
        raw_provider.expect_get_latest_blockhash().returning(|| {
            Box::pin(async { Err(SolanaProviderError::RpcError("RPC error".to_string())) })
        });

        raw_provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1000000u64) })); // Sufficient balance

        // Mock disable_relayer call
        let mut disabled_relayer = relayer_model.clone();
        disabled_relayer.system_disabled = true;
        mock_repo
            .expect_disable_relayer()
            .with(eq("test-relayer-id".to_string()), always())
            .returning(move |_, _| Ok(disabled_relayer.clone()));

        // Mock notification job production
        job_producer
            .expect_produce_send_notification_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        // Mock health check job scheduling
        job_producer
            .expect_produce_relayer_health_check_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let ctx = TestCtx {
            relayer_model,
            mock_repo,
            provider: Arc::new(raw_provider),
            job_producer: Arc::new(job_producer),
            ..Default::default()
        };

        let solana_relayer = ctx.into_relayer().await;
        let result = solana_relayer.initialize_relayer().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_initialize_relayer_enables_when_validation_passes_and_was_disabled() {
        let mut raw_provider = MockSolanaProviderTrait::new();
        let mut mock_repo = MockRelayerRepository::new();

        let mut relayer_model = create_test_relayer();
        relayer_model.system_disabled = true; // Start as disabled

        // Mock successful validations
        raw_provider
            .expect_get_latest_blockhash()
            .returning(|| Box::pin(async { Ok(Hash::new_unique()) }));

        raw_provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1000000u64) })); // Sufficient balance

        // Mock enable_relayer call
        let mut enabled_relayer = relayer_model.clone();
        enabled_relayer.system_disabled = false;
        mock_repo
            .expect_enable_relayer()
            .with(eq("test-relayer-id".to_string()))
            .returning(move |_| Ok(enabled_relayer.clone()));

        // Mock any potential disable_relayer calls (even though they shouldn't happen)
        let mut disabled_relayer = relayer_model.clone();
        disabled_relayer.system_disabled = true;
        mock_repo
            .expect_disable_relayer()
            .returning(move |_, _| Ok(disabled_relayer.clone()));

        let ctx = TestCtx {
            relayer_model,
            mock_repo,
            provider: Arc::new(raw_provider),
            ..Default::default()
        };

        let solana_relayer = ctx.into_relayer().await;
        let result = solana_relayer.initialize_relayer().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_initialize_relayer_no_action_when_enabled_and_validation_passes() {
        let mut raw_provider = MockSolanaProviderTrait::new();
        let mock_repo = MockRelayerRepository::new();

        let mut relayer_model = create_test_relayer();
        relayer_model.system_disabled = false; // Start as enabled

        // Mock successful validations
        raw_provider
            .expect_get_latest_blockhash()
            .returning(|| Box::pin(async { Ok(Hash::new_unique()) }));

        raw_provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1000000u64) })); // Sufficient balance

        let ctx = TestCtx {
            relayer_model,
            mock_repo,
            provider: Arc::new(raw_provider),
            ..Default::default()
        };

        let solana_relayer = ctx.into_relayer().await;
        let result = solana_relayer.initialize_relayer().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_initialize_relayer_sends_notification_when_disabled() {
        let mut raw_provider = MockSolanaProviderTrait::new();
        let mut mock_repo = MockRelayerRepository::new();
        let mut job_producer = MockJobProducerTrait::new();

        let mut relayer_model = create_test_relayer();
        relayer_model.system_disabled = false; // Start as enabled
        relayer_model.notification_id = Some("test-notification-id".to_string());

        // Mock validation failure - balance check fails
        raw_provider
            .expect_get_latest_blockhash()
            .returning(|| Box::pin(async { Ok(Hash::new_unique()) }));

        raw_provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(100u64) })); // Insufficient balance

        // Mock disable_relayer call
        let mut disabled_relayer = relayer_model.clone();
        disabled_relayer.system_disabled = true;
        mock_repo
            .expect_disable_relayer()
            .with(eq("test-relayer-id".to_string()), always())
            .returning(move |_, _| Ok(disabled_relayer.clone()));

        // Mock notification job production - verify it's called
        job_producer
            .expect_produce_send_notification_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        // Mock health check job scheduling
        job_producer
            .expect_produce_relayer_health_check_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let ctx = TestCtx {
            relayer_model,
            mock_repo,
            provider: Arc::new(raw_provider),
            job_producer: Arc::new(job_producer),
            ..Default::default()
        };

        let solana_relayer = ctx.into_relayer().await;
        let result = solana_relayer.initialize_relayer().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_initialize_relayer_no_notification_when_no_notification_id() {
        let mut raw_provider = MockSolanaProviderTrait::new();
        let mut mock_repo = MockRelayerRepository::new();

        let mut relayer_model = create_test_relayer();
        relayer_model.system_disabled = false; // Start as enabled
        relayer_model.notification_id = None; // No notification ID

        // Mock validation failure - RPC validation fails
        raw_provider.expect_get_latest_blockhash().returning(|| {
            Box::pin(async {
                Err(SolanaProviderError::RpcError(
                    "RPC validation failed".to_string(),
                ))
            })
        });

        raw_provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1000000u64) })); // Sufficient balance

        // Mock disable_relayer call
        let mut disabled_relayer = relayer_model.clone();
        disabled_relayer.system_disabled = true;
        mock_repo
            .expect_disable_relayer()
            .with(eq("test-relayer-id".to_string()), always())
            .returning(move |_, _| Ok(disabled_relayer.clone()));

        // No notification job should be produced since notification_id is None
        // But health check job should still be scheduled
        let mut job_producer = MockJobProducerTrait::new();
        job_producer
            .expect_produce_relayer_health_check_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let ctx = TestCtx {
            relayer_model,
            mock_repo,
            provider: Arc::new(raw_provider),
            job_producer: Arc::new(job_producer),
            ..Default::default()
        };

        let solana_relayer = ctx.into_relayer().await;
        let result = solana_relayer.initialize_relayer().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_initialize_relayer_policy_validation_fails() {
        let mut raw_provider = MockSolanaProviderTrait::new();

        let mut relayer_model = create_test_relayer();
        relayer_model.system_disabled = false;

        // Set up a policy that will cause validation to fail
        relayer_model.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: "InvalidMintAddress".to_string(),
                decimals: Some(9),
                symbol: Some("INVALID".to_string()),
                max_allowed_fee: Some(0),
                swap_config: None,
            }]),
            ..Default::default()
        });

        // Mock provider calls that might be made during token validation
        raw_provider
            .expect_get_token_metadata_from_pubkey()
            .returning(|_| {
                Box::pin(async {
                    Err(SolanaProviderError::RpcError("Token not found".to_string()))
                })
            });

        let ctx = TestCtx {
            relayer_model,
            provider: Arc::new(raw_provider),
            ..Default::default()
        };

        let solana_relayer = ctx.into_relayer().await;
        let result = solana_relayer.initialize_relayer().await;

        // Should fail due to policy validation error
        assert!(result.is_err());
        match result.unwrap_err() {
            RelayerError::PolicyConfigurationError(msg) => {
                assert!(msg.contains("Error while processing allowed tokens policy"));
            }
            other => panic!("Expected PolicyConfigurationError, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_success() {
        let signer = MockSolanaSignTrait::new();

        let relayer_model = RelayerRepoModel {
            id: "test-relayer-id".to_string(),
            address: "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin".to_string(),
            network: "devnet".to_string(),
            policies: RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
                fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
                min_balance: Some(0),
                ..Default::default()
            }),
            ..Default::default()
        };

        let ctx = TestCtx {
            relayer_model,
            signer: Arc::new(signer),
            ..Default::default()
        };

        let solana_relayer = ctx.into_relayer().await;

        let sign_request = SignTransactionRequest::Solana(SignTransactionRequestSolana {
            transaction: EncodedSerializedTransaction::new("raw_transaction_data".to_string()),
        });

        let result = solana_relayer.sign_transaction(&sign_request).await;
        assert!(result.is_ok());
        let response = result.unwrap();
        match response {
            SignTransactionExternalResponse::Solana(solana_resp) => {
                assert_eq!(
                    solana_resp.transaction.into_inner(),
                    "signed_transaction_data"
                );
                assert_eq!(solana_resp.signature, "signature_data");
            }
            _ => panic!("Expected Solana response"),
        }
    }

    #[tokio::test]
    async fn test_get_status_success() {
        let mut raw_provider = MockSolanaProviderTrait::new();
        let mut tx_repo = MockTransactionRepository::new();

        // Mock balance retrieval
        raw_provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1000000) }));

        // Mock transaction counts
        tx_repo
            .expect_find_by_status()
            .with(
                eq("test-id"),
                eq(vec![
                    TransactionStatus::Pending,
                    TransactionStatus::Submitted,
                ]),
            )
            .returning(|_, _| {
                Ok(vec![
                    TransactionRepoModel::default(),
                    TransactionRepoModel::default(),
                ])
            });

        // Mock recent confirmed transaction
        let recent_tx = TransactionRepoModel {
            id: "recent-tx".to_string(),
            relayer_id: "test-id".to_string(),
            network_data: NetworkTransactionData::Solana(SolanaTransactionData::default()),
            network_type: NetworkType::Solana,
            status: TransactionStatus::Confirmed,
            confirmed_at: Some(Utc::now().to_string()),
            ..Default::default()
        };
        tx_repo
            .expect_find_by_status()
            .with(eq("test-id"), eq(vec![TransactionStatus::Confirmed]))
            .returning(move |_, _| Ok(vec![recent_tx.clone()]));

        let ctx = TestCtx {
            tx_repo: Arc::new(tx_repo),
            provider: Arc::new(raw_provider),
            ..Default::default()
        };

        let solana_relayer = ctx.into_relayer().await;

        let result = solana_relayer.get_status().await;
        assert!(result.is_ok());
        let status = result.unwrap();

        match status {
            RelayerStatus::Solana {
                balance,
                pending_transactions_count,
                last_confirmed_transaction_timestamp,
                ..
            } => {
                assert_eq!(balance, "1000000");
                assert_eq!(pending_transactions_count, 2);
                assert!(last_confirmed_transaction_timestamp.is_some());
            }
            _ => panic!("Expected Solana status"),
        }
    }

    #[tokio::test]
    async fn test_get_status_balance_error() {
        let mut raw_provider = MockSolanaProviderTrait::new();
        let tx_repo = MockTransactionRepository::new();

        // Mock balance error
        raw_provider.expect_get_balance().returning(|_| {
            Box::pin(async { Err(SolanaProviderError::RpcError("RPC error".to_string())) })
        });

        let ctx = TestCtx {
            tx_repo: Arc::new(tx_repo),
            provider: Arc::new(raw_provider),
            ..Default::default()
        };

        let solana_relayer = ctx.into_relayer().await;

        let result = solana_relayer.get_status().await;
        assert!(result.is_err());
        match result.unwrap_err() {
            RelayerError::UnderlyingSolanaProvider(err) => {
                assert!(err.to_string().contains("RPC error"));
            }
            other => panic!("Expected UnderlyingSolanaProvider, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_get_status_no_recent_transactions() {
        let mut raw_provider = MockSolanaProviderTrait::new();
        let mut tx_repo = MockTransactionRepository::new();

        // Mock balance retrieval
        raw_provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(500000) }));

        // Mock transaction counts
        tx_repo
            .expect_find_by_status()
            .with(
                eq("test-id"),
                eq(vec![
                    TransactionStatus::Pending,
                    TransactionStatus::Submitted,
                ]),
            )
            .returning(|_, _| Ok(vec![]));

        tx_repo
            .expect_find_by_status()
            .with(eq("test-id"), eq(vec![TransactionStatus::Confirmed]))
            .returning(|_, _| Ok(vec![]));

        let ctx = TestCtx {
            tx_repo: Arc::new(tx_repo),
            provider: Arc::new(raw_provider),
            ..Default::default()
        };

        let solana_relayer = ctx.into_relayer().await;

        let result = solana_relayer.get_status().await;
        assert!(result.is_ok());
        let status = result.unwrap();

        match status {
            RelayerStatus::Solana {
                balance,
                pending_transactions_count,
                last_confirmed_transaction_timestamp,
                ..
            } => {
                assert_eq!(balance, "500000");
                assert_eq!(pending_transactions_count, 0);
                assert!(last_confirmed_transaction_timestamp.is_none());
            }
            _ => panic!("Expected Solana status"),
        }
    }

    // GasAbstractionTrait tests
    // These are passthrough methods to RPC handlers, so we verify:
    // 1. Wrong network type returns ValidationError
    // The actual RPC handler functionality (including method calls) is tested in the RPC handler tests
    // Note: We can't easily mock the RPC handler here due to type constraints in TestCtx,
    // but the passthrough behavior is verified through the RPC handler tests.

    #[tokio::test]
    async fn test_quote_sponsored_transaction_wrong_network() {
        let ctx = TestCtx::default();
        let solana_relayer = ctx.into_relayer().await;

        // Use Stellar request instead of Solana
        let request = SponsoredTransactionQuoteRequest::Stellar(
            crate::models::StellarFeeEstimateRequestParams {
                transaction_xdr: Some("test-xdr".to_string()),
                operations: None,
                source_account: None,
                fee_token: "native".to_string(),
            },
        );

        let result = solana_relayer.quote_sponsored_transaction(request).await;
        assert!(result.is_err());

        if let Err(RelayerError::ValidationError(msg)) = result {
            assert!(msg.contains("Expected Solana fee estimate request parameters"));
        } else {
            panic!("Expected ValidationError for wrong network type");
        }
    }

    #[tokio::test]
    async fn test_build_sponsored_transaction_wrong_network() {
        let ctx = TestCtx::default();
        let solana_relayer = ctx.into_relayer().await;

        // Use Stellar request instead of Solana
        let request = SponsoredTransactionBuildRequest::Stellar(
            crate::models::StellarPrepareTransactionRequestParams {
                transaction_xdr: Some("test-xdr".to_string()),
                operations: None,
                source_account: None,
                fee_token: "native".to_string(),
            },
        );

        let result = solana_relayer.build_sponsored_transaction(request).await;
        assert!(result.is_err());

        if let Err(RelayerError::ValidationError(msg)) = result {
            assert!(msg.contains("Expected Solana prepare transaction request parameters"));
        } else {
            panic!("Expected ValidationError for wrong network type");
        }
    }
}
