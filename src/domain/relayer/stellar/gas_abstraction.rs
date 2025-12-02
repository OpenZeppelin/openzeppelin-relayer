//! Gas abstraction implementation for Stellar relayers.
//!
//! This module implements the `GasAbstractionTrait` for Stellar relayers, providing
//! gas abstraction functionality including fee estimation and transaction preparation.

use async_trait::async_trait;
use chrono::Utc;
use soroban_rs::xdr::{Limits, Operation, TransactionEnvelope, WriteXdr};
use tracing::debug;

use crate::constants::{
    get_stellar_sponsored_transaction_validity_duration, STELLAR_DEFAULT_TRANSACTION_FEE,
};
use crate::domain::relayer::{
    stellar::xdr_utils::parse_transaction_xdr, GasAbstractionTrait, RelayerError, StellarRelayer,
};
use crate::domain::transaction::stellar::{
    utils::{
        add_operation_to_envelope, convert_xlm_fee_to_token, create_fee_payment_operation,
        estimate_fee, set_time_bounds, FeeQuote,
    },
    StellarTransactionValidator,
};
use crate::domain::xdr_needs_simulation;
use crate::jobs::JobProducerTrait;
use crate::models::{
    transaction::stellar::OperationSpec, SponsoredTransactionBuildRequest,
    SponsoredTransactionBuildResponse, SponsoredTransactionQuoteRequest,
    SponsoredTransactionQuoteResponse, StellarFeeEstimateResult, StellarPrepareTransactionResult,
    StellarTransactionData, TransactionInput,
};
use crate::models::{NetworkRepoModel, RelayerRepoModel, TransactionRepoModel};
use crate::repositories::{
    NetworkRepository, RelayerRepository, Repository, TransactionRepository,
};
use crate::services::provider::StellarProviderTrait;
use crate::services::signer::StellarSignTrait;
use crate::services::stellar_dex::StellarDexServiceTrait;
use crate::services::TransactionCounterServiceTrait;

