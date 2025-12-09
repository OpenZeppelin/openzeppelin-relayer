//! Operations-based transaction preparation logic.

use eyre::Result;
use tracing::info;

use super::common::{get_next_sequence, sign_stellar_transaction, simulate_if_needed};
use crate::{
    constants::STELLAR_DEFAULT_TRANSACTION_FEE,
    domain::extract_operations,
    models::{
        RelayerStellarPolicy, StellarFeePaymentStrategy, StellarTransactionData, TransactionError,
        TransactionRepoModel,
    },
    repositories::TransactionCounterTrait,
    services::{provider::StellarProviderTrait, signer::Signer},
};

/// Process operations-based transaction.
///
/// This function:
/// 1. Gets the next sequence number for the relayer
/// 2. Updates the stellar data with the sequence number
/// 3. Builds the unsigned envelope from operations
/// 4. Rejects gasless transactions (User fee payment strategy) - these should use unsigned_xdr path
/// 5. Simulates the transaction if needed (for Soroban operations)
/// 6. Signs the transaction envelope
///
/// # Arguments
/// * `counter_service` - Service for managing transaction sequence numbers
/// * `relayer_id` - The relayer's ID
/// * `relayer_address` - The relayer's Stellar address
/// * `tx` - The transaction model to process
/// * `stellar_data` - The stellar-specific transaction data containing operations
/// * `provider` - Provider for Stellar RPC operations
/// * `signer` - Service for signing transactions
/// * `relayer_policy` - Optional relayer policy for validation
///
/// # Returns
/// The updated stellar data with simulation results (if applicable) and signature
#[allow(clippy::too_many_arguments)]
pub async fn process_operations<C, P, S>(
    counter_service: &C,
    relayer_id: &str,
    relayer_address: &str,
    tx: &TransactionRepoModel,
    stellar_data: StellarTransactionData,
    provider: &P,
    signer: &S,
    relayer_policy: Option<&RelayerStellarPolicy>,
) -> Result<StellarTransactionData, TransactionError>
where
    C: TransactionCounterTrait + Send + Sync,
    P: StellarProviderTrait + Send + Sync,
    S: Signer + Send + Sync,
{
    // Reject gasless transactions (User fee payment strategy) in operations path
    // Gasless transactions should be processed via fee bump(SignedXdr) path only.
    if let Some(policy) = relayer_policy {
        if matches!(
            policy.fee_payment_strategy,
            Some(StellarFeePaymentStrategy::User)
        ) {
            return Err(TransactionError::ValidationError(
                "Gasless transactions (User fee payment strategy) are not supported via operations path. \
                 Please use fee bump(SignedXdr) path for gasless transactions.".to_string(),
            ));
        }
    }
    // Get the next sequence number
    let sequence_i64 = get_next_sequence(counter_service, relayer_id, relayer_address).await?;

    info!(
        "Using sequence number {} for operations transaction {}",
        sequence_i64, tx.id
    );

    // Update stellar data with sequence
    let stellar_data = stellar_data.with_sequence_number(sequence_i64);

    // Build the unsigned envelope
    let unsigned_env = stellar_data
        .get_envelope_for_simulation()
        .map_err(TransactionError::from)?;

    // Check if simulation is needed and apply results
    let stellar_data_with_sim = match simulate_if_needed(&unsigned_env, provider).await? {
        Some(sim_resp) => {
            info!("Applying simulation results to operations transaction");
            // Get operation count from the envelope
            let op_count = extract_operations(&unsigned_env)?.len() as u64;
            stellar_data
                .with_simulation_data(sim_resp, op_count)
                .map_err(|e| {
                    TransactionError::ValidationError(format!(
                        "Failed to apply simulation data: {e}"
                    ))
                })?
        }
        None => {
            // For non-simulated transactions, ensure fee is set to default
            let op_count = extract_operations(&unsigned_env)?.len() as u32;
            let fee = STELLAR_DEFAULT_TRANSACTION_FEE * op_count;
            stellar_data.with_fee(fee)
        }
    };

    // Sign the transaction
    // The signer will build the envelope from operations and sign it
    sign_stellar_transaction(signer, stellar_data_with_sim).await
}

