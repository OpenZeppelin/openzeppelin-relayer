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

use crate::{
    constants::{
        DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE, DEFAULT_SOLANA_MIN_BALANCE,
        SOLANA_SMALLEST_UNIT_NAME, WRAPPED_SOL_MINT,
    },
    domain::{
        relayer::RelayerError, BalanceResponse, DexStrategy, SolanaRelayerDexTrait,
        SolanaRelayerTrait, SwapParams,
    },
    jobs::{JobProducerTrait, SolanaTokenSwapRequest},
    models::{
        produce_relayer_disabled_payload, produce_solana_dex_webhook_payload, JsonRpcRequest,
        JsonRpcResponse, NetworkRepoModel, NetworkRpcRequest, NetworkRpcResult, NetworkType,
        RelayerNetworkPolicy, RelayerRepoModel, RelayerSolanaPolicy, SolanaAllowedTokensPolicy,
        SolanaDexPayload, SolanaNetwork, TransactionRepoModel,
    },
    repositories::{NetworkRepository, RelayerRepository, Repository},
    services::{
        JupiterService, JupiterServiceTrait, SolanaProvider, SolanaProviderTrait, SolanaSignTrait,
        SolanaSigner,
    },
};
use async_trait::async_trait;
use eyre::Result;
use futures::future::try_join_all;
use log::{error, info, warn};
use solana_sdk::{account::Account, pubkey::Pubkey};