#[async_trait]
impl<P, RR, NR, TR, J, TCS, S, D> GasAbstractionTrait
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
    async fn quote_sponsored_transaction(
        &self,
        params: SponsoredTransactionQuoteRequest,
    ) -> Result<SponsoredTransactionQuoteResponse, RelayerError> {
        let params = match params {
            SponsoredTransactionQuoteRequest::Stellar(p) => p,
            _ => {
                return Err(RelayerError::ValidationError(
                    "Expected Stellar fee estimate request parameters".to_string(),
                ));
            }
        };
        debug!(
            "Processing quote sponsored transaction request for token: {}",
            params.fee_token
        );

        // Validate allowed token
        let policy = self.relayer.policies.get_stellar_policy();
        StellarTransactionValidator::validate_allowed_token(&params.fee_token, &policy)
            .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Validate that either transaction_xdr or operations is provided
        if params.transaction_xdr.is_none() && params.operations.is_none() {
            return Err(RelayerError::ValidationError(
                "Must provide either transaction_xdr or operations in the request".to_string(),
            ));
        }

        // Build envelope from XDR or operations (reusing logic from build method)
        let envelope = build_envelope_from_request(
            params.transaction_xdr.as_ref(),
            params.operations.as_ref(),
            params.source_account.as_ref(),
            &self.network.passphrase,
            &self.provider,
        )
        .await?;

        // Run comprehensive security validation (similar to build method)
        StellarTransactionValidator::gasless_transaction_validation(
            &envelope,
            &self.relayer.address,
            &policy,
            &self.provider,
            None, // Duration validation not needed for quote
        )
        .await
        .map_err(|e| {
            RelayerError::ValidationError(format!("Failed to validate gasless transaction: {e}"))
        })?;

        // Estimate fee using estimate_fee utility which handles simulation if needed
        let inner_tx_fee = estimate_fee(&envelope, &self.provider, None)
            .await
            .map_err(crate::models::RelayerError::from)?;

        // Add fees for fee payment operation (100 stroops) and fee-bump transaction (100 stroops)
        // For Soroban transactions, the simulation already accounts for resource fees,
        // we just need to add the inclusion fees for the additional operations
        let is_soroban = xdr_needs_simulation(&envelope).unwrap_or(false);
        let mut additional_fees = 0;
        if !is_soroban {
            additional_fees = 2 * STELLAR_DEFAULT_TRANSACTION_FEE as u64; // 200 stroops total
        }
        let xlm_fee = inner_tx_fee + additional_fees;

        // Convert to token amount via DEX service
        let fee_quote = convert_xlm_fee_to_token(
            self.dex_service.as_ref(),
            &policy,
            xlm_fee,
            &params.fee_token,
        )
        .await
        .map_err(crate::models::RelayerError::from)?;

        // Validate max fee
        StellarTransactionValidator::validate_max_fee(fee_quote.fee_in_stroops, &policy)
            .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Validate token-specific max fee
        StellarTransactionValidator::validate_token_max_fee(
            &params.fee_token,
            fee_quote.fee_in_token,
            &policy,
        )
        .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Check user token balance to ensure they have enough to pay the fee
        StellarTransactionValidator::validate_user_token_balance(
            &envelope,
            &params.fee_token,
            fee_quote.fee_in_token,
            &self.provider,
        )
        .await
        .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        debug!("Fee estimate result: {:?}", fee_quote);

        let result = StellarFeeEstimateResult {
            fee_in_token_ui: fee_quote.fee_in_token_ui,
            fee_in_token: fee_quote.fee_in_token.to_string(),
            conversion_rate: fee_quote.conversion_rate.to_string(),
        };
        Ok(SponsoredTransactionQuoteResponse::Stellar(result))
    }

    async fn build_sponsored_transaction(
        &self,
        params: SponsoredTransactionBuildRequest,
    ) -> Result<SponsoredTransactionBuildResponse, RelayerError> {
        let params = match params {
            SponsoredTransactionBuildRequest::Stellar(p) => p,
            _ => {
                return Err(RelayerError::ValidationError(
                    "Expected Stellar prepare transaction request parameters".to_string(),
                ));
            }
        };
        debug!(
            "Processing prepare transaction request for token: {}",
            params.fee_token
        );

        // Validate allowed token
        let policy = self.relayer.policies.get_stellar_policy();
        StellarTransactionValidator::validate_allowed_token(&params.fee_token, &policy)
            .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Validate that either transaction_xdr or operations is provided
        if params.transaction_xdr.is_none() && params.operations.is_none() {
            return Err(RelayerError::ValidationError(
                "Must provide either transaction_xdr or operations in the request".to_string(),
            ));
        }

        // Build envelope from XDR or operations (reusing shared helper)
        let envelope = build_envelope_from_request(
            params.transaction_xdr.as_ref(),
            params.operations.as_ref(),
            params.source_account.as_ref(),
            &self.network.passphrase,
            &self.provider,
        )
        .await?;

        StellarTransactionValidator::gasless_transaction_validation(
            &envelope,
            &self.relayer.address,
            &policy,
            &self.provider,
            None, // Duration validation not needed here as time bounds are set during build
        )
        .await
        .map_err(|e| {
            RelayerError::ValidationError(format!("Failed to validate gasless transaction: {e}"))
        })?;

        // Get fee estimate using estimate_fee utility which handles simulation if needed
        // For non-Soroban transactions, we'll add 200 stroops (100 for fee payment op + 100 for fee-bump)
        let inner_tx_fee = estimate_fee(&envelope, &self.provider, None)
            .await
            .map_err(crate::models::RelayerError::from)?;

        // Add fees for fee payment operation (100 stroops) and fee-bump transaction (100 stroops)
        // For Soroban transactions, the simulation already accounts for resource fees,
        // we just need to add the inclusion fees for the additional operations
        let is_soroban = xdr_needs_simulation(&envelope).unwrap_or(false);
        let mut additional_fees = 0;
        if !is_soroban {
            additional_fees = 2 * STELLAR_DEFAULT_TRANSACTION_FEE as u64; // 200 stroops total
        }
        let xlm_fee = inner_tx_fee + additional_fees;

        debug!(
            inner_tx_fee = inner_tx_fee,
            additional_fees = additional_fees,
            total_fee = xlm_fee,
            "Fee estimated: inner transaction + fee payment op + fee-bump transaction fee"
        );

        // Calculate fee quote first to check user balance before modifying envelope
        let preliminary_fee_quote = convert_xlm_fee_to_token(
            self.dex_service.as_ref(),
            &policy,
            xlm_fee,
            &params.fee_token,
        )
        .await
        .map_err(crate::models::RelayerError::from)?;

        // Validate max fee
        StellarTransactionValidator::validate_max_fee(
            preliminary_fee_quote.fee_in_stroops,
            &policy,
        )
        .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Validate token-specific max fee
        StellarTransactionValidator::validate_token_max_fee(
            &params.fee_token,
            preliminary_fee_quote.fee_in_token,
            &policy,
        )
        .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Check user token balance to ensure they have enough to pay the fee
        StellarTransactionValidator::validate_user_token_balance(
            &envelope,
            &params.fee_token,
            preliminary_fee_quote.fee_in_token,
            &self.provider,
        )
        .await
        .map_err(|e| RelayerError::ValidationError(e.to_string()))?;

        // Add payment operation using the validated fee quote
        let mut final_envelope = add_payment_operation_to_envelope(
            envelope,
            &preliminary_fee_quote,
            &params.fee_token,
            &self.relayer.address,
        )?;

        // Use the validated fee quote (no duplicate DEX call)
        let fee_quote = preliminary_fee_quote;

        debug!(
            estimated_fee = xlm_fee,
            final_fee_in_token = fee_quote.fee_in_token_ui,
            "Transaction prepared successfully"
        );

        // Set final time bounds just before returning to give user maximum time to review and submit
        let valid_until = Utc::now() + get_stellar_sponsored_transaction_validity_duration();
        set_time_bounds(&mut final_envelope, valid_until)
            .map_err(crate::models::RelayerError::from)?;

        // Serialize final transaction
        let extended_xdr = final_envelope
            .to_xdr_base64(Limits::none())
            .map_err(|e| RelayerError::Internal(format!("Failed to serialize XDR: {e}")))?;

        Ok(SponsoredTransactionBuildResponse::Stellar(
            StellarPrepareTransactionResult {
                transaction: extended_xdr,
                fee_in_token: fee_quote.fee_in_token.to_string(),
                fee_in_token_ui: fee_quote.fee_in_token_ui,
                fee_in_stroops: fee_quote.fee_in_stroops.to_string(),
                fee_token: params.fee_token,
                valid_until: valid_until.to_rfc3339(),
            },
        ))
    }
}