#[cfg(test)]
mod tests {
    use std::future::ready;

    use super::*;
    use crate::{
        domain::{
            transaction::stellar::test_helpers::TEST_PK, SignTransactionResponse,
            SignTransactionResponseStellar,
        },
        models::{
            AssetSpec, DecoratedSignature, NetworkTransactionData, NetworkType, OperationSpec,
            RepositoryError, TransactionInput, TransactionStatus,
        },
        repositories::MockTransactionCounterTrait,
        services::{
            provider::{MockStellarProviderTrait, ProviderError},
            signer::MockSigner,
        },
    };
    use soroban_rs::stellar_rpc_client::SimulateTransactionResponse;
    use soroban_rs::xdr::{self};

    fn create_test_transaction() -> TransactionRepoModel {
        TransactionRepoModel {
            id: "test-tx-1".to_string(),
            relayer_id: "test-relayer".to_string(),
            status: TransactionStatus::Pending,
            status_reason: None,
            network_data: NetworkTransactionData::Stellar(create_test_stellar_data()),
            created_at: chrono::Utc::now().to_rfc3339(),
            sent_at: None,
            confirmed_at: None,
            valid_until: None,
            priced_at: None,
            hashes: vec![],
            network_type: NetworkType::Stellar,
            noop_count: None,
            is_canceled: Some(false),
            delete_at: None,
        }
    }

    fn create_test_stellar_data() -> StellarTransactionData {
        StellarTransactionData {
            source_account: TEST_PK.to_string(),
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            fee: None,
            sequence_number: None,
            transaction_input: TransactionInput::Operations(vec![OperationSpec::Payment {
                destination: TEST_PK.to_string(),
                amount: 10000000, // 1 XLM in stroops
                asset: AssetSpec::Native,
            }]),
            memo: None,
            valid_until: None,
            signatures: vec![],
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
        }
    }

    fn create_dummy_signature() -> DecoratedSignature {
        DecoratedSignature {
            hint: xdr::SignatureHint([0, 1, 2, 3]),
            signature: xdr::Signature(vec![0; 64].try_into().unwrap()),
        }
    }