use super::{
    NetworkDex, SolanaRpcError, SolanaRpcHandler, SolanaRpcMethodsImpl, SolanaTokenProgram,
    SwapResult, TokenAccount,
};

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
    TR: Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: SolanaSignTrait + Send + Sync + 'static,
    JS: JupiterServiceTrait + Send + Sync + 'static,
    SP: SolanaProviderTrait + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
{
    relayer: RelayerRepoModel,
    signer: Arc<S>,
    network: SolanaNetwork,
    provider: Arc<SP>,
    rpc_handler: Arc<SolanaRpcHandler<SolanaRpcMethodsImpl<SP, S, JS, J>>>,
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
    TR: Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: SolanaSignTrait + Send + Sync + 'static,
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
        rpc_handler: Arc<SolanaRpcHandler<SolanaRpcMethodsImpl<SP, S, JS, J>>>,
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
                .produce_solana_token_swap_request_job(
                    SolanaTokenSwapRequest {
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
    TR: Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: SolanaSignTrait + Send + Sync + 'static,
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
        info!("Handling token swap request for relayer: {}", relayer_id);
        let relayer = self
            .relayer_repository
            .get_by_id(relayer_id.clone())
            .await?;

        let policy = relayer.policies.get_solana_policy();

        let swap_config = match policy.get_swap_config() {
            Some(config) => config,
            None => {
                info!("No swap configuration specified; Exiting.");
                return Ok(vec![]);
            }
        };

        match swap_config.strategy {
            Some(strategy) => strategy,
            None => {
                info!("No swap strategy specified; Exiting.");
                return Ok(vec![]);
            }
        };

        let relayer_pubkey = Pubkey::from_str(&relayer.address)
            .map_err(|e| RelayerError::ProviderError(format!("Invalid relayer address: {}", e)))?;

        let tokens_to_swap = {
            let mut eligible_tokens = Vec::<TokenSwapCandidate>::new();

            if let Some(allowed_tokens) = policy.allowed_tokens.as_ref() {
                for token in allowed_tokens {
                    let token_mint = Pubkey::from_str(&token.mint).map_err(|e| {
                        RelayerError::ProviderError(format!("Invalid token mint: {}", e))
                    })?;
                    let token_account = SolanaTokenProgram::get_and_unpack_token_account(
                        &*self.provider,
                        &relayer_pubkey,
                        &token_mint,
                    )
                    .await
                    .map_err(|e| {
                        RelayerError::ProviderError(format!("Failed to get token account: {}", e))
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
                        info!("Token swap eligible for token: {:?}", token);

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
                    error!("Failed to produce notification job: {}", e);
                }
            }
        }

        Ok(swap_results)
    }
}

#[async_trait]
impl<RR, TR, J, S, JS, SP, NR> SolanaRelayerTrait for SolanaRelayer<RR, TR, J, S, JS, SP, NR>
where
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    TR: Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: SolanaSignTrait + Send + Sync + 'static,
    JS: JupiterServiceTrait + Send + Sync + 'static,
    SP: SolanaProviderTrait + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
{
    async fn get_balance(&self) -> Result<BalanceResponse, RelayerError> {
        let address = &self.relayer.address;
        let balance = self.provider.get_balance(address).await?;

        Ok(BalanceResponse {
            balance: balance as u128,
            unit: SOLANA_SMALLEST_UNIT_NAME.to_string(),
        })
    }

    async fn rpc(
        &self,
        request: JsonRpcRequest<NetworkRpcRequest>,
    ) -> Result<JsonRpcResponse<NetworkRpcResult>, RelayerError> {
        let response = self.rpc_handler.handle_request(request).await;

        match response {
            Ok(response) => Ok(response),
            Err(e) => {
                error!("Error while processing RPC request: {}", e);
                let error_response = match e {
                    SolanaRpcError::UnsupportedMethod(msg) => {
                        JsonRpcResponse::error(32000, "UNSUPPORTED_METHOD", &msg)
                    }
                    SolanaRpcError::FeatureFetch(msg) => JsonRpcResponse::error(
                        -32008,
                        "FEATURE_FETCH_ERROR",
                        &format!("Failed to retrieve the list of enabled features: {}", msg),
                    ),
                    SolanaRpcError::InvalidParams(msg) => {
                        JsonRpcResponse::error(-32602, "INVALID_PARAMS", &msg)
                    }
                    SolanaRpcError::UnsupportedFeeToken(msg) => JsonRpcResponse::error(
                        -32000,
                        "UNSUPPORTED
                        FEE_TOKEN",
                        &format!(
                            "The provided fee_token is not supported by the relayer: {}",
                            msg
                        ),
                    ),
                    SolanaRpcError::Estimation(msg) => JsonRpcResponse::error(
                        -32001,
                        "ESTIMATION_ERROR",
                        &format!(
                            "Failed to estimate the fee due to internal or network issues: {}",
                            msg
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
                                "The sender does not have enough funds for the transfer: {}",
                                msg
                            ),
                        )
                    }
                    SolanaRpcError::TransactionPreparation(msg) => JsonRpcResponse::error(
                        -32003,
                        "TRANSACTION_PREPARATION_ERROR",
                        &format!("Failed to prepare the transfer transaction: {}", msg),
                    ),
                    SolanaRpcError::Preparation(msg) => JsonRpcResponse::error(
                        -32013,
                        "PREPARATION_ERROR",
                        &format!("Failed to prepare the transfer transaction: {}", msg),
                    ),
                    SolanaRpcError::Signature(msg) => JsonRpcResponse::error(
                        -32005,
                        "SIGNATURE_ERROR",
                        &format!("Failed to sign the transaction: {}", msg),
                    ),
                    SolanaRpcError::Signing(msg) => JsonRpcResponse::error(
                        -32005,
                        "SIGNATURE_ERROR",
                        &format!("Failed to sign the transaction: {}", msg),
                    ),
                    SolanaRpcError::TokenFetch(msg) => JsonRpcResponse::error(
                        -32007,
                        "TOKEN_FETCH_ERROR",
                        &format!("Failed to retrieve the list of supported tokens: {}", msg),
                    ),
                    SolanaRpcError::BadRequest(msg) => JsonRpcResponse::error(
                        -32007,
                        "BAD_REQUEST",
                        &format!("Bad request: {}", msg),
                    ),
                    SolanaRpcError::Send(msg) => JsonRpcResponse::error(
                        -32006,
                        "SEND_ERROR",
                        &format!(
                            "Failed to submit the transaction to the blockchain: {}",
                            msg
                        ),
                    ),
                    SolanaRpcError::SolanaTransactionValidation(msg) => JsonRpcResponse::error(
                        -32013,
                        "PREPARATION_ERROR",
                        &format!("Failed to prepare the transfer transaction: {}", msg),
                    ),
                    SolanaRpcError::Encoding(msg) => JsonRpcResponse::error(
                        -32601,
                        "INVALID_PARAMS",
                        &format!("The transaction parameter is invalid or missing: {}", msg),
                    ),
                    SolanaRpcError::TokenAccount(msg) => JsonRpcResponse::error(
                        -32601,
                        "PREPARATION_ERROR",
                        &format!("Invalid Token Account: {}", msg),
                    ),
                    SolanaRpcError::Token(msg) => JsonRpcResponse::error(
                        -32601,
                        "PREPARATION_ERROR",
                        &format!("Invalid Token Account: {}", msg),
                    ),
                    SolanaRpcError::Provider(msg) => JsonRpcResponse::error(
                        -32006,
                        "PREPARATION_ERROR",
                        &format!("Failed to prepare the transfer transaction: {}", msg),
                    ),
                    SolanaRpcError::Internal(_) => {
                        JsonRpcResponse::error(-32000, "INTERNAL_ERROR", "Internal error")
                    }
                };
                Ok(error_response)
            }
        }
    }

    async fn validate_min_balance(&self) -> Result<(), RelayerError> {
        let balance = self
            .provider
            .get_balance(&self.relayer.address)
            .await
            .map_err(|e| RelayerError::ProviderError(e.to_string()))?;

        info!("Balance : {} for relayer: {}", balance, self.relayer.id);

        let policy = self.relayer.policies.get_solana_policy();

        if balance < policy.min_balance.unwrap_or(DEFAULT_SOLANA_MIN_BALANCE) {
            return Err(RelayerError::InsufficientBalanceError(
                "Insufficient balance".to_string(),
            ));
        }

        Ok(())
    }

    async fn initialize_relayer(&self) -> Result<(), RelayerError> {
        info!("Initializing relayer: {}", self.relayer.id);

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

        let validate_rpc_result = self.validate_rpc().await;

        let validate_min_balance_result = self.validate_min_balance().await;

        // disable relayer if any check fails
        if validate_rpc_result.is_err() || validate_min_balance_result.is_err() {
            let reason = vec![
                validate_rpc_result
                    .err()
                    .map(|e| format!("RPC validation failed: {}", e)),
                validate_min_balance_result
                    .err()
                    .map(|e| format!("Balance check failed: {}", e)),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<String>>()
            .join(", ");

            warn!("Disabling relayer: {} due to: {}", self.relayer.id, reason);
            let updated_relayer = self
                .relayer_repository
                .disable_relayer(self.relayer.id.clone())
                .await?;
            if let Some(notification_id) = &self.relayer.notification_id {
                self.job_producer
                    .produce_send_notification_job(
                        produce_relayer_disabled_payload(
                            notification_id,
                            &updated_relayer,
                            &reason,
                        ),
                        None,
                    )
                    .await?;
            }
        }

        self.check_balance_and_trigger_token_swap_if_needed()
            .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{NetworkConfigCommon, SolanaNetworkConfig},
        domain::create_network_dex_generic,
        jobs::MockJobProducerTrait,
        models::{
            EncodedSerializedTransaction, FeeEstimateRequestParams,
            GetFeaturesEnabledRequestParams, JsonRpcId, NetworkConfigData, NetworkRepoModel,
            RelayerSolanaSwapConfig, SolanaAllowedTokensSwapConfig, SolanaRpcResult,
            SolanaSwapStrategy,
        },
        repositories::{MockNetworkRepository, MockRelayerRepository, MockTransactionRepository},
        services::{
            MockJupiterServiceTrait, MockSolanaProviderTrait, MockSolanaSignTrait, QuoteResponse,
            RoutePlan, SolanaProviderError, SwapEvents, SwapInfo, SwapResponse,
            UltraExecuteResponse, UltraOrderResponse,
        },
    };
    use mockall::predicate::*;
    use solana_sdk::{hash::Hash, program_pack::Pack, signature::Signature};
    use spl_token::state::Account as SplAccount;

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
        rpc_handler: Arc<
            SolanaRpcHandler<
                SolanaRpcMethodsImpl<
                    MockSolanaProviderTrait,
                    MockSolanaSignTrait,
                    MockJupiterServiceTrait,
                    MockJobProducerTrait,
                >,
            >,
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

            let rpc_handler = Arc::new(SolanaRpcHandler::new(SolanaRpcMethodsImpl::new_mock(
                relayer_model.clone(),
                provider.clone(),
                signer.clone(),
                jupiter.clone(),
                job.clone(),
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

                    let token_account = spl_token::state::Account {
                        mint: Pubkey::new_unique(),
                        owner: Pubkey::new_unique(),
                        amount: 10000000,
                        state: spl_token::state::AccountState::Initialized,
                        ..Default::default()
                    };
                    spl_token::state::Account::pack(token_account, &mut account_data).unwrap();

                    Ok(solana_sdk::account::Account {
                        lamports: 1_000_000,
                        data: account_data,
                        owner: spl_token::id(),
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

                    let token_account = spl_token::state::Account {
                        mint: Pubkey::new_unique(),
                        owner: Pubkey::new_unique(),
                        amount: 10000000,
                        state: spl_token::state::AccountState::Initialized,
                        ..Default::default()
                    };
                    spl_token::state::Account::pack(token_account, &mut account_data).unwrap();

                    Ok(solana_sdk::account::Account {
                        lamports: 1_000_000,
                        data: account_data,
                        owner: spl_token::id(),
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
            .expect_produce_solana_token_swap_request_job()
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
            .expect_produce_solana_token_swap_request_job()
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
                FeeEstimateRequestParams {
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
                GetFeaturesEnabledRequestParams {},
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
}