/// Add payment operation to envelope using a pre-computed fee quote
///
/// This function adds a fee payment operation to the transaction envelope using
/// a pre-computed FeeQuote. This avoids duplicate DEX calls and ensures the
/// validated fee quote matches the fee amount in the payment operation.
///
/// Note: Time bounds should be set separately just before returning the transaction
/// to give the user maximum time to review and submit.
///
/// # Arguments
/// * `envelope` - The transaction envelope to add the payment operation to
/// * `fee_quote` - Pre-computed fee quote containing the token amount to charge
/// * `fee_token` - Asset identifier for the fee token
/// * `relayer_address` - Address of the relayer receiving the fee payment
///
/// # Returns
/// The updated envelope with the payment operation added (if not Soroban)
fn add_payment_operation_to_envelope(
    mut envelope: TransactionEnvelope,
    fee_quote: &FeeQuote,
    fee_token: &str,
    relayer_address: &str,
) -> Result<TransactionEnvelope, RelayerError> {
    // Convert fee amount to i64 for payment operation
    let fee_amount = i64::try_from(fee_quote.fee_in_token).map_err(|_| {
        RelayerError::Internal(
            "Fee amount too large for payment operation (exceeds i64::MAX)".to_string(),
        )
    })?;

    let is_soroban = xdr_needs_simulation(&envelope).unwrap_or(false);
    // For Soroban we don't add the fee payment operation because of Soroban limitation to allow just single operation in the transaction
    if !is_soroban {
        // Add fee payment operation to envelope
        add_fee_payment_operation(&mut envelope, fee_token, fee_amount, relayer_address)?;
    }

    Ok(envelope)
}

/// Build a transaction envelope from either XDR or operations
///
/// This helper function is used by both quote and build methods to construct
/// a transaction envelope from either a pre-built XDR transaction or from
/// operations with a source account.
///
/// When building from operations, this function fetches the user's current
/// sequence number from the network to ensure the transaction can be properly
/// signed and submitted by the user.
async fn build_envelope_from_request<P>(
    transaction_xdr: Option<&String>,
    operations: Option<&Vec<OperationSpec>>,
    source_account: Option<&String>,
    network_passphrase: &str,
    provider: &P,
) -> Result<TransactionEnvelope, RelayerError>
where
    P: StellarProviderTrait + Send + Sync,
{
    if let Some(xdr) = transaction_xdr {
        parse_transaction_xdr(xdr, false)
            .map_err(|e| RelayerError::Internal(format!("Failed to parse XDR: {e}")))
    } else if let Some(ops) = operations {
        // Build envelope from operations
        let source_account = source_account.ok_or_else(|| {
            RelayerError::ValidationError(
                "source_account is required when providing operations".to_string(),
            )
        })?;

        // Create StellarTransactionData from operations
        // Fetch the user's current sequence number from the network
        // This is required because the user will sign the transaction with their account
        let account_entry = provider.get_account(source_account).await.map_err(|e| {
            RelayerError::Internal(format!(
                "Failed to fetch account sequence number for {source_account}: {e}",
            ))
        })?;

        // Use the next sequence number (current + 1)
        let next_sequence = account_entry.seq_num.0 + 1;

        let stellar_data = StellarTransactionData {
            source_account: source_account.clone(),
            fee: None,
            sequence_number: Some(next_sequence as i64),
            memo: None,
            valid_until: None,
            network_passphrase: network_passphrase.to_string(),
            signatures: vec![],
            hash: None,
            simulation_transaction_data: None,
            transaction_input: TransactionInput::Operations(ops.clone()),
            signed_envelope_xdr: None,
        };

        // Build unsigned envelope from operations
        stellar_data.build_unsigned_envelope().map_err(|e| {
            RelayerError::Internal(format!("Failed to build envelope from operations: {e}"))
        })
    } else {
        Err(RelayerError::ValidationError(
            "Must provide either transaction_xdr or operations in the request".to_string(),
        ))
    }
}

