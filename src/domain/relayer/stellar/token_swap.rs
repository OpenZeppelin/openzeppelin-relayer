//! Token swap implementation for Stellar relayers.
//!
//! This module implements the `StellarRelayerDexTrait` for Stellar relayers, providing
//! token swap functionality for managing relayer token balances.

use async_trait::async_trait;
use futures::future::try_join_all;
use tracing::{debug, error, info};

use crate::constants::DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE;
use crate::domain::relayer::{
    Relayer, RelayerError, StellarRelayer, StellarRelayerDexTrait, SwapResult,
};
use crate::domain::transaction::stellar::token::get_token_balance;
use crate::jobs::JobProducerTrait;
use crate::models::transaction::request::StellarTransactionRequest;
use crate::models::{
    produce_stellar_dex_webhook_payload, NetworkTransactionRequest, RelayerRepoModel,
    StellarDexPayload,
};
use crate::models::{NetworkRepoModel, TransactionRepoModel};
use crate::repositories::{
    NetworkRepository, RelayerRepository, Repository, TransactionRepository,
};
use crate::services::provider::StellarProviderTrait;
use crate::services::signer::StellarSignTrait;
use crate::services::stellar_dex::{StellarDexServiceTrait, SwapTransactionParams};
use crate::services::TransactionCounterServiceTrait;

#[async_trait]
impl<P, RR, NR, TR, J, TCS, S, D> StellarRelayerDexTrait
    for StellarRelayer<P, RR, NR, TR, J, TCS, S, D>
