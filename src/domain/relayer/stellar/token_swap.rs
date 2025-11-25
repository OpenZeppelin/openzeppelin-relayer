//! Token swap implementation for Stellar relayers.
//!
//! This module implements the `StellarRelayerDexTrait` for Stellar relayers, providing
//! token swap functionality for managing relayer token balances.

use async_trait::async_trait;
use futures::future::join_all;
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
    StellarDexPayload, StellarFeePaymentStrategy,
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
    /// 2. Iterates allowed tokens, checking balances and calculating swap amounts.
    /// 3. Executes swaps through the DEX service (Paths service).
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

        let policy = relayer.policies.get_stellar_policy();

        // Token swaps are only supported for user fee payment strategy
        // This ensures swaps are only performed when users pay fees in tokens
        if !matches!(
            policy.fee_payment_strategy,
            Some(StellarFeePaymentStrategy::User)
        ) {
            debug!(
                %relayer_id,
                "Token swap is only supported for user fee payment strategy; Exiting."
            );
            return Ok(vec![]);
        }

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

        // Get allowed tokens and calculate swap amounts
        let tokens_to_swap = {
            let mut eligible_tokens = Vec::new();

            let allowed_tokens = policy.get_allowed_tokens();
            if allowed_tokens.is_empty() {
                debug!(%relayer_id, "No allowed tokens configured for swap");
                return Ok(vec![]);
            }

            for token in &allowed_tokens {
                // Fetch token balance - continue on error for individual tokens
                let token_balance =
                    match get_token_balance(&self.provider, &relayer.address, &token.asset).await {
                        Ok(balance) => balance,
                        Err(e) => {
                            error!(
                                %relayer_id,
                                token = %token.asset,
                                error = %e,
                                "Failed to get token balance, skipping this token"
                            );
                            continue;
                        }
                    };

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
                let swap_amount_clone = *swap_amount;

                Some(async move {
                    info!(
                        "Preparing swap transaction for {} tokens of type {} for relayer: {}",
                        swap_amount_clone, token_asset, relayer_id_clone
                    );

                    // Prepare swap transaction parameters
                    // Note: Sequence number is not set here - it will be managed by the transaction pipeline
                    // when the transaction goes through the prepare phase via the gate mechanism
                    let swap_params = SwapTransactionParams {
                        source_account: relayer_address.clone(),
                        source_asset: token_asset.clone(),
                        destination_asset: "native".to_string(), // Always swap to XLM
                        amount: swap_amount_clone,
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
                        .map(|(xdr, quote)| (token_asset.clone(), swap_amount_clone, quote, xdr))
                        .map_err(|e| {
                            // Convert error and include token info for better error handling
                            RelayerError::Internal(format!(
                                "Failed to prepare swap transaction for token {token_asset} (amount {swap_amount_clone}): {e}",
                            ))
                        })
                })
            })
            .collect();

        // Prepare all swap transactions concurrently
        // Use join_all instead of try_join_all to collect all results (successes and failures)
        // This allows processing to continue even if some swaps fail
        let swap_prep_results = join_all(swap_prep_futures).await;

        // Queue each prepared swap transaction for background processing
        // This ensures swaps go through the same gate mechanism as regular transactions
        let mut swap_results = Vec::new();
        for result in swap_prep_results {
            match result {
                Ok((token_asset, swap_amount, quote, xdr)) => {
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
                Err(e) => {
                    // Log error but continue processing other swaps
                    // The error message already includes token and amount info from map_err above
                    error!(
                        %relayer_id,
                        error = %e,
                        "Failed to prepare swap transaction, skipping this token"
                    );
                    // Extract token and amount from error message
                    // Error format: "Failed to prepare swap transaction for token {token} (amount {amount}): {error}"
                    let error_msg = e.to_string();
                    let token_asset = error_msg
                        .split("token ")
                        .nth(1)
                        .and_then(|s| s.split(" (amount ").next())
                        .unwrap_or("unknown")
                        .to_string();
                    let swap_amount = error_msg
                        .split("(amount ")
                        .nth(1)
                        .and_then(|s| s.split(")").next())
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0);

                    swap_results.push(SwapResult {
                        mint: token_asset,
                        source_amount: swap_amount,
                        destination_amount: 0,
                        transaction_signature: String::new(),
                        error: Some(error_msg),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{NetworkConfigCommon, StellarNetworkConfig},
        domain::stellar::parse_account_id,
        jobs::MockJobProducerTrait,
        models::{
            NetworkConfigData, NetworkRepoModel, NetworkType, RelayerNetworkPolicy,
            RelayerRepoModel, RelayerStellarPolicy, RelayerStellarSwapConfig,
            StellarAllowedTokensPolicy, StellarAllowedTokensSwapConfig, StellarFeePaymentStrategy,
            StellarSwapStrategy,
        },
        repositories::{
            InMemoryNetworkRepository, MockRelayerRepository, MockTransactionRepository,
        },
        services::{
            provider::MockStellarProviderTrait, signer::MockStellarSignTrait,
            stellar_dex::MockStellarDexServiceTrait, MockTransactionCounterServiceTrait,
        },
    };
    use mockall::predicate::*;
    use soroban_rs::xdr::{
        AccountEntry, AccountEntryExt, AccountId, PublicKey, SequenceNumber, String32, Thresholds,
        Uint256, VecM, WriteXdr,
    };
    use std::future::ready;
    use std::sync::Arc;

    const TEST_PK: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
    const TEST_NETWORK_PASSPHRASE: &str = "Test SDF Network ; September 2015";
    const USDC_ASSET: &str = "USDC:GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN";

    /// Helper function to create a mock provider with trustline balance for USDC
    fn create_mock_provider_with_usdc_balance(balance: i64) -> MockStellarProviderTrait {
        let mut provider = MockStellarProviderTrait::new();
        provider.expect_get_ledger_entries().returning(move |keys| {
            use soroban_rs::stellar_rpc_client::{GetLedgerEntriesResponse, LedgerEntryResult};
            use soroban_rs::xdr::{
                LedgerEntry, LedgerEntryData, LedgerEntryExt, LedgerKey, TrustLineAsset,
                TrustLineEntry, TrustLineEntryExt, WriteXdr,
            };

            // Extract account_id and asset from the ledger key
            let (account_id, asset) = if let Some(LedgerKey::Trustline(trustline_key)) =
                keys.first()
            {
                (
                    trustline_key.account_id.clone(),
                    trustline_key.asset.clone(),
                )
            } else {
                // Fallback
                let fallback_account = parse_account_id(TEST_PK).unwrap_or_else(|_| {
                    AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32])))
                });
                let fallback_issuer =
                    parse_account_id("GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN")
                        .unwrap_or_else(|_| {
                            AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32])))
                        });
                let fallback_asset = TrustLineAsset::CreditAlphanum4(soroban_rs::xdr::AlphaNum4 {
                    asset_code: soroban_rs::xdr::AssetCode4(*b"USDC"),
                    issuer: fallback_issuer,
                });
                (fallback_account, fallback_asset)
            };

            let trustline_entry = TrustLineEntry {
                account_id,
                asset,
                balance,
                limit: i64::MAX,
                flags: 0,
                ext: TrustLineEntryExt::V0,
            };

            let ledger_entry = LedgerEntry {
                last_modified_ledger_seq: 0,
                data: LedgerEntryData::Trustline(trustline_entry),
                ext: LedgerEntryExt::V0,
            };

            // Encode LedgerEntryData to XDR base64 (not the full LedgerEntry)
            let xdr_base64 = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .expect("Failed to encode trustline entry data to XDR");

            Box::pin(ready(Ok(GetLedgerEntriesResponse {
                entries: Some(vec![LedgerEntryResult {
                    key: String::new(),
                    xdr: xdr_base64,
                    last_modified_ledger: 1000,
                    live_until_ledger_seq_ledger_seq: None,
                }]),
                latest_ledger: 1000,
            })))
        });

        // Mock get_account for sequence sync and XLM balance check
        provider.expect_get_account().returning(|_| {
            Box::pin(ready(Ok(AccountEntry {
                account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                balance: 100_000_000,         // 10 XLM (with 7 decimals)
                seq_num: SequenceNumber(100), // Non-zero sequence for sync_sequence
                num_sub_entries: 0,
                inflation_dest: None,
                flags: 0,
                home_domain: String32::default(),
                thresholds: Thresholds([0; 4]),
                signers: VecM::default(),
                ext: AccountEntryExt::V0,
            })))
        });

        provider
    }

    /// Helper function to create a test relayer with user fee payment strategy and swap config
    fn create_test_relayer_with_swap_config() -> RelayerRepoModel {
        let mut policy = RelayerStellarPolicy::default();
        policy.fee_payment_strategy = Some(StellarFeePaymentStrategy::User);
        policy.swap_config = Some(RelayerStellarSwapConfig {
            strategies: vec![StellarSwapStrategy::OrderBook],
            min_balance_threshold: None,
            cron_schedule: None,
        });
        policy.allowed_tokens = Some(vec![StellarAllowedTokensPolicy {
            asset: USDC_ASSET.to_string(),
            metadata: None,
            max_allowed_fee: None,
            swap_config: Some(StellarAllowedTokensSwapConfig {
                min_amount: Some(1000000),
                max_amount: Some(100000000),
                retain_min_amount: Some(1000000),
                slippage_percentage: Some(1.0),
            }),
        }]);

        RelayerRepoModel {
            id: "test-relayer-id".to_string(),
            name: "Test Relayer".to_string(),
            network: "testnet".to_string(),
            paused: false,
            network_type: NetworkType::Stellar,
            signer_id: "signer-id".to_string(),
            policies: RelayerNetworkPolicy::Stellar(policy),
            address: TEST_PK.to_string(),
            notification_id: Some("notification-id".to_string()),
            system_disabled: false,
            custom_rpc_urls: None,
            ..Default::default()
        }
    }

    /// Helper function to create a mock DEX service
    fn create_mock_dex_service() -> Arc<MockStellarDexServiceTrait> {
        let mut mock_dex = MockStellarDexServiceTrait::new();
        mock_dex.expect_supported_asset_types().returning(|| {
            use crate::services::stellar_dex::AssetType;
            std::collections::HashSet::from([AssetType::Native, AssetType::Classic])
        });
        mock_dex
            .expect_can_handle_asset()
            .returning(|asset| asset == USDC_ASSET || asset == "native");
        Arc::new(mock_dex)
    }

    /// Helper function to create a test network
    fn create_test_network() -> NetworkRepoModel {
        NetworkRepoModel {
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
                passphrase: Some(TEST_NETWORK_PASSPHRASE.to_string()),
                horizon_url: Some("https://horizon-testnet.stellar.org".to_string()),
            }),
        }
    }

    /// Helper function to create a Stellar relayer instance for testing with customizable mocks
    async fn create_test_relayer_with_mocks(
        relayer_model: RelayerRepoModel,
        provider: MockStellarProviderTrait,
        dex_service: Arc<MockStellarDexServiceTrait>,
        tx_job_result: Result<(), crate::jobs::JobProducerError>,
        notification_job_result: Result<(), crate::jobs::JobProducerError>,
    ) -> crate::domain::relayer::stellar::StellarRelayer<
        MockStellarProviderTrait,
        MockRelayerRepository,
        InMemoryNetworkRepository,
        MockTransactionRepository,
        MockJobProducerTrait,
        MockTransactionCounterServiceTrait,
        MockStellarSignTrait,
        MockStellarDexServiceTrait,
    > {
        let network_repository = Arc::new(InMemoryNetworkRepository::new());
        let test_network = create_test_network();
        network_repository.create(test_network).await.unwrap();

        let mut relayer_repo = MockRelayerRepository::new();
        let relayer_model_clone = relayer_model.clone();
        relayer_repo
            .expect_get_by_id()
            .returning(move |_| Ok(relayer_model_clone.clone()));

        // Mock update_policy for populate_allowed_tokens_metadata
        let relayer_model_clone2 = relayer_model.clone();
        relayer_repo
            .expect_update_policy()
            .returning(move |_, _| Ok(relayer_model_clone2.clone()));

        // Mock enable_relayer and disable_relayer for check_health
        let relayer_model_clone3 = relayer_model.clone();
        relayer_repo
            .expect_enable_relayer()
            .returning(move |_| Ok(relayer_model_clone3.clone()));
        let relayer_model_clone4 = relayer_model.clone();
        relayer_repo
            .expect_disable_relayer()
            .returning(move |_, _| Ok(relayer_model_clone4.clone()));

        let mut tx_repo = MockTransactionRepository::new();
        tx_repo.expect_create().returning(|t| Ok(t.clone()));

        let mut job_producer = MockJobProducerTrait::new();
        job_producer
            .expect_produce_transaction_request_job()
            .returning({
                let tx_job_result = tx_job_result.clone();
                move |_, _| {
                    let result = tx_job_result.clone();
                    Box::pin(async move { result })
                }
            });
        job_producer
            .expect_produce_send_notification_job()
            .returning({
                let notification_job_result = notification_job_result.clone();
                move |_, _| {
                    let result = notification_job_result.clone();
                    Box::pin(async move { result })
                }
            });
        job_producer
            .expect_produce_relayer_health_check_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));
        job_producer
            .expect_produce_check_transaction_status_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let mut counter = MockTransactionCounterServiceTrait::new();
        counter
            .expect_set()
            .returning(|_| Box::pin(async { Ok(()) }));
        let counter = Arc::new(counter);
        let signer = Arc::new(MockStellarSignTrait::new());

        crate::domain::relayer::stellar::StellarRelayer::new(
            relayer_model,
            signer,
            provider,
            crate::domain::relayer::stellar::StellarRelayerDependencies::new(
                Arc::new(relayer_repo),
                network_repository,
                Arc::new(tx_repo),
                counter,
                Arc::new(job_producer),
            ),
            dex_service,
        )
        .await
        .unwrap()
    }

    /// Helper function to create a Stellar relayer instance for testing
    async fn create_test_relayer_instance(
        relayer_model: RelayerRepoModel,
        provider: MockStellarProviderTrait,
        dex_service: Arc<MockStellarDexServiceTrait>,
    ) -> crate::domain::relayer::stellar::StellarRelayer<
        MockStellarProviderTrait,
        MockRelayerRepository,
        InMemoryNetworkRepository,
        MockTransactionRepository,
        MockJobProducerTrait,
        MockTransactionCounterServiceTrait,
        MockStellarSignTrait,
        MockStellarDexServiceTrait,
    > {
        create_test_relayer_with_mocks(relayer_model, provider, dex_service, Ok(()), Ok(())).await
    }

    #[tokio::test]
    async fn test_handle_token_swap_request_with_user_fee_strategy() {
        let relayer_model = create_test_relayer_with_swap_config();
        let provider = create_mock_provider_with_usdc_balance(5000000); // 5 USDC

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service.expect_supported_asset_types().returning(|| {
            use crate::services::stellar_dex::AssetType;
            std::collections::HashSet::from([AssetType::Native, AssetType::Classic])
        });
        dex_service
            .expect_can_handle_asset()
            .returning(|asset| asset == USDC_ASSET || asset == "native");

        // Mock prepare_swap_transaction
        dex_service.expect_prepare_swap_transaction().returning(|_| {
            Box::pin(ready(Ok((
                "AAAAAgAAAACige4lTdwSB/sto4SniEdJ2kOa2X65s5bqkd40J4DjSwAAAAEAAHAkAAAADwAAAAAAAAAAAAAAAQAAAAAAAAABAAAAAKKB7iVN3BIH+y2jhKeIR0naQ5rZfrmzluqR3jQngONLAAAAAAAAAAAAD0JAAAAAAAAAAAA=".to_string(),
                crate::services::stellar_dex::StellarQuoteResponse {
                    input_asset: USDC_ASSET.to_string(),
                    output_asset: "native".to_string(),
                    in_amount: 40000000,
                    out_amount: 10000000,
                    price_impact_pct: 0.0,
                    slippage_bps: 100,
                    path: None,
                },
            ))))
        });

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let result = relayer
            .handle_token_swap_request("test-relayer-id".to_string())
            .await;

        assert!(result.is_ok());
        let swap_results = result.unwrap();

        // Verify we have exactly one swap result
        assert_eq!(swap_results.len(), 1);

        let swap_result = &swap_results[0];

        // Verify swap result content
        assert_eq!(swap_result.mint, USDC_ASSET);
        assert_eq!(swap_result.source_amount, 4000000); // 5M balance - 1M retain = 4M
        assert_eq!(swap_result.destination_amount, 10000000);
        assert!(swap_result.error.is_none());
        assert!(!swap_result.transaction_signature.is_empty());

        // Verify transaction signature format (should be a UUID-like string)
        assert!(swap_result.transaction_signature.len() > 0);
    }

    #[tokio::test]
    async fn test_handle_token_swap_request_with_relayer_fee_strategy() {
        let mut relayer_model = create_test_relayer_with_swap_config();
        // Change to Relayer fee payment strategy
        if let RelayerNetworkPolicy::Stellar(ref mut policy) = relayer_model.policies {
            policy.fee_payment_strategy = Some(StellarFeePaymentStrategy::Relayer);
        }

        let provider = MockStellarProviderTrait::new();
        let dex_service = create_mock_dex_service();
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let result = relayer
            .handle_token_swap_request("test-relayer-id".to_string())
            .await;

        assert!(result.is_ok());
        let swap_results = result.unwrap();
        // Should return empty vector when fee payment strategy is not User
        assert!(swap_results.is_empty());
    }

    #[tokio::test]
    async fn test_handle_token_swap_request_no_swap_config() {
        let mut relayer_model = create_test_relayer_with_swap_config();
        // Remove swap config
        if let RelayerNetworkPolicy::Stellar(ref mut policy) = relayer_model.policies {
            policy.swap_config = None;
        }

        let provider = MockStellarProviderTrait::new();
        let dex_service = create_mock_dex_service();
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let result = relayer
            .handle_token_swap_request("test-relayer-id".to_string())
            .await;

        assert!(result.is_ok());
        let swap_results = result.unwrap();
        // Should return empty vector when no swap config
        assert!(swap_results.is_empty());
    }

    #[tokio::test]
    async fn test_handle_token_swap_request_no_allowed_tokens() {
        let mut relayer_model = create_test_relayer_with_swap_config();
        // Remove allowed tokens
        if let RelayerNetworkPolicy::Stellar(ref mut policy) = relayer_model.policies {
            policy.allowed_tokens = Some(vec![]);
        }

        let provider = MockStellarProviderTrait::new();
        let dex_service = create_mock_dex_service();
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let result = relayer
            .handle_token_swap_request("test-relayer-id".to_string())
            .await;

        assert!(result.is_ok());
        let swap_results = result.unwrap();
        // Should return empty vector when no allowed tokens
        assert!(swap_results.is_empty());
    }

    #[tokio::test]
    async fn test_handle_token_swap_request_balance_below_minimum() {
        let relayer_model = create_test_relayer_with_swap_config();
        let provider = create_mock_provider_with_usdc_balance(500000); // 0.5 USDC (below min_amount of 1 USDC)

        let dex_service = create_mock_dex_service();
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let result = relayer
            .handle_token_swap_request("test-relayer-id".to_string())
            .await;

        assert!(result.is_ok());
        let swap_results = result.unwrap();
        // Should return empty vector when balance is below minimum
        assert!(swap_results.is_empty());
    }

    #[tokio::test]
    async fn test_handle_token_swap_request_token_balance_fetch_failure() {
        let relayer_model = create_test_relayer_with_swap_config();
        let mut provider = MockStellarProviderTrait::new();

        // Mock get_ledger_entries to return an error
        provider.expect_get_ledger_entries().returning(|_| {
            Box::pin(ready(Err(crate::services::provider::ProviderError::Other(
                "Connection failed".to_string(),
            ))))
        });

        let dex_service = create_mock_dex_service();
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let result = relayer
            .handle_token_swap_request("test-relayer-id".to_string())
            .await;

        assert!(result.is_ok());
        let swap_results = result.unwrap();
        // Should return empty vector when token balance fetch fails
        assert!(swap_results.is_empty());
    }

    #[tokio::test]
    async fn test_handle_token_swap_request_dex_service_prepare_failure() {
        let relayer_model = create_test_relayer_with_swap_config();
        let provider = create_mock_provider_with_usdc_balance(50000000); // 5 USDC

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service.expect_supported_asset_types().returning(|| {
            use crate::services::stellar_dex::AssetType;
            std::collections::HashSet::from([AssetType::Native, AssetType::Classic])
        });
        dex_service
            .expect_can_handle_asset()
            .returning(|asset| asset == USDC_ASSET || asset == "native");

        // Mock prepare_swap_transaction to fail
        dex_service
            .expect_prepare_swap_transaction()
            .returning(|_| {
                Box::pin(ready(Err(
                    crate::services::stellar_dex::StellarDexServiceError::ApiError {
                        message: "Insufficient liquidity".to_string(),
                    },
                )))
            });

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let result = relayer
            .handle_token_swap_request("test-relayer-id".to_string())
            .await;

        assert!(result.is_ok());
        let swap_results = result.unwrap();
        // Should have one failed swap result
        assert_eq!(swap_results.len(), 1);
        assert!(swap_results[0].error.is_some());
        assert_eq!(swap_results[0].source_amount, 49000000); // 50M - 1M retain = 49M
        assert_eq!(swap_results[0].destination_amount, 0);
        assert!(swap_results[0].transaction_signature.is_empty());
    }

    #[tokio::test]
    async fn test_handle_token_swap_request_transaction_processing_failure() {
        let relayer_model = create_test_relayer_with_swap_config();
        let provider = create_mock_provider_with_usdc_balance(5000000); // 5 USDC

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service.expect_supported_asset_types().returning(|| {
            use crate::services::stellar_dex::AssetType;
            std::collections::HashSet::from([AssetType::Native, AssetType::Classic])
        });
        dex_service
            .expect_can_handle_asset()
            .returning(|asset| asset == USDC_ASSET || asset == "native");

        // Mock prepare_swap_transaction
        dex_service.expect_prepare_swap_transaction().returning(|_| {
            Box::pin(ready(Ok((
                "AAAAAgAAAACige4lTdwSB/sto4SniEdJ2kOa2X65s5bqkd40J4DjSwAAAAEAAHAkAAAADwAAAAAAAAAAAAAAAQAAAAAAAAABAAAAAKKB7iVN3BIH+y2jhKeIR0naQ5rZfrmzluqR3jQngONLAAAAAAAAAAAAD0JAAAAAAAAAAAA=".to_string(),
                crate::services::stellar_dex::StellarQuoteResponse {
                    input_asset: USDC_ASSET.to_string(),
                    output_asset: "native".to_string(),
                    in_amount: 40000000,
                    out_amount: 10000000,
                    price_impact_pct: 0.0,
                    slippage_bps: 100,
                    path: None,
                },
            ))))
        });

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_with_mocks(
            relayer_model,
            provider,
            dex_service,
            Err(crate::jobs::JobProducerError::QueueError(
                "Queue full".to_string(),
            )),
            Ok(()),
        )
        .await;

        let result = relayer
            .handle_token_swap_request("test-relayer-id".to_string())
            .await;

        assert!(result.is_ok());
        let swap_results = result.unwrap();
        // Should have one failed swap result
        assert_eq!(swap_results.len(), 1);
        assert!(swap_results[0].error.is_some());
        assert!(swap_results[0]
            .error
            .as_ref()
            .unwrap()
            .contains("Failed to queue transaction"));
        assert_eq!(swap_results[0].source_amount, 4000000); // 5M - 1M retain = 4M
        assert_eq!(swap_results[0].destination_amount, 0);
        assert!(swap_results[0].transaction_signature.is_empty());
    }

    #[tokio::test]
    async fn test_handle_token_swap_request_notification_failure() {
        let relayer_model = create_test_relayer_with_swap_config();
        let provider = create_mock_provider_with_usdc_balance(5000000); // 5 USDC

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service.expect_supported_asset_types().returning(|| {
            use crate::services::stellar_dex::AssetType;
            std::collections::HashSet::from([AssetType::Native, AssetType::Classic])
        });
        dex_service
            .expect_can_handle_asset()
            .returning(|asset| asset == USDC_ASSET || asset == "native");

        // Mock prepare_swap_transaction
        dex_service.expect_prepare_swap_transaction().returning(|_| {
            Box::pin(ready(Ok((
                "AAAAAgAAAACige4lTdwSB/sto4SniEdJ2kOa2X65s5bqkd40J4DjSwAAAAEAAHAkAAAADwAAAAAAAAAAAAAAAQAAAAAAAAABAAAAAKKB7iVN3BIH+y2jhKeIR0naQ5rZfrmzluqR3jQngONLAAAAAAAAAAAAD0JAAAAAAAAAAAA=".to_string(),
                crate::services::stellar_dex::StellarQuoteResponse {
                    input_asset: USDC_ASSET.to_string(),
                    output_asset: "native".to_string(),
                    in_amount: 40000000,
                    out_amount: 10000000,
                    price_impact_pct: 0.0,
                    slippage_bps: 100,
                    path: None,
                },
            ))))
        });

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_with_mocks(
            relayer_model,
            provider,
            dex_service,
            Ok(()),
            Err(crate::jobs::JobProducerError::QueueError(
                "Notification queue full".to_string(),
            )),
        )
        .await;

        let result = relayer
            .handle_token_swap_request("test-relayer-id".to_string())
            .await;

        // Should still succeed even if notification fails
        assert!(result.is_ok());
        let swap_results = result.unwrap();
        assert_eq!(swap_results.len(), 1);
        assert!(swap_results[0].error.is_none());
        assert!(!swap_results[0].transaction_signature.is_empty());
    }

    #[tokio::test]
    async fn test_handle_token_swap_request_multiple_tokens() {
        let mut relayer_model = create_test_relayer_with_swap_config();
        // Add a second token
        if let RelayerNetworkPolicy::Stellar(ref mut policy) = relayer_model.policies {
            policy.allowed_tokens = Some(vec![
                StellarAllowedTokensPolicy {
                    asset: USDC_ASSET.to_string(),
                    metadata: None,
                    max_allowed_fee: None,
                    swap_config: Some(StellarAllowedTokensSwapConfig {
                        min_amount: Some(1000000),
                        max_amount: Some(100000000),
                        retain_min_amount: Some(1000000),
                        slippage_percentage: Some(1.0),
                    }),
                },
                StellarAllowedTokensPolicy {
                    asset: "EURC:GDHU6WRG4IEQXM5NZ4BMPKOXHW76MZM4Y2IEMFDVXBSDP6SJY4ITNPP2"
                        .to_string(),
                    metadata: None,
                    max_allowed_fee: None,
                    swap_config: Some(StellarAllowedTokensSwapConfig {
                        min_amount: Some(2000000),
                        max_amount: Some(50000000),
                        retain_min_amount: Some(500000),
                        slippage_percentage: Some(0.5),
                    }),
                },
            ]);
        }

        // Mock get_ledger_entries for both tokens - will be called twice
        let provider = create_mock_provider_with_usdc_balance(5000000); // 5 units for both tokens

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service.expect_supported_asset_types().returning(|| {
            use crate::services::stellar_dex::AssetType;
            std::collections::HashSet::from([AssetType::Native, AssetType::Classic])
        });
        dex_service.expect_can_handle_asset().returning(|asset| {
            asset == USDC_ASSET
                || asset == "EURC:GDHU6WRG4IEQXM5NZ4BMPKOXHW76MZM4Y2IEMFDVXBSDP6SJY4ITNPP2"
                || asset == "native"
        });

        // Mock prepare_swap_transaction for both tokens
        dex_service.expect_prepare_swap_transaction().returning(|_| {
            Box::pin(ready(Ok((
                "AAAAAgAAAACige4lTdwSB/sto4SniEdJ2kOa2X65s5bqkd40J4DjSwAAAAEAAHAkAAAADwAAAAAAAAAAAAAAAQAAAAAAAAABAAAAAKKB7iVN3BIH+y2jhKeIR0naQ5rZfrmzluqR3jQngONLAAAAAAAAAAAAD0JAAAAAAAAAAAA=".to_string(),
                crate::services::stellar_dex::StellarQuoteResponse {
                    input_asset: USDC_ASSET.to_string(),
                    output_asset: "native".to_string(),
                    in_amount: 40000000,
                    out_amount: 10000000,
                    price_impact_pct: 0.0,
                    slippage_bps: 100,
                    path: None,
                },
            ))))
        });

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let result = relayer
            .handle_token_swap_request("test-relayer-id".to_string())
            .await;

        assert!(result.is_ok());
        let swap_results = result.unwrap();
        // Should have queued swap transactions for both tokens
        assert_eq!(swap_results.len(), 2);
        assert!(swap_results.iter().all(|r| r.error.is_none()));
        assert!(swap_results
            .iter()
            .all(|r| !r.transaction_signature.is_empty()));
    }

    #[tokio::test]
    async fn test_handle_token_swap_request_partial_failure() {
        let mut relayer_model = create_test_relayer_with_swap_config();
        // Add a second token
        if let RelayerNetworkPolicy::Stellar(ref mut policy) = relayer_model.policies {
            policy.allowed_tokens = Some(vec![
                StellarAllowedTokensPolicy {
                    asset: USDC_ASSET.to_string(),
                    metadata: None,
                    max_allowed_fee: None,
                    swap_config: Some(StellarAllowedTokensSwapConfig {
                        min_amount: Some(1000000),
                        max_amount: Some(100000000),
                        retain_min_amount: Some(1000000),
                        slippage_percentage: Some(1.0),
                    }),
                },
                StellarAllowedTokensPolicy {
                    asset: "EURC:GDHU6WRG4IEQXM5NZ4BMPKOXHW76MZM4Y2IEMFDVXBSDP6SJY4ITNPP2"
                        .to_string(),
                    metadata: None,
                    max_allowed_fee: None,
                    swap_config: Some(StellarAllowedTokensSwapConfig {
                        min_amount: Some(2000000),
                        max_amount: Some(50000000),
                        retain_min_amount: Some(500000),
                        slippage_percentage: Some(0.5),
                    }),
                },
            ]);
        }

        let mut provider = MockStellarProviderTrait::new();

        // Mock get_ledger_entries - first call succeeds, second fails
        let mut call_count = 0;
        provider.expect_get_ledger_entries().returning(move |_| {
            call_count += 1;
            if call_count == 1 {
                // First token (USDC) - return balance
                use soroban_rs::stellar_rpc_client::{GetLedgerEntriesResponse, LedgerEntryResult};
                use soroban_rs::xdr::{
                    LedgerEntry, LedgerEntryData, TrustLineAsset, TrustLineEntry, TrustLineEntryExt,
                };

                let trustline_entry = TrustLineEntry {
                    account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                    asset: TrustLineAsset::CreditAlphanum4(soroban_rs::xdr::AlphaNum4 {
                        asset_code: soroban_rs::xdr::AssetCode4(*b"USDC"),
                        issuer: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([
                            0x3b, 0x99, 0x11, 0x38, 0x0e, 0xfe, 0x98, 0x8b, 0xa0, 0xa8, 0x90, 0x0e,
                            0xb1, 0xcf, 0xe4, 0x4f, 0x36, 0x6f, 0x7d, 0xbe, 0x94, 0x6b, 0xed, 0x07,
                            0x72, 0x40, 0xf7, 0xf6, 0x24, 0xdf, 0x15, 0xc5,
                        ]))),
                    }),
                    balance: 5000000,
                    limit: i64::MAX,
                    flags: 0,
                    ext: TrustLineEntryExt::V0,
                };

                let ledger_entry = LedgerEntry {
                    last_modified_ledger_seq: 0,
                    data: LedgerEntryData::Trustline(trustline_entry),
                    ext: soroban_rs::xdr::LedgerEntryExt::V0,
                };

                // Encode LedgerEntryData to XDR base64 (not the full LedgerEntry)
                let xdr_base64 = ledger_entry
                    .data
                    .to_xdr_base64(soroban_rs::xdr::Limits::none())
                    .unwrap();

                Box::pin(ready(Ok(GetLedgerEntriesResponse {
                    entries: Some(vec![LedgerEntryResult {
                        key: String::new(),
                        xdr: xdr_base64,
                        last_modified_ledger: 1000,
                        live_until_ledger_seq_ledger_seq: None,
                    }]),
                    latest_ledger: 1000,
                })))
            } else {
                // Second token (EURC) - return error
                Box::pin(ready(Err(crate::services::provider::ProviderError::Other(
                    "Connection failed".to_string(),
                ))))
            }
        });

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service.expect_supported_asset_types().returning(|| {
            use crate::services::stellar_dex::AssetType;
            std::collections::HashSet::from([AssetType::Native, AssetType::Classic])
        });
        dex_service.expect_can_handle_asset().returning(|asset| {
            asset == USDC_ASSET
                || asset == "EURC:GDHU6WRG4IEQXM5NZ4BMPKOXHW76MZM4Y2IEMFDVXBSDP6SJY4ITNPP2"
                || asset == "native"
        });

        // Mock prepare_swap_transaction - only called once due to balance fetch failure for second token
        dex_service.expect_prepare_swap_transaction().returning(|_| {
            Box::pin(ready(Ok((
                "AAAAAgAAAACige4lTdwSB/sto4SniEdJ2kOa2X65s5bqkd40J4DjSwAAAAEAAHAkAAAADwAAAAAAAAAAAAAAAQAAAAAAAAABAAAAAKKB7iVN3BIH+y2jhKeIR0naQ5rZfrmzluqR3jQngONLAAAAAAAAAAAAD0JAAAAAAAAAAAA=".to_string(),
                crate::services::stellar_dex::StellarQuoteResponse {
                    input_asset: USDC_ASSET.to_string(),
                    output_asset: "native".to_string(),
                    in_amount: 40000000,
                    out_amount: 10000000,
                    price_impact_pct: 0.0,
                    slippage_bps: 100,
                    path: None,
                },
            ))))
        });

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let result = relayer
            .handle_token_swap_request("test-relayer-id".to_string())
            .await;

        assert!(result.is_ok());
        let swap_results = result.unwrap();
        // Should have only one successful swap (first token succeeds, second fails balance fetch)
        assert_eq!(swap_results.len(), 1);
        assert!(swap_results[0].error.is_none());
        assert!(!swap_results[0].transaction_signature.is_empty());
    }

    #[test]
    fn test_calculate_swap_amount_no_constraints() {
        let result = calculate_swap_amount(10000000, None, None, None).unwrap();
        assert_eq!(result, 10000000);
    }

    #[test]
    fn test_calculate_swap_amount_with_max_amount() {
        let result = calculate_swap_amount(10000000, None, Some(5000000), None).unwrap();
        assert_eq!(result, 5000000);
    }

    #[test]
    fn test_calculate_swap_amount_with_retain_min() {
        let result = calculate_swap_amount(10000000, None, None, Some(2000000)).unwrap();
        assert_eq!(result, 8000000); // 10M - 2M = 8M
    }

    #[test]
    fn test_calculate_swap_amount_with_max_and_retain() {
        let result = calculate_swap_amount(10000000, None, Some(5000000), Some(2000000)).unwrap();
        assert_eq!(result, 5000000); // min(5M, 8M) = 5M
    }

    #[test]
    fn test_calculate_swap_amount_below_minimum() {
        let result = calculate_swap_amount(500000, Some(1000000), None, None).unwrap();
        assert_eq!(result, 0); // Below minimum, should return 0
    }

    #[test]
    fn test_calculate_swap_amount_insufficient_for_retain() {
        let result = calculate_swap_amount(1000000, None, None, Some(2000000)).unwrap();
        assert_eq!(result, 0); // Can't retain minimum, should return 0
    }

    #[test]
    fn test_calculate_swap_amount_exact_minimum() {
        let result = calculate_swap_amount(1000000, Some(1000000), None, None).unwrap();
        assert_eq!(result, 1000000); // Exactly at minimum
    }

    #[test]
    fn test_calculate_swap_amount_all_constraints() {
        // Balance: 10M, Max: 5M, Retain: 2M, Min: 1M
        // Available: 10M - 2M = 8M
        // Capped by max: min(8M, 5M) = 5M
        // Above minimum: 5M >= 1M
        let result =
            calculate_swap_amount(10000000, Some(1000000), Some(5000000), Some(2000000)).unwrap();
        assert_eq!(result, 5000000);
    }

    #[test]
    fn test_calculate_swap_amount_balance_equals_retain_min() {
        // Balance exactly equals retain minimum - should return 0
        let result = calculate_swap_amount(2000000, None, None, Some(2000000)).unwrap();
        assert_eq!(result, 0);
    }

    #[test]
    fn test_calculate_swap_amount_balance_below_retain_min() {
        // Balance below retain minimum - should return 0
        let result = calculate_swap_amount(1000000, None, None, Some(2000000)).unwrap();
        assert_eq!(result, 0);
    }

    #[test]
    fn test_calculate_swap_amount_max_amount_larger_than_available() {
        // Max amount larger than available balance after retain
        let result = calculate_swap_amount(10000000, None, Some(15000000), Some(2000000)).unwrap();
        assert_eq!(result, 8000000); // 10M - 2M = 8M (not capped by max)
    }

    #[test]
    fn test_calculate_swap_amount_very_large_numbers() {
        // Test with very large numbers to ensure no overflow
        let large_balance = u64::MAX / 2;
        let large_max = u64::MAX / 4;
        let result = calculate_swap_amount(large_balance, None, Some(large_max), None).unwrap();
        assert_eq!(result, large_max); // Should be capped by max_amount
    }

    #[test]
    fn test_calculate_swap_amount_zero_balance() {
        let result = calculate_swap_amount(0, None, None, None).unwrap();
        assert_eq!(result, 0);
    }

    #[test]
    fn test_calculate_swap_amount_minimum_at_boundary() {
        // Amount exactly equals minimum after all calculations
        let result = calculate_swap_amount(3000000, Some(1000000), None, Some(2000000)).unwrap();
        assert_eq!(result, 1000000); // 3M - 2M = 1M (exactly at minimum)
    }

    #[test]
    fn test_calculate_swap_amount_max_capped_by_balance() {
        // Max amount larger than balance
        let result = calculate_swap_amount(5000000, None, Some(10000000), None).unwrap();
        assert_eq!(result, 5000000); // Capped by balance
    }

    #[test]
    fn test_calculate_swap_amount_complex_scenario() {
        // Complex scenario: Balance 15M, Max 10M, Retain 3M, Min 2M
        // Available: 15M - 3M = 12M
        // Capped by max: min(12M, 10M) = 10M
        // Above minimum: 10M >= 2M ✓
        let result =
            calculate_swap_amount(15000000, Some(2000000), Some(10000000), Some(3000000)).unwrap();
        assert_eq!(result, 10000000);
    }
}
