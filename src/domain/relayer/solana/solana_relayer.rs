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
    constants::{DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE, SOLANA_SMALLEST_UNIT_NAME},
    domain::{
        relayer::RelayerError, BalanceResponse, DexStrategy, JsonRpcRequest, JsonRpcResponse,
        SolanaRelayerTrait, SwapParams,
    },
    jobs::{JobProducer, JobProducerTrait, SolanaTokenSwapRequest},
    models::{
        produce_relayer_disabled_payload, NetworkRpcRequest, NetworkRpcResult,
        RelayerNetworkPolicy, RelayerRepoModel, RelayerSolanaPolicy, SolanaAllowedTokensPolicy,
        SolanaNetwork,
    },
    repositories::{
        InMemoryRelayerRepository, InMemoryTransactionRepository, RelayerRepository,
        RelayerRepositoryStorage, Repository,
    },
    services::{SolanaProvider, SolanaProviderTrait, SolanaSigner},
};
use async_trait::async_trait;
use eyre::Result;
use futures::future::try_join_all;
use log::{error, info, warn};
use solana_sdk::{account::Account, pubkey::Pubkey};

use super::{
    NetworkDex, SolanaRpcError, SolanaRpcHandler, SolanaRpcMethodsImpl, SolanaTokenProgram,
    TokenAccount,
};

struct TokenSwapCandidate<'a> {
    policy: &'a SolanaAllowedTokensPolicy,
    account: TokenAccount,
    swap_amount: u64,
}

#[allow(dead_code)]
pub struct SolanaRelayer {
    relayer: RelayerRepoModel,
    signer: Arc<SolanaSigner>,
    network: SolanaNetwork,
    provider: Arc<SolanaProvider>,
    rpc_handler: Arc<SolanaRpcHandler<SolanaRpcMethodsImpl>>,
    relayer_repository: Arc<RelayerRepositoryStorage<InMemoryRelayerRepository>>,
    transaction_repository: Arc<InMemoryTransactionRepository>,
    job_producer: Arc<JobProducer>,
    dex_service: Arc<NetworkDex>,
}

impl SolanaRelayer {
    pub fn new(
        relayer: RelayerRepoModel,
        signer: Arc<SolanaSigner>,
        relayer_repository: Arc<RelayerRepositoryStorage<InMemoryRelayerRepository>>,
        provider: Arc<SolanaProvider>,
        rpc_handler: Arc<SolanaRpcHandler<SolanaRpcMethodsImpl>>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
        job_producer: Arc<JobProducer>,
        dex_service: Arc<NetworkDex>,
    ) -> Result<Self, RelayerError> {
        let network = match SolanaNetwork::from_network_str(&relayer.network) {
            Ok(network) => network,
            Err(e) => return Err(RelayerError::NetworkConfiguration(e.to_string())),
        };

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
            Ok::<SolanaAllowedTokensPolicy, RelayerError>(SolanaAllowedTokensPolicy::new(
                token_metadata.mint,
                Some(token_metadata.decimals),
                Some(token_metadata.symbol.to_string()),
                token.max_allowed_fee,
                token.swap_config.clone(),
            ))
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

    async fn should_execute_token_swap(&self, token_policy: &SolanaAllowedTokensPolicy) -> bool {
        // Check if swap config exists
        match token_policy.swap_config.as_ref() {
            Some(_) => return true,
            None => {
                info!(
                    "No swap configuration specified for token: {}",
                    token_policy.mint
                );
                return false;
            }
        };
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

    pub async fn handle_token_swap_request(&self, relayer_id: String) -> Result<(), RelayerError> {
        info!("Handling token swap request for relayer: {}", relayer_id);
        let relayer = self
            .relayer_repository
            .get_by_id(relayer_id.clone())
            .await?;

        let policy = relayer.policies.get_solana_policy();
        let swap_config = match policy.get_swap_config() {
            Some(config) => config,
            None => {
                info!("No swap configuration specified; skipping validation.");
                return Ok(());
            }
        };

        let swap_strategy = match swap_config.strategy {
            Some(strategy) => strategy,
            None => {
                info!("No swap strategy specified; skipping validation.");
                return Ok(());
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
                        info!(
                            "Token swap eligible for token: {}. Current balance: {}, min amount: {:?}, max amount: {:?}, retain min: {:?}",
                            token.mint,
                            token_account.amount,
                            token.swap_config.as_ref().and_then(|config| config.min_amount),
                            token.swap_config.as_ref().and_then(|config| config.max_amount),
                            token.swap_config.as_ref().and_then(|config| config.retain_min_amount)
                        );
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
            let provider = &self.provider;
            let signer = &self.signer;
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
                        destination_mint: "So11111111111111111111111111111111111111112".to_string(), // SOL mint
                        amount: swap_amount,
                        slippage_percent,
                    })
                    .await?;

                info!(
                    "Successfully swapped {} tokens of type {} to {} SOL for relayer {}",
                    swap_amount, token_mint, swap_result.destination_amount, relayer_id_clone
                );

                Ok::<_, RelayerError>(swap_result)
            }
        });
        // Wait for all swaps to complete
        let swap_results = try_join_all(swap_futures).await?;

        // Log summary of swaps
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
        }

        Ok(())
    }
}

#[async_trait]
impl SolanaRelayerTrait for SolanaRelayer {
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

        if balance < policy.min_balance {
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
mod tests {}