where
    P: StellarProviderTrait + Send + Sync,
    D: StellarDexServiceTrait + Send + Sync + 'static,
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

        let strategies = &swap_config.strategies;
        if strategies.is_empty() {
            debug!(%relayer_id, "No swap strategies specified for relayer; Exiting.");
            return Ok(vec![]);
        }

        // Check XLM balance
        let account_entry = self
            .provider
            .get_account(&relayer.address)
            .await
            .map_err(|e| RelayerError::ProviderError(format!("Failed to get account: {e}")))?;

        // Convert balance from i64 to u64 for comparison (Stellar balances are i64 but always positive)
        let xlm_balance = if account_entry.balance < 0 {
            return Err(RelayerError::ProviderError(
                "Account balance is negative".to_string(),
            ));
        } else {
            account_entry.balance as u64
        };

        // Check if XLM balance is below threshold (if threshold is configured)
        if let Some(threshold) = swap_config.min_balance_threshold {
            if xlm_balance > threshold {
                debug!(
                    %relayer_id,
                    balance = xlm_balance,
                    threshold = threshold,
                    "XLM balance above threshold, skipping token swap"
                );
                return Ok(vec![]);
            }

            info!(
                %relayer_id,
                balance = xlm_balance,
                threshold = threshold,
                "XLM balance below threshold, checking tokens for swap"
            );
        } else {
            info!(
                %relayer_id,
                balance = xlm_balance,
                "Checking tokens for swap; current XLM balance"
            );
        }

        // Get allowed tokens and calculate swap amounts
        let tokens_to_swap = {
            let mut eligible_tokens = Vec::new();

            let allowed_tokens = policy.get_allowed_tokens();
            if allowed_tokens.is_empty() {
                debug!(%relayer_id, "No allowed tokens configured for swap");
                return Ok(vec![]);
            }

            for token in &allowed_tokens {
                // Fetch token balance
                let token_balance =
                    get_token_balance(&self.provider, &relayer.address, &token.asset)
                        .await
                        .map_err(|e| {
                            RelayerError::ProviderError(format!(
                                "Failed to get token balance for {}: {}",
                                token.asset, e
                            ))
                        })?;

                // Calculate swap amount based on configuration
                let swap_amount = calculate_swap_amount(
                    token_balance,
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
                    debug!(%relayer_id, token = ?token.asset, "token swap eligible for token");

                    // Store token asset and swap amount (clone necessary data)
                    eligible_tokens.push((
                        token.asset.clone(),
                        swap_amount,
                        token
                            .swap_config
                            .as_ref()
                            .and_then(|config| config.slippage_percentage)
                            .unwrap_or(DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE),
                    ));
                }
            }

            eligible_tokens
        };
        let network_passphrase = self.network.passphrase.clone();
        let relayer_network = relayer.network.clone();

        // Prepare swap transactions for every eligible token
        // Transactions are queued for background processing through the gate mechanism
        // Sequence numbers will be managed by the transaction pipeline during preparation
        // This ensures swaps don't conflict with other transactions in the pipeline
        // The strategy router will automatically select the appropriate DEX service
        // based on asset type and configured strategies
        let swap_prep_futures: Vec<_> = tokens_to_swap
            .iter()
            .filter_map(|(token_asset, swap_amount, slippage_percent)| {
                // Check if any configured strategy can handle this asset
                if !self.dex_service.can_handle_asset(token_asset) {
                    debug!(
                        %relayer_id,
                        token = ?token_asset,
                        "Skipping token swap - no configured strategy can handle this asset type"
                    );
                    return None;
                }

                let token_asset = token_asset.clone();
                let dex_service = self.dex_service.clone();
                let relayer_address = relayer.address.clone();
                let relayer_id_clone = relayer_id.clone();
                let slippage_percent = *slippage_percent;
                let network_passphrase = network_passphrase.clone();
                let token_decimals = policy.get_allowed_token_decimals(&token_asset);

                Some(async move {
                    info!(
                        "Preparing swap transaction for {} tokens of type {} for relayer: {}",
                        swap_amount, token_asset, relayer_id_clone
                    );

                    // Prepare swap transaction parameters
                    // Note: Sequence number is not set here - it will be managed by the transaction pipeline
                    // when the transaction goes through the prepare phase via the gate mechanism
                    let swap_params = SwapTransactionParams {
                        source_account: relayer_address.clone(),
                        source_asset: token_asset.clone(),
                        destination_asset: "native".to_string(), // Always swap to XLM
                        amount: *swap_amount,
                        slippage_percent,
                        network_passphrase: network_passphrase.clone(),
                        source_asset_decimals: token_decimals,
                        destination_asset_decimals: Some(7), // XLM always has 7 decimals
                    };

                    // Prepare swap transaction (get quote and build XDR) without executing
                    // The transaction will be queued for background processing through the gate mechanism
                    dex_service
                        .prepare_swap_transaction(swap_params)
                        .await
                        .map(|(xdr, quote)| (token_asset, *swap_amount, quote, xdr))
                        .map_err(|e| {
                            RelayerError::Internal(format!(
                                "Failed to prepare swap transaction: {e}"
                            ))
                        })
                })
            })
            .collect();

        // Prepare all swap transactions concurrently
        let swap_prep_results = try_join_all(swap_prep_futures).await?;

        // Queue each prepared swap transaction for background processing
        // This ensures swaps go through the same gate mechanism as regular transactions
        let mut swap_results = Vec::new();
        for (token_asset, swap_amount, quote, xdr) in swap_prep_results {
            // Create transaction request and queue for background processing
            let stellar_request = StellarTransactionRequest {
                source_account: Some(relayer.address.clone()),
                network: relayer_network.clone(),
                operations: None,
                memo: None,
                valid_until: None,
                transaction_xdr: Some(xdr),
                fee_bump: None,
                max_fee: None,
            };

            let network_request = NetworkTransactionRequest::Stellar(stellar_request);

            // Queue the swap transaction for background processing
            // This will go through the gate mechanism and be processed by the transaction handler
            match self.process_transaction_request(network_request).await {
                Ok(transaction_model) => {
                    info!(
                        "Swap transaction queued for relayer: {}. Token: {}, Amount: {}, Destination: {}, Transaction ID: {}",
                        relayer_id, token_asset, swap_amount, quote.out_amount, transaction_model.id
                    );

                    swap_results.push(SwapResult {
                        mint: token_asset,
                        source_amount: swap_amount,
                        destination_amount: quote.out_amount,
                        transaction_signature: transaction_model.id, // Use transaction ID instead of hash
                        error: None,
                    });
                }
                Err(e) => {
                    error!(
                        "Error queueing swap transaction for relayer: {}. Token: {}, Error: {}",
                        relayer_id, token_asset, e
                    );
                    swap_results.push(SwapResult {
                        mint: token_asset,
                        source_amount: swap_amount,
                        destination_amount: 0,
                        transaction_signature: "".to_string(),
                        error: Some(format!("Failed to queue transaction: {e}")),
                    });
                }
            }
        }

        if !swap_results.is_empty() {
            let queued_count = swap_results
                .iter()
                .filter(|result| result.error.is_none())
                .count();
            let failed_count = swap_results.len() - queued_count;

            info!(
                "Queued {} swap transactions for relayer {} ({} successful, {} failed). \
                 Each transaction will send its own status notification when processed.",
                swap_results.len(),
                relayer_id,
                queued_count,
                failed_count
            );

            // Send notification with transaction IDs for tracking queued swaps
            // Transaction IDs are included in SwapResult.transaction_signature field
            // This allows users to track which transactions were queued
            // Each transaction will also send its own status notification when processed
            if let Some(notification_id) = &relayer.notification_id {
                // Only send notification if we have at least one successfully queued swap
                let has_queued_swaps = swap_results.iter().any(|result| {
                    result.error.is_none() && !result.transaction_signature.is_empty()
                });

                if has_queued_swaps {
                    let webhook_result = self
                        .job_producer
                        .produce_send_notification_job(
                            produce_stellar_dex_webhook_payload(
                                notification_id,
                                "stellar_dex_queued".to_string(),
                                StellarDexPayload {
                                    swap_results: swap_results.clone(),
                                },
                            ),
                            None,
                        )
                        .await;

                    if let Err(e) = webhook_result {
                        error!(error = %e, "failed to produce swap queued notification job");
                    }
                }
            }
        }

        Ok(swap_results)
    }
}

/// Calculate swap amount based on current balance and swap configuration
///
/// This function determines how much of a token should be swapped based on:
/// - Maximum swap amount (caps the swap)
/// - Retain minimum amount (ensures minimum balance is retained)
/// - Minimum swap amount (ensures swap meets minimum requirement)
///
/// Returns 0 if swap should not be performed (e.g., balance too low, below minimum)
fn calculate_swap_amount(
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
