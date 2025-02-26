//! # Solana Relayer Module
//!
//! This module implements a relayer for the Solana network. It defines a trait
//! `SolanaRelayerTrait` for common operations such as sending JSON RPC requests,
//! fetching balance information, signing transactions, etc. The module uses a
//! SolanaProvider for making RPC calls.
//!
//! It integrates with other parts of the system including the job queue ([`JobProducer`]),
//! in-memory repositories, and the application's domain models.
use std::sync::Arc;

use crate::{
    constants::SOLANA_SMALLEST_UNIT_NAME,
    domain::{
        relayer::RelayerError, BalanceResponse, JsonRpcRequest, JsonRpcResponse, SolanaRelayerTrait,
    },
    jobs::{JobProducer, JobProducerTrait},
    models::{
        produce_relayer_disabled_payload, RelayerNetworkPolicy, RelayerRepoModel,
        RelayerSolanaPolicy, SolanaAllowedTokensPolicy, SolanaNetwork,
    },
    repositories::{InMemoryTransactionRepository, RelayerRepository, RelayerRepositoryStorage},
    services::{SolanaProvider, SolanaProviderTrait, SolanaSigner},
};
use async_trait::async_trait;
use eyre::Result;
use futures::future::try_join_all;
use log::{error, info, warn};
use solana_sdk::account::Account;

use super::{SolanaRpcError, SolanaRpcHandler, SolanaRpcMethodsImpl};

#[allow(dead_code)]
pub struct SolanaRelayer {
    relayer: RelayerRepoModel,
    signer: Arc<SolanaSigner>,
    network: SolanaNetwork,
    provider: Arc<SolanaProvider>,
    rpc_handler: Arc<SolanaRpcHandler<SolanaRpcMethodsImpl>>,
    relayer_repository: Arc<RelayerRepositoryStorage>,
    transaction_repository: Arc<InMemoryTransactionRepository>,
    job_producer: Arc<JobProducer>,
}

impl SolanaRelayer {
    pub fn new(
        relayer: RelayerRepoModel,
        signer: Arc<SolanaSigner>,
        relayer_repository: Arc<RelayerRepositoryStorage>,
        provider: Arc<SolanaProvider>,
        rpc_handler: Arc<SolanaRpcHandler<SolanaRpcMethodsImpl>>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
        job_producer: Arc<JobProducer>,
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
                token.conversion_slippage_percentage,
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

    async fn rpc(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse, RelayerError> {
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
                    SolanaRpcError::InsufficientFunds(msg) => JsonRpcResponse::error(
                        -32002,
                        "INSUFFICIENT_FUNDS",
                        &format!(
                            "The sender does not have enough funds for the transfer: {}",
                            msg
                        ),
                    ),
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {}