/// Add a fee payment operation to the transaction envelope
fn add_fee_payment_operation(
    envelope: &mut TransactionEnvelope,
    fee_token: &str,
    fee_amount: i64,
    relayer_address: &str,
) -> Result<(), RelayerError> {
    let payment_op_spec = create_fee_payment_operation(relayer_address, fee_token, fee_amount)
        .map_err(crate::models::RelayerError::from)?;

    // Convert OperationSpec to XDR Operation
    let payment_op = Operation::try_from(payment_op_spec)
        .map_err(|e| RelayerError::Internal(format!("Failed to convert payment operation: {e}")))?;

    // Add payment operation to transaction
    add_operation_to_envelope(envelope, payment_op).map_err(crate::models::RelayerError::from)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::transaction::stellar::utils::parse_account_id;
    use crate::services::stellar_dex::AssetType;
    use crate::{
        config::{NetworkConfigCommon, StellarNetworkConfig},
        jobs::MockJobProducerTrait,
        models::{
            transaction::stellar::OperationSpec, AssetSpec, NetworkConfigData, NetworkRepoModel,
            NetworkType, RelayerNetworkPolicy, RelayerRepoModel, RelayerStellarPolicy,
            SponsoredTransactionBuildRequest, SponsoredTransactionQuoteRequest,
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
    use soroban_rs::stellar_rpc_client::GetLedgerEntriesResponse;
    use soroban_rs::stellar_rpc_client::LedgerEntryResult;
    use soroban_rs::xdr::{
        AccountEntry, AccountEntryExt, AccountId, AlphaNum4, AssetCode4, LedgerEntry,
        LedgerEntryData, LedgerEntryExt, LedgerKey, Limits, MuxedAccount, Operation, OperationBody,
        PaymentOp, Preconditions, PublicKey, SequenceNumber, String32, Thresholds, Transaction,
        TransactionEnvelope, TransactionExt, TransactionV1Envelope, TrustLineEntry,
        TrustLineEntryExt, Uint256, VecM, WriteXdr,
    };
    use std::future::ready;
    use std::sync::Arc;
    use stellar_strkey::ed25519::PublicKey as Ed25519PublicKey;

    const TEST_PK: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
    const TEST_NETWORK_PASSPHRASE: &str = "Test SDF Network ; September 2015";
    const USDC_ASSET: &str = "USDC:GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN";

    /// Helper function to create a test transaction XDR
    fn create_test_transaction_xdr() -> String {
        // Use a different account than TEST_PK (relayer address) to avoid validation error
        let source_pk = Ed25519PublicKey::from_string(
            "GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2",
        )
        .unwrap();
        let dest_pk = Ed25519PublicKey::from_string(
            "GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ",
        )
        .unwrap();

        let payment_op = PaymentOp {
            destination: MuxedAccount::Ed25519(Uint256(dest_pk.0)),
            asset: soroban_rs::xdr::Asset::Native,
            amount: 1000000,
        };

        let operation = Operation {
            source_account: None,
            body: OperationBody::Payment(payment_op),
        };

        let operations: VecM<Operation, 100> = vec![operation].try_into().unwrap();

        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
            fee: 100,
            seq_num: SequenceNumber(2), // Must be > account sequence (1)
            cond: Preconditions::None,
            memo: soroban_rs::xdr::Memo::None,
            operations,
            ext: TransactionExt::V0,
        };

        let envelope = TransactionV1Envelope {
            tx,
            signatures: vec![].try_into().unwrap(),
        };

        let tx_envelope = TransactionEnvelope::Tx(envelope);
        tx_envelope.to_xdr_base64(Limits::none()).unwrap()
    }

    /// Helper function to create a test relayer with user fee payment strategy
    fn create_test_relayer_with_user_fee_strategy() -> RelayerRepoModel {
        let mut policy = RelayerStellarPolicy::default();
        policy.fee_payment_strategy = Some(crate::models::StellarFeePaymentStrategy::User);
        policy.allowed_tokens = Some(vec![crate::models::StellarAllowedTokensPolicy {
            asset: USDC_ASSET.to_string(),
            metadata: None,
            max_allowed_fee: None,
            swap_config: None,
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
        mock_dex
            .expect_supported_asset_types()
            .returning(|| std::collections::HashSet::from([AssetType::Native, AssetType::Classic]));
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
        let network_repository = Arc::new(InMemoryNetworkRepository::new());
        let test_network = create_test_network();
        network_repository.create(test_network).await.unwrap();

        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());
        let counter = Arc::new(MockTransactionCounterServiceTrait::new());
        let signer = Arc::new(MockStellarSignTrait::new());

        crate::domain::relayer::stellar::StellarRelayer::new(
            relayer_model,
            signer,
            provider,
            crate::domain::relayer::stellar::StellarRelayerDependencies::new(
                relayer_repo,
                network_repository,
                tx_repo,
                counter,
                job_producer,
            ),
            dex_service,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn test_quote_sponsored_transaction_with_xdr() {
        let relayer_model = create_test_relayer_with_user_fee_strategy();
        let mut provider = MockStellarProviderTrait::new();

        // Mock account for validation
        provider.expect_get_account().returning(|_| {
            Box::pin(ready(Ok(AccountEntry {
                account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                balance: 1000000000,
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

        // Mock get_ledger_entries for token balance validation
        // This mock extracts the account ID from the ledger key and returns a trustline with sufficient balance
        provider.expect_get_ledger_entries().returning(|keys| {
            // Extract account ID from the first ledger key (should be a Trustline key)
            let account_id = if let Some(LedgerKey::Trustline(trustline_key)) = keys.first() {
                trustline_key.account_id.clone()
            } else {
                // Fallback: try to parse TEST_PK
                parse_account_id(TEST_PK).unwrap_or_else(|_| {
                    AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32])))
                })
            };

            let issuer_id =
                parse_account_id("GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN")
                    .unwrap_or_else(|_| {
                        AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32])))
                    });

            // Create a trustline entry with sufficient balance
            let trustline_entry = TrustLineEntry {
                account_id,
                asset: soroban_rs::xdr::TrustLineAsset::CreditAlphanum4(AlphaNum4 {
                    asset_code: AssetCode4(*b"USDC"),
                    issuer: issuer_id,
                }),
                balance: 10_000_000i64,
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
            let xdr = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .expect("Failed to encode trustline entry data to XDR");

            Box::pin(ready(Ok(GetLedgerEntriesResponse {
                entries: Some(vec![LedgerEntryResult {
                    key: "test_key".to_string(),
                    xdr,
                    last_modified_ledger: 0u32,
                    live_until_ledger_seq_ledger_seq: None,
                }]),
                latest_ledger: 0,
            })))
        });

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service
            .expect_supported_asset_types()
            .returning(|| std::collections::HashSet::from([AssetType::Native, AssetType::Classic]));

        // Mock get_xlm_to_token_quote for fee conversion (XLM -> token)
        dex_service
            .expect_get_xlm_to_token_quote()
            .returning(|_, _, _, _| {
                Box::pin(ready(Ok(
                    crate::services::stellar_dex::StellarQuoteResponse {
                        input_asset: "native".to_string(),
                        output_asset: USDC_ASSET.to_string(),
                        in_amount: 100000,
                        out_amount: 1500000,
                        price_impact_pct: 0.0,
                        slippage_bps: 100,
                        path: None,
                    },
                )))
            });

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let transaction_xdr = create_test_transaction_xdr();
        let request = SponsoredTransactionQuoteRequest::Stellar(
            crate::models::StellarFeeEstimateRequestParams {
                transaction_xdr: Some(transaction_xdr),
                operations: None,
                source_account: None,
                fee_token: USDC_ASSET.to_string(),
            },
        );

        let result = relayer.quote_sponsored_transaction(request).await;
        if let Err(e) = &result {
            eprintln!("Quote error: {:?}", e);
        }
        assert!(result.is_ok());

        if let SponsoredTransactionQuoteResponse::Stellar(quote) = result.unwrap() {
            assert_eq!(quote.fee_in_token, "1500000");
            assert!(!quote.fee_in_token_ui.is_empty());
            assert!(!quote.conversion_rate.is_empty());
        } else {
            panic!("Expected Stellar quote response");
        }
    }

    #[tokio::test]
    async fn test_quote_sponsored_transaction_with_operations() {
        let relayer_model = create_test_relayer_with_user_fee_strategy();
        let mut provider = MockStellarProviderTrait::new();

        provider.expect_get_account().returning(|_| {
            Box::pin(ready(Ok(AccountEntry {
                account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                balance: 1000000000,
                seq_num: SequenceNumber(-1),
                num_sub_entries: 0,
                inflation_dest: None,
                flags: 0,
                home_domain: String32::default(),
                thresholds: Thresholds([0; 4]),
                signers: VecM::default(),
                ext: AccountEntryExt::V0,
            })))
        });

        // Mock get_ledger_entries for token balance validation
        // This mock extracts the account ID from the ledger key and returns a trustline with sufficient balance
        provider.expect_get_ledger_entries().returning(|keys| {
            // Extract account ID from the first ledger key (should be a Trustline key)
            let account_id = if let Some(LedgerKey::Trustline(trustline_key)) = keys.first() {
                trustline_key.account_id.clone()
            } else {
                // Fallback: use the source account from the test
                parse_account_id("GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2")
                    .unwrap_or_else(|_| {
                        AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32])))
                    })
            };

            let issuer_id =
                parse_account_id("GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN")
                    .unwrap_or_else(|_| {
                        AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32])))
                    });

            // Create a trustline entry with sufficient balance
            let trustline_entry = TrustLineEntry {
                account_id,
                asset: soroban_rs::xdr::TrustLineAsset::CreditAlphanum4(AlphaNum4 {
                    asset_code: AssetCode4(*b"USDC"),
                    issuer: issuer_id,
                }),
                balance: 10_000_000i64,
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
            let xdr = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .expect("Failed to encode trustline entry data to XDR");

            Box::pin(ready(Ok(
                soroban_rs::stellar_rpc_client::GetLedgerEntriesResponse {
                    entries: Some(vec![LedgerEntryResult {
                        key: "test_key".to_string(),
                        xdr,
                        last_modified_ledger: 0u32,
                        live_until_ledger_seq_ledger_seq: None,
                    }]),
                    latest_ledger: 0,
                },
            )))
        });

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service
            .expect_supported_asset_types()
            .returning(|| std::collections::HashSet::from([AssetType::Native, AssetType::Classic]));

        // Mock get_xlm_to_token_quote for fee conversion (XLM -> token)
        dex_service
            .expect_get_xlm_to_token_quote()
            .returning(|_, _, _, _| {
                Box::pin(ready(Ok(
                    crate::services::stellar_dex::StellarQuoteResponse {
                        input_asset: "native".to_string(),
                        output_asset: USDC_ASSET.to_string(),
                        in_amount: 100000,
                        out_amount: 1500000,
                        price_impact_pct: 0.0,
                        slippage_bps: 100,
                        path: None,
                    },
                )))
            });

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let operations = vec![OperationSpec::Payment {
            destination: TEST_PK.to_string(),
            amount: 1000000,
            asset: AssetSpec::Native,
        }];

        let request = SponsoredTransactionQuoteRequest::Stellar(
            crate::models::StellarFeeEstimateRequestParams {
                transaction_xdr: None,
                operations: Some(operations),
                source_account: Some(
                    "GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2".to_string(),
                ),
                fee_token: USDC_ASSET.to_string(),
            },
        );

        let result = relayer.quote_sponsored_transaction(request).await;
        if let Err(e) = &result {
            eprintln!("Quote error: {:?}", e);
        }
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_quote_sponsored_transaction_invalid_token() {
        let relayer_model = create_test_relayer_with_user_fee_strategy();
        let provider = MockStellarProviderTrait::new();
        let dex_service = create_mock_dex_service();
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let transaction_xdr = create_test_transaction_xdr();
        let request = SponsoredTransactionQuoteRequest::Stellar(
            crate::models::StellarFeeEstimateRequestParams {
                transaction_xdr: Some(transaction_xdr),
                operations: None,
                source_account: None,
                fee_token: "INVALID:TOKEN".to_string(),
            },
        );

        let result = relayer.quote_sponsored_transaction(request).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RelayerError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_quote_sponsored_transaction_missing_xdr_and_operations() {
        let relayer_model = create_test_relayer_with_user_fee_strategy();
        let provider = MockStellarProviderTrait::new();
        let dex_service = create_mock_dex_service();
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let request = SponsoredTransactionQuoteRequest::Stellar(
            crate::models::StellarFeeEstimateRequestParams {
                transaction_xdr: None,
                operations: None,
                source_account: None,
                fee_token: USDC_ASSET.to_string(),
            },
        );

        let result = relayer.quote_sponsored_transaction(request).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RelayerError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_build_sponsored_transaction_with_xdr() {
        let relayer_model = create_test_relayer_with_user_fee_strategy();
        let mut provider = MockStellarProviderTrait::new();

        provider.expect_get_account().returning(|_| {
            Box::pin(ready(Ok(AccountEntry {
                account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                balance: 1000000000,
                seq_num: SequenceNumber(-1),
                num_sub_entries: 0,
                inflation_dest: None,
                flags: 0,
                home_domain: String32::default(),
                thresholds: Thresholds([0; 4]),
                signers: VecM::default(),
                ext: AccountEntryExt::V0,
            })))
        });

        // Mock get_ledger_entries for token balance validation
        // This mock extracts the account ID from the ledger key and returns a trustline with sufficient balance
        provider.expect_get_ledger_entries().returning(|keys| {
            // Extract account ID from the first ledger key (should be a Trustline key)
            let account_id = if let Some(LedgerKey::Trustline(trustline_key)) = keys.first() {
                trustline_key.account_id.clone()
            } else {
                // Fallback: try to parse TEST_PK
                parse_account_id(TEST_PK).unwrap_or_else(|_| {
                    AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32])))
                })
            };

            let issuer_id =
                parse_account_id("GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN")
                    .unwrap_or_else(|_| {
                        AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32])))
                    });

            // Create a trustline entry with sufficient balance (10 USDC = 10000000 with 6 decimals)
            let trustline_entry = TrustLineEntry {
                account_id,
                asset: soroban_rs::xdr::TrustLineAsset::CreditAlphanum4(AlphaNum4 {
                    asset_code: AssetCode4(*b"USDC"),
                    issuer: issuer_id,
                }),
                balance: 10_000_000i64, // 10 USDC (with 6 decimals) - sufficient for fee
                limit: i64::MAX,
                flags: 0,
                ext: TrustLineEntryExt::V0, // V0 has no liabilities
            };

            let ledger_entry = LedgerEntry {
                last_modified_ledger_seq: 0,
                data: LedgerEntryData::Trustline(trustline_entry),
                ext: LedgerEntryExt::V0,
            };

            // Encode LedgerEntryData to XDR base64 (not the full LedgerEntry)
            let xdr = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .expect("Failed to encode trustline entry data to XDR");

            Box::pin(ready(Ok(
                soroban_rs::stellar_rpc_client::GetLedgerEntriesResponse {
                    entries: Some(vec![LedgerEntryResult {
                        key: "test_key".to_string(),
                        xdr,
                        last_modified_ledger: 0u32,
                        live_until_ledger_seq_ledger_seq: None,
                    }]),
                    latest_ledger: 0,
                },
            )))
        });

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service
            .expect_supported_asset_types()
            .returning(|| std::collections::HashSet::from([AssetType::Native, AssetType::Classic]));

        // Mock get_xlm_to_token_quote for build (converting XLM fee to token)
        dex_service
            .expect_get_xlm_to_token_quote()
            .returning(|_, _, _, _| {
                Box::pin(ready(Ok(
                    crate::services::stellar_dex::StellarQuoteResponse {
                        input_asset: "native".to_string(),
                        output_asset: USDC_ASSET.to_string(),
                        in_amount: 1000000,
                        out_amount: 1500000,
                        price_impact_pct: 0.0,
                        slippage_bps: 100,
                        path: None,
                    },
                )))
            });

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let transaction_xdr = create_test_transaction_xdr();
        let request = SponsoredTransactionBuildRequest::Stellar(
            crate::models::StellarPrepareTransactionRequestParams {
                transaction_xdr: Some(transaction_xdr),
                operations: None,
                source_account: None,
                fee_token: USDC_ASSET.to_string(),
            },
        );

        let result = relayer.build_sponsored_transaction(request).await;
        assert!(result.is_ok());

        if let SponsoredTransactionBuildResponse::Stellar(build) = result.unwrap() {
            assert!(!build.transaction.is_empty());
            assert_eq!(build.fee_in_token, "1500000");
            assert!(!build.fee_in_token_ui.is_empty());
            assert_eq!(build.fee_token, USDC_ASSET);
            assert!(!build.valid_until.is_empty());
        } else {
            panic!("Expected Stellar build response");
        }
    }

    #[tokio::test]
    async fn test_build_sponsored_transaction_with_operations() {
        let relayer_model = create_test_relayer_with_user_fee_strategy();
        let mut provider = MockStellarProviderTrait::new();

        provider.expect_get_account().returning(|_| {
            Box::pin(ready(Ok(AccountEntry {
                account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                balance: 1000000000,
                seq_num: SequenceNumber(-1),
                num_sub_entries: 0,
                inflation_dest: None,
                flags: 0,
                home_domain: String32::default(),
                thresholds: Thresholds([0; 4]),
                signers: VecM::default(),
                ext: AccountEntryExt::V0,
            })))
        });

        provider.expect_get_ledger_entries().returning(|_| {
            use crate::domain::transaction::stellar::utils::parse_account_id;
            use soroban_rs::stellar_rpc_client::LedgerEntryResult;
            use soroban_rs::xdr::{
                AccountId, AlphaNum4, AssetCode4, LedgerEntry, LedgerEntryData, LedgerEntryExt,
                PublicKey, TrustLineEntry, TrustLineEntryExt, Uint256, WriteXdr,
            };

            // Parse account IDs - use the source account from the test
            let account_id =
                parse_account_id("GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2")
                    .unwrap_or_else(|_| {
                        AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32])))
                    });
            let issuer_id =
                parse_account_id("GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN")
                    .unwrap_or_else(|_| {
                        AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32])))
                    });

            // Create a trustline entry with sufficient balance (10 USDC = 10000000 with 6 decimals)
            // The fee is 1500000 (from the quote), so 10 USDC is more than enough
            let trustline_entry = TrustLineEntry {
                account_id,
                asset: soroban_rs::xdr::TrustLineAsset::CreditAlphanum4(AlphaNum4 {
                    asset_code: AssetCode4(*b"USDC"),
                    issuer: issuer_id,
                }),
                balance: 10_000_000i64,
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
            // The parse_ledger_entry_from_xdr function expects just the data portion
            let xdr = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .expect("Failed to encode trustline entry data to XDR");

            Box::pin(ready(Ok(
                soroban_rs::stellar_rpc_client::GetLedgerEntriesResponse {
                    entries: Some(vec![LedgerEntryResult {
                        key: "test_key".to_string(),
                        xdr,
                        last_modified_ledger: 0u32,
                        live_until_ledger_seq_ledger_seq: None,
                    }]),
                    latest_ledger: 0,
                },
            )))
        });

        let mut dex_service = MockStellarDexServiceTrait::new();
        dex_service
            .expect_supported_asset_types()
            .returning(|| std::collections::HashSet::from([AssetType::Native, AssetType::Classic]));

        dex_service
            .expect_get_xlm_to_token_quote()
            .returning(|_, _, _, _| {
                Box::pin(ready(Ok(
                    crate::services::stellar_dex::StellarQuoteResponse {
                        input_asset: "native".to_string(),
                        output_asset: USDC_ASSET.to_string(),
                        in_amount: 1000000,
                        out_amount: 1500000,
                        price_impact_pct: 0.0,
                        slippage_bps: 100,
                        path: None,
                    },
                )))
            });

        let dex_service = Arc::new(dex_service);
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let operations = vec![OperationSpec::Payment {
            destination: TEST_PK.to_string(),
            amount: 1000000,
            asset: AssetSpec::Native,
        }];

        let request = SponsoredTransactionBuildRequest::Stellar(
            crate::models::StellarPrepareTransactionRequestParams {
                transaction_xdr: None,
                operations: Some(operations),
                source_account: Some(
                    "GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2".to_string(),
                ),
                fee_token: USDC_ASSET.to_string(),
            },
        );

        let result = relayer.build_sponsored_transaction(request).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_build_sponsored_transaction_missing_source_account() {
        let relayer_model = create_test_relayer_with_user_fee_strategy();
        let provider = MockStellarProviderTrait::new();
        let dex_service = create_mock_dex_service();
        let relayer = create_test_relayer_instance(relayer_model, provider, dex_service).await;

        let operations = vec![OperationSpec::Payment {
            destination: TEST_PK.to_string(),
            amount: 1000000,
            asset: AssetSpec::Native,
        }];

        let request = SponsoredTransactionBuildRequest::Stellar(
            crate::models::StellarPrepareTransactionRequestParams {
                transaction_xdr: None,
                operations: Some(operations),
                source_account: None,
                fee_token: USDC_ASSET.to_string(),
            },
        );

        let result = relayer.build_sponsored_transaction(request).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RelayerError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_build_envelope_from_request_with_xdr() {
        let provider = MockStellarProviderTrait::new();
        let transaction_xdr = create_test_transaction_xdr();
        let result = build_envelope_from_request(
            Some(&transaction_xdr),
            None,
            None,
            TEST_NETWORK_PASSPHRASE,
            &provider,
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_build_envelope_from_request_with_operations() {
        let mut provider = MockStellarProviderTrait::new();

        // Mock get_account to return a valid account with sequence number
        provider.expect_get_account().returning(|_| {
            Box::pin(ready(Ok(AccountEntry {
                account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                balance: 1000000000,
                seq_num: SequenceNumber(100),
                num_sub_entries: 0,
                inflation_dest: None,
                flags: 0,
                home_domain: String32::default(),
                thresholds: Thresholds([0; 4]),
                signers: VecM::default(),
                ext: AccountEntryExt::V0,
            })))
        });

        let operations = vec![OperationSpec::Payment {
            destination: TEST_PK.to_string(),
            amount: 1000000,
            asset: AssetSpec::Native,
        }];

        let result = build_envelope_from_request(
            None,
            Some(&operations),
            Some(&TEST_PK.to_string()),
            TEST_NETWORK_PASSPHRASE,
            &provider,
        )
        .await;
        assert!(result.is_ok());

        // Verify the sequence number is set correctly (current + 1 = 101)
        if let Ok(envelope) = result {
            if let TransactionEnvelope::Tx(tx_env) = envelope {
                assert_eq!(tx_env.tx.seq_num.0, 101);
            }
        }
    }

    #[tokio::test]
    async fn test_build_envelope_from_request_missing_source_account() {
        let provider = MockStellarProviderTrait::new();
        let operations = vec![OperationSpec::Payment {
            destination: TEST_PK.to_string(),
            amount: 1000000,
            asset: AssetSpec::Native,
        }];

        let result = build_envelope_from_request(
            None,
            Some(&operations),
            None,
            TEST_NETWORK_PASSPHRASE,
            &provider,
        )
        .await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RelayerError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_build_envelope_from_request_missing_both() {
        let provider = MockStellarProviderTrait::new();
        let result =
            build_envelope_from_request(None, None, None, TEST_NETWORK_PASSPHRASE, &provider).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RelayerError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_build_envelope_from_request_invalid_xdr() {
        let provider = MockStellarProviderTrait::new();
        let result = build_envelope_from_request(
            Some(&"INVALID_XDR".to_string()),
            None,
            None,
            TEST_NETWORK_PASSPHRASE,
            &provider,
        )
        .await;
        assert!(result.is_err());
    }
}
