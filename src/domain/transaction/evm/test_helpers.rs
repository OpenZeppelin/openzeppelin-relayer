//! Shared test utilities for EVM transaction tests.
//! This module provides common test helpers, mocks, and builders to reduce code duplication.

#[cfg(test)]
pub mod test_utils {
    use crate::{
        domain::transaction::evm::{EvmRelayerTransaction, MockPriceCalculatorTrait},
        jobs::MockJobProducerTrait,
        models::{
            evm::Speed, EvmTransactionData, NetworkTransactionData, NetworkType, RelayerEvmPolicy,
            RelayerNetworkPolicy, RelayerRepoModel, TransactionRepoModel, TransactionStatus, U256,
        },
        repositories::{
            MockNetworkRepository, MockRelayerRepository, MockTransactionCounterTrait,
            MockTransactionRepository,
        },
        services::{MockEvmProviderTrait, MockSigner},
    };
    use chrono::Utc;
    use std::sync::Arc;

    /// Helper struct holding all the mocks we often need
    pub struct TestMocks {
        pub provider: MockEvmProviderTrait,
        pub relayer_repo: MockRelayerRepository,
        pub network_repo: MockNetworkRepository,
        pub tx_repo: MockTransactionRepository,
        pub job_producer: MockJobProducerTrait,
        pub signer: MockSigner,
        pub counter: MockTransactionCounterTrait,
        pub price_calc: MockPriceCalculatorTrait,
    }

    /// Returns a default `TestMocks` with zero-configuration stubs.
    /// You can override expectations in each test as needed.
    pub fn default_test_mocks() -> TestMocks {
        TestMocks {
            provider: MockEvmProviderTrait::new(),
            relayer_repo: MockRelayerRepository::new(),
            network_repo: MockNetworkRepository::new(),
            tx_repo: MockTransactionRepository::new(),
            job_producer: MockJobProducerTrait::new(),
            signer: MockSigner::new(),
            counter: MockTransactionCounterTrait::new(),
            price_calc: MockPriceCalculatorTrait::new(),
        }
    }

    /// Minimal "builder" for TransactionRepoModel.
    /// Allows quick creation of a test transaction with default fields,
    /// then updates them based on the provided status or overrides.
    pub fn make_test_transaction(status: TransactionStatus) -> TransactionRepoModel {
        TransactionRepoModel {
            id: "test-tx-id".to_string(),
            relayer_id: "test-relayer-id".to_string(),
            status,
            status_reason: None,
            created_at: Utc::now().to_rfc3339(),
            sent_at: None,
            confirmed_at: None,
            valid_until: None,
            network_type: NetworkType::Evm,
            network_data: NetworkTransactionData::Evm(EvmTransactionData {
                chain_id: 1,
                from: "0xSender".to_string(),
                to: Some("0xRecipient".to_string()),
                value: U256::from(0),
                data: Some("0xData".to_string()),
                gas_limit: Some(21000),
                gas_price: Some(20000000000),
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                nonce: None,
                signature: None,
                hash: None,
                speed: Some(Speed::Fast),
                raw: None,
            }),
            priced_at: None,
            hashes: Vec::new(),
            noop_count: None,
            is_canceled: Some(false),
            delete_at: None,
        }
    }

    /// Creates a test relayer with default configuration
    pub fn create_test_relayer() -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test-relayer-id".to_string(),
            name: "Test Relayer".to_string(),
            paused: false,
            system_disabled: false,
            network: "test_network".to_string(),
            network_type: NetworkType::Evm,
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
            signer_id: "test_signer".to_string(),
            address: "0x".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        }
    }

    /// Minimal "builder" for EvmRelayerTransaction.
    /// Takes mock dependencies as arguments.
    pub fn make_test_evm_relayer_transaction(
        relayer: RelayerRepoModel,
        mocks: TestMocks,
    ) -> EvmRelayerTransaction<
        MockEvmProviderTrait,
        MockRelayerRepository,
        MockNetworkRepository,
        MockTransactionRepository,
        MockJobProducerTrait,
        MockSigner,
        MockTransactionCounterTrait,
        MockPriceCalculatorTrait,
    > {
        EvmRelayerTransaction::new(
            relayer,
            mocks.provider,
            Arc::new(mocks.relayer_repo),
            Arc::new(mocks.network_repo),
            Arc::new(mocks.tx_repo),
            Arc::new(mocks.counter),
            Arc::new(mocks.job_producer),
            mocks.price_calc,
            mocks.signer,
        )
        .unwrap()
    }
}