    #[tokio::test]
    async fn test_process_operations_payment_success() {
        let relayer_id = "test-relayer";
        let relayer_address = TEST_PK;

        let mut counter = MockTransactionCounterTrait::new();
        counter
            .expect_get_and_increment()
            .returning(|_, _| Box::pin(ready(Ok(42))));

        let provider = MockStellarProviderTrait::new();

        let mut signer = MockSigner::new();
        signer.expect_sign_transaction().returning(|_| {
            Box::pin(async {
                Ok(SignTransactionResponse::Stellar(
                    SignTransactionResponseStellar {
                        signature: create_dummy_signature(),
                    },
                ))
            })
        });

        let tx = create_test_transaction();
        let stellar_data = create_test_stellar_data();

        let result = process_operations(
            &counter,
            relayer_id,
            relayer_address,
            &tx,
            stellar_data,
            &provider,
            &signer,
            None,
        )
        .await;

        match result {
            Ok(updated_data) => {
                assert_eq!(updated_data.sequence_number, Some(42));
                assert_eq!(updated_data.signatures.len(), 1);
            }
            Err(e) => panic!("Test failed with error: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_process_operations_with_soroban_simulation() {
        let relayer_id = "test-relayer";
        let relayer_address = TEST_PK;

        let mut counter = MockTransactionCounterTrait::new();
        counter
            .expect_get_and_increment()
            .returning(|_, _| Box::pin(ready(Ok(42))));

        let mut provider = MockStellarProviderTrait::new();
        // Mock simulation response for Soroban operations
        provider
            .expect_simulate_transaction_envelope()
            .returning(|_| {
                Box::pin(async {
                    Ok(SimulateTransactionResponse {
                        min_resource_fee: 100,
                        transaction_data: "AAAAAQAAAAAAAAACAAAAAAAAAAAAAAAAAAAABgAAAAEAAAAGAAAAAG0JZTO9fU6p3NeJp5w3TpKhZmx6p1pR7mq9wFwCnEIuAAAAFAAAAAEAAAAAAAAAB8NVb2IAAAH0AAAAAQAAAAAAABfAAAAAAAAAAPUAAAAAAAAENgAAAAA=".to_string(),
                        ..Default::default()
                    })
                })
            });

        let mut signer = MockSigner::new();
        signer.expect_sign_transaction().returning(|_| {
            Box::pin(async {
                Ok(SignTransactionResponse::Stellar(
                    SignTransactionResponseStellar {
                        signature: create_dummy_signature(),
                    },
                ))
            })
        });

        let tx = create_test_transaction();
        let mut stellar_data = create_test_stellar_data();
        // Change to a Soroban operation to trigger simulation
        stellar_data.transaction_input =
            TransactionInput::Operations(vec![OperationSpec::InvokeContract {
                contract_address: "CA7QYNF7SOWQ3GLR2BGMZEHXAVIRZA4KVWLTJJFC7MGXUA74P7UJUWDA"
                    .to_string(),
                function_name: "transfer".to_string(),
                args: vec![],
                auth: None,
            }]);

        let result = process_operations(
            &counter,
            relayer_id,
            relayer_address,
            &tx,
            stellar_data,
            &provider,
            &signer,
            None,
        )
        .await;

        match result {
            Ok(updated_data) => {
                assert_eq!(updated_data.sequence_number, Some(42));
                assert!(updated_data.simulation_transaction_data.is_some());
                assert_eq!(updated_data.signatures.len(), 1);
            }
            Err(e) => panic!("Test failed with error: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_process_operations_sequence_failure() {
        let relayer_id = "test-relayer";
        let relayer_address = TEST_PK;

        let mut counter = MockTransactionCounterTrait::new();
        counter.expect_get_and_increment().returning(|_, _| {
            Box::pin(async { Err(RepositoryError::NotFound("Counter not found".to_string())) })
        });

        let provider = MockStellarProviderTrait::new();
        let signer = MockSigner::new();

        let tx = create_test_transaction();
        let stellar_data = create_test_stellar_data();

        let result = process_operations(
            &counter,
            relayer_id,
            relayer_address,
            &tx,
            stellar_data,
            &provider,
            &signer,
            None,
        )
        .await;

        assert!(result.is_err());
        // The sequence error is mapped to UnexpectedError in get_next_sequence
        assert!(matches!(
            result.unwrap_err(),
            TransactionError::UnexpectedError(_)
        ));
    }

    #[tokio::test]
    async fn test_process_operations_build_envelope_failure() {
        let relayer_id = "test-relayer";
        let relayer_address = TEST_PK;

        let mut counter = MockTransactionCounterTrait::new();
        counter
            .expect_get_and_increment()
            .returning(|_, _| Box::pin(ready(Ok(42))));

        let provider = MockStellarProviderTrait::new();
        let mut signer = MockSigner::new();
        // Empty operations might still reach the signer
        signer.expect_sign_transaction().returning(|_| {
            Box::pin(async {
                Err(crate::models::SignerError::SigningError(
                    "Cannot sign empty transaction".to_string(),
                ))
            })
        });

        let tx = create_test_transaction();
        let mut stellar_data = create_test_stellar_data();
        // Set invalid operation to cause envelope build failure
        stellar_data.transaction_input = TransactionInput::Operations(vec![]);

        let result = process_operations(
            &counter,
            relayer_id,
            relayer_address,
            &tx,
            stellar_data,
            &provider,
            &signer,
            None,
        )
        .await;

        assert!(result.is_err());
        // Empty operations should cause an error
        // The exact error depends on where validation happens
    }

    #[tokio::test]
    async fn test_process_operations_signer_failure() {
        let relayer_id = "test-relayer";
        let relayer_address = TEST_PK;

        let mut counter = MockTransactionCounterTrait::new();
        counter
            .expect_get_and_increment()
            .returning(|_, _| Box::pin(ready(Ok(42))));

        let provider = MockStellarProviderTrait::new();

        let mut signer = MockSigner::new();
        signer.expect_sign_transaction().returning(|_| {
            Box::pin(async {
                Err(crate::models::SignerError::SigningError(
                    "Signing failed".to_string(),
                ))
            })
        });

        let tx = create_test_transaction();
        let stellar_data = create_test_stellar_data();

        let result = process_operations(
            &counter,
            relayer_id,
            relayer_address,
            &tx,
            stellar_data,
            &provider,
            &signer,
            None,
        )
        .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TransactionError::SignerError(_)
        ));
    }

    #[tokio::test]
    async fn test_process_operations_simulation_failure() {
        let relayer_id = "test-relayer";
        let relayer_address = TEST_PK;

        let mut counter = MockTransactionCounterTrait::new();
        counter
            .expect_get_and_increment()
            .returning(|_, _| Box::pin(ready(Ok(100))));

        let mut provider = MockStellarProviderTrait::new();
        // Mock simulation to fail
        provider
            .expect_simulate_transaction_envelope()
            .returning(|_| {
                Box::pin(async {
                    Err(ProviderError::Other(
                        "Simulation failed: insufficient resources".to_string(),
                    ))
                })
            });

        let signer = MockSigner::new();

        let tx = create_test_transaction();
        let mut stellar_data = create_test_stellar_data();
        // Use Soroban operation to trigger simulation
        stellar_data.transaction_input =
            TransactionInput::Operations(vec![OperationSpec::InvokeContract {
                contract_address: "CA7QYNF7SOWQ3GLR2BGMZEHXAVIRZA4KVWLTJJFC7MGXUA74P7UJUWDA"
                    .to_string(),
                function_name: "test".to_string(),
                args: vec![],
                auth: None,
            }]);

        let result = process_operations(
            &counter,
            relayer_id,
            relayer_address,
            &tx,
            stellar_data,
            &provider,
            &signer,
            None,
        )
        .await;

        assert!(result.is_err());

        match result.unwrap_err() {
            TransactionError::UnderlyingProvider(provider_error) => match provider_error {
                ProviderError::Other(msg) => {
                    assert!(msg.contains("Simulation failed"));
                }
                _ => panic!("Expected UnexpectedError"),
            },
            _ => panic!("Expected UnexpectedError"),
        }
    }

    #[tokio::test]
    async fn test_process_operations_rejects_user_fee_payment_strategy() {
        use crate::models::{RelayerStellarPolicy, StellarFeePaymentStrategy};

        let relayer_id = "test-relayer";
        let relayer_address = TEST_PK;

        let counter = MockTransactionCounterTrait::new();
        let provider = MockStellarProviderTrait::new();
        let signer = MockSigner::new();

        let tx = create_test_transaction();
        let stellar_data = create_test_stellar_data();

        // Create a policy with User fee payment strategy (gasless mode)
        let mut policy = RelayerStellarPolicy::default();
        policy.fee_payment_strategy = Some(StellarFeePaymentStrategy::User);

        let result = process_operations(
            &counter,
            relayer_id,
            relayer_address,
            &tx,
            stellar_data,
            &provider,
            &signer,
            Some(&policy),
        )
        .await;

        // Should return a validation error
        assert!(result.is_err());

        match result.unwrap_err() {
            TransactionError::ValidationError(msg) => {
                assert!(msg.contains("Gasless transactions"));
                assert!(msg.contains("User fee payment strategy"));
                assert!(msg.contains("not supported via operations path"));
                assert!(msg.contains("fee bump"));
            }
            other => panic!("Expected ValidationError, got: {:?}", other),
        }
    }
}
