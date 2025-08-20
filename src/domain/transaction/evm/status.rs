//! This module contains the status-related functionality for EVM transactions.
//! It includes methods for checking transaction status, determining when to resubmit
//! or replace transactions with NOOPs, and updating transaction status in the repository.

use chrono::{DateTime, Duration, Utc};
use eyre::Result;
use log::info;

use super::EvmRelayerTransaction;
use super::{
    get_age_of_sent_at, has_enough_confirmations, is_noop, is_transaction_valid, make_noop,
    too_many_attempts, too_many_noop_attempts,
};
use crate::constants::ARBITRUM_TIME_TO_RESUBMIT;
use crate::models::{EvmNetwork, NetworkRepoModel, NetworkType};
use crate::repositories::{NetworkRepository, RelayerRepository};
use crate::{
    domain::transaction::evm::price_calculator::PriceCalculatorTrait,
    jobs::JobProducerTrait,
    models::{
        NetworkTransactionData, RelayerRepoModel, TransactionError, TransactionRepoModel,
        TransactionStatus, TransactionUpdateRequest,
    },
    repositories::{Repository, TransactionCounterTrait, TransactionRepository},
    services::{EvmProviderTrait, Signer},
    utils::{get_resubmit_timeout_for_speed, get_resubmit_timeout_with_backoff},
};

impl<P, RR, NR, TR, J, S, TCR, PC> EvmRelayerTransaction<P, RR, NR, TR, J, S, TCR, PC>
where
    P: EvmProviderTrait + Send + Sync,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: Signer + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PC: PriceCalculatorTrait + Send + Sync,
{
    pub(super) async fn check_transaction_status(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<TransactionStatus, TransactionError> {
        if tx.status == TransactionStatus::Expired
            || tx.status == TransactionStatus::Failed
            || tx.status == TransactionStatus::Confirmed
        {
            return Ok(tx.status.clone());
        }

        let evm_data = tx.network_data.get_evm_transaction_data()?;
        let tx_hash = evm_data
            .hash
            .as_ref()
            .ok_or(TransactionError::UnexpectedError(
                "Transaction hash is missing".to_string(),
            ))?;

        let receipt_result = self.provider().get_transaction_receipt(tx_hash).await?;

        if let Some(receipt) = receipt_result {
            if !receipt.status() {
                return Ok(TransactionStatus::Failed);
            }
            let last_block_number = self.provider().get_block_number().await?;
            let tx_block_number = receipt
                .block_number
                .ok_or(TransactionError::UnexpectedError(
                    "Transaction receipt missing block number".to_string(),
                ))?;

            let network_model = self
                .network_repository()
                .get_by_chain_id(NetworkType::Evm, evm_data.chain_id)
                .await?
                .ok_or(TransactionError::UnexpectedError(format!(
                    "Network with chain id {} not found",
                    evm_data.chain_id
                )))?;

            let network = EvmNetwork::try_from(network_model).map_err(|e| {
                TransactionError::UnexpectedError(format!(
                    "Error converting network model to EvmNetwork: {}",
                    e
                ))
            })?;

            if !has_enough_confirmations(
                tx_block_number,
                last_block_number,
                network.required_confirmations,
            ) {
                info!("Transaction mined but not confirmed: {}", tx_hash);
                return Ok(TransactionStatus::Mined);
            }
            Ok(TransactionStatus::Confirmed)
        } else {
            info!("Transaction not yet mined: {}", tx_hash);
            Ok(TransactionStatus::Submitted)
        }
    }

    /// Determines if a transaction should be resubmitted.
    pub(super) async fn should_resubmit(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<bool, TransactionError> {
        if tx.status != TransactionStatus::Submitted {
            return Err(TransactionError::UnexpectedError(format!(
                "Transaction must be in Submitted status to resubmit, found: {:?}",
                tx.status
            )));
        }

        let evm_data = tx.network_data.get_evm_transaction_data()?;
        let age = get_age_of_sent_at(tx)?;

        // Check if network lacks mempool and determine appropriate timeout
        let network_model = self
            .network_repository()
            .get_by_chain_id(NetworkType::Evm, evm_data.chain_id)
            .await?
            .ok_or(TransactionError::UnexpectedError(format!(
                "Network with chain id {} not found",
                evm_data.chain_id
            )))?;

        let network = EvmNetwork::try_from(network_model).map_err(|e| {
            TransactionError::UnexpectedError(format!(
                "Error converting network model to EvmNetwork: {}",
                e
            ))
        })?;

        let timeout = match network.is_arbitrum() {
            true => ARBITRUM_TIME_TO_RESUBMIT,
            false => get_resubmit_timeout_for_speed(&evm_data.speed),
        };

        let timeout_with_backoff = match network.is_arbitrum() {
            true => timeout, // Use base timeout without backoff for Arbitrum
            false => get_resubmit_timeout_with_backoff(timeout, tx.hashes.len()),
        };

        if age > Duration::milliseconds(timeout_with_backoff) {
            info!("Transaction has been pending for too long, resubmitting");
            return Ok(true);
        }
        Ok(false)
    }

    /// Determines if a transaction should be replaced with a NOOP transaction.
    pub(super) async fn should_noop(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<bool, TransactionError> {
        if too_many_noop_attempts(tx) {
            info!("Transaction has too many NOOP attempts already");
            return Ok(false);
        }

        let evm_data = tx.network_data.get_evm_transaction_data()?;
        if is_noop(&evm_data) {
            return Ok(false);
        }

        let network_model = self
            .network_repository()
            .get_by_chain_id(NetworkType::Evm, evm_data.chain_id)
            .await?
            .ok_or(TransactionError::UnexpectedError(format!(
                "Network with chain id {} not found",
                evm_data.chain_id
            )))?;

        let network = EvmNetwork::try_from(network_model).map_err(|e| {
            TransactionError::UnexpectedError(format!(
                "Error converting network model to EvmNetwork: {}",
                e
            ))
        })?;

        if network.is_rollup() && too_many_attempts(tx) {
            info!("Rollup transaction has too many attempts, will replace with NOOP");
            return Ok(true);
        }

        if !is_transaction_valid(&tx.created_at, &tx.valid_until) {
            info!("Transaction is expired, will replace with NOOP");
            return Ok(true);
        }

        if tx.status == TransactionStatus::Pending {
            let created_at = &tx.created_at;
            let created_time = DateTime::parse_from_rfc3339(created_at)
                .map_err(|_| {
                    TransactionError::UnexpectedError("Error parsing created_at time".to_string())
                })?
                .with_timezone(&Utc);
            let age = Utc::now().signed_duration_since(created_time);
            if age > Duration::minutes(1) {
                info!("Transaction in Pending state for over 1 minute, will replace with NOOP");
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Helper method that updates transaction status only if it's different from the current status.
    pub(super) async fn update_transaction_status_if_needed(
        &self,
        tx: TransactionRepoModel,
        new_status: TransactionStatus,
    ) -> Result<TransactionRepoModel, TransactionError> {
        if tx.status != new_status {
            return self.update_transaction_status(tx, new_status).await;
        }
        Ok(tx)
    }

    /// Prepares a NOOP transaction update request.
    pub(super) async fn prepare_noop_update_request(
        &self,
        tx: &TransactionRepoModel,
        is_cancellation: bool,
    ) -> Result<TransactionUpdateRequest, TransactionError> {
        let mut evm_data = tx.network_data.get_evm_transaction_data()?;
        let network_model = self
            .network_repository()
            .get_by_chain_id(NetworkType::Evm, evm_data.chain_id)
            .await?
            .ok_or(TransactionError::UnexpectedError(format!(
                "Network with chain id {} not found",
                evm_data.chain_id
            )))?;

        let network = EvmNetwork::try_from(network_model).map_err(|e| {
            TransactionError::UnexpectedError(format!(
                "Error converting network model to EvmNetwork: {}",
                e
            ))
        })?;

        make_noop(&mut evm_data, &network, Some(self.provider())).await?;

        let noop_count = tx.noop_count.unwrap_or(0) + 1;
        let update_request = TransactionUpdateRequest {
            network_data: Some(NetworkTransactionData::Evm(evm_data)),
            noop_count: Some(noop_count),
            is_canceled: if is_cancellation {
                Some(true)
            } else {
                tx.is_canceled
            },
            ..Default::default()
        };
        Ok(update_request)
    }

    /// Handles transactions in the Submitted state.
    async fn handle_submitted_state(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        if self.should_resubmit(&tx).await? {
            let resubmitted_tx = self.handle_resubmission(tx).await?;
            self.schedule_status_check(&resubmitted_tx, None).await?;
            return Ok(resubmitted_tx);
        }

        self.schedule_status_check(&tx, Some(5)).await?;
        self.update_transaction_status_if_needed(tx, TransactionStatus::Submitted)
            .await
    }

    /// Processes transaction resubmission logic
    async fn handle_resubmission(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!("Scheduling resubmit job for transaction: {}", tx.id);

        let tx_to_process = if self.should_noop(&tx).await? {
            self.process_noop_transaction(&tx).await?
        } else {
            tx
        };

        self.send_transaction_resubmit_job(&tx_to_process).await?;
        Ok(tx_to_process)
    }

    /// Handles NOOP transaction processing before resubmission
    async fn process_noop_transaction(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!("Preparing transaction NOOP before resubmission: {}", tx.id);
        let update = self.prepare_noop_update_request(tx, false).await?;
        let updated_tx = self
            .transaction_repository()
            .partial_update(tx.id.clone(), update)
            .await?;

        self.send_transaction_update_notification(&updated_tx)
            .await?;
        Ok(updated_tx)
    }

    /// Handles transactions in the Pending state.
    async fn handle_pending_state(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        if self.should_noop(&tx).await? {
            info!("Preparing NOOP for pending transaction: {}", tx.id);
            let update = self.prepare_noop_update_request(&tx, false).await?;
            let updated_tx = self
                .transaction_repository()
                .partial_update(tx.id.clone(), update)
                .await?;

            self.send_transaction_submit_job(&updated_tx).await?;
            self.send_transaction_update_notification(&updated_tx)
                .await?;
            return Ok(updated_tx);
        } else {
            self.schedule_status_check(&tx, Some(5)).await?;
        }
        Ok(tx)
    }

    /// Handles transactions in the Mined state.
    async fn handle_mined_state(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        self.schedule_status_check(&tx, Some(5)).await?;
        self.update_transaction_status_if_needed(tx, TransactionStatus::Mined)
            .await
    }

    /// Handles transactions in final states (Confirmed, Failed, Expired).
    async fn handle_final_state(
        &self,
        tx: TransactionRepoModel,
        status: TransactionStatus,
    ) -> Result<TransactionRepoModel, TransactionError> {
        self.update_transaction_status_if_needed(tx, status).await
    }

    /// Inherent status-handling method.
    ///
    /// This method encapsulates the full logic for handling transaction status,
    /// including resubmission, NOOP replacement, and updating status.
    pub async fn handle_status_impl(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!("Checking transaction status for tx: {:?}", tx.id);

        let status = self.check_transaction_status(&tx).await?;
        info!("Transaction status: {:?}", status);

        match status {
            TransactionStatus::Submitted => self.handle_submitted_state(tx).await,
            TransactionStatus::Pending => self.handle_pending_state(tx).await,
            TransactionStatus::Mined => self.handle_mined_state(tx).await,
            TransactionStatus::Confirmed
            | TransactionStatus::Failed
            | TransactionStatus::Expired => self.handle_final_state(tx, status).await,
            _ => Err(TransactionError::UnexpectedError(format!(
                "Unexpected transaction status: {:?}",
                status
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        config::{EvmNetworkConfig, NetworkConfigCommon},
        domain::transaction::evm::{EvmRelayerTransaction, MockPriceCalculatorTrait},
        jobs::MockJobProducerTrait,
        models::{
            evm::Speed, EvmTransactionData, NetworkConfigData, NetworkRepoModel,
            NetworkTransactionData, NetworkType, RelayerEvmPolicy, RelayerNetworkPolicy,
            RelayerRepoModel, TransactionRepoModel, TransactionStatus, U256,
        },
        repositories::{
            MockNetworkRepository, MockRelayerRepository, MockTransactionCounterTrait,
            MockTransactionRepository,
        },
        services::{MockEvmProviderTrait, MockSigner},
    };
    use alloy::{
        consensus::{Eip658Value, Receipt, ReceiptEnvelope, ReceiptWithBloom},
        primitives::{b256, Address, BlockHash, Bloom, TxHash},
        rpc::types::TransactionReceipt,
    };
    use chrono::{Duration, Utc};
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

    /// Returns a `TestMocks` with network repository configured for prepare_noop_update_request tests.
    pub fn default_test_mocks_with_network() -> TestMocks {
        let mut mocks = default_test_mocks();
        // Set up default expectation for get_by_chain_id that prepare_noop_update_request tests need
        mocks
            .network_repo
            .expect_get_by_chain_id()
            .returning(|network_type, chain_id| {
                if network_type == NetworkType::Evm && chain_id == 1 {
                    Ok(Some(create_test_network_model()))
                } else {
                    Ok(None)
                }
            });
        mocks
    }

    /// Creates a test NetworkRepoModel for chain_id 1 (mainnet)
    pub fn create_test_network_model() -> NetworkRepoModel {
        let evm_config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "mainnet".to_string(),
                from: None,
                rpc_urls: Some(vec!["https://rpc.example.com".to_string()]),
                explorer_urls: Some(vec!["https://explorer.example.com".to_string()]),
                average_blocktime_ms: Some(12000),
                is_testnet: Some(false),
                tags: Some(vec!["mainnet".to_string()]),
            },
            chain_id: Some(1),
            required_confirmations: Some(12),
            features: Some(vec!["eip1559".to_string()]),
            symbol: Some("ETH".to_string()),
        };
        NetworkRepoModel {
            id: "evm:mainnet".to_string(),
            name: "mainnet".to_string(),
            network_type: NetworkType::Evm,
            config: NetworkConfigData::Evm(evm_config),
        }
    }

    /// Creates a test NetworkRepoModel for chain_id 42161 (Arbitrum-like) with no-mempool tag
    pub fn create_test_no_mempool_network_model() -> NetworkRepoModel {
        let evm_config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "arbitrum".to_string(),
                from: None,
                rpc_urls: Some(vec!["https://arb-rpc.example.com".to_string()]),
                explorer_urls: Some(vec!["https://arb-explorer.example.com".to_string()]),
                average_blocktime_ms: Some(1000),
                is_testnet: Some(false),
                tags: Some(vec!["arbitrum".to_string(), "no-mempool".to_string()]),
            },
            chain_id: Some(42161),
            required_confirmations: Some(12),
            features: Some(vec!["eip1559".to_string()]),
            symbol: Some("ETH".to_string()),
        };
        NetworkRepoModel {
            id: "evm:arbitrum".to_string(),
            name: "arbitrum".to_string(),
            network_type: NetworkType::Evm,
            config: NetworkConfigData::Evm(evm_config),
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
            delete_at: None,
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

    fn create_test_relayer() -> RelayerRepoModel {
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

    fn make_mock_receipt(status: bool, block_number: Option<u64>) -> TransactionReceipt {
        // Use some placeholder values for minimal completeness
        let tx_hash = TxHash::from(b256!(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ));
        let block_hash = BlockHash::from(b256!(
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        ));
        let from_address = Address::from([0x11; 20]);

        TransactionReceipt {
            // A default, minimal "Legacy" receipt envelope
            inner: ReceiptEnvelope::Legacy(ReceiptWithBloom {
                receipt: Receipt {
                    status: Eip658Value::Eip658(status), // determines success/fail
                    cumulative_gas_used: 0,
                    logs: vec![],
                },
                logs_bloom: Bloom::ZERO,
            }),
            transaction_hash: tx_hash,
            transaction_index: Some(0),
            block_hash: block_number.map(|_| block_hash), // only set if mined
            block_number,
            gas_used: 21000,
            effective_gas_price: 1000,
            blob_gas_used: None,
            blob_gas_price: None,
            from: from_address,
            to: None,
            contract_address: None,
            authorization_list: None,
        }
    }

    // Tests for `check_transaction_status`
    mod check_transaction_status_tests {
        use super::*;

        #[tokio::test]
        async fn test_not_mined() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);

            // Provide a hash so we can check for receipt
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }

            // Mock that get_transaction_receipt returns None (not mined)
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(None) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();
            assert_eq!(status, TransactionStatus::Submitted);
        }

        #[tokio::test]
        async fn test_mined_but_not_confirmed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);

            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }

            // Mock a mined receipt with block_number = 100
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(Some(make_mock_receipt(true, Some(100)))) }));

            // Mock block_number that hasn't reached the confirmation threshold
            mocks
                .provider
                .expect_get_block_number()
                .return_once(|| Box::pin(async { Ok(100) }));

            // Mock network repository to return a test network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();
            assert_eq!(status, TransactionStatus::Mined);
        }

        #[tokio::test]
        async fn test_confirmed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);

            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }

            // Mock a mined receipt with block_number = 100
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(Some(make_mock_receipt(true, Some(100)))) }));

            // Mock block_number that meets the confirmation threshold
            mocks
                .provider
                .expect_get_block_number()
                .return_once(|| Box::pin(async { Ok(113) }));

            // Mock network repository to return a test network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();
            assert_eq!(status, TransactionStatus::Confirmed);
        }

        #[tokio::test]
        async fn test_failed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);

            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }

            // Mock a mined receipt with failure
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(Some(make_mock_receipt(false, Some(100)))) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();
            assert_eq!(status, TransactionStatus::Failed);
        }
    }

    // Tests for `should_resubmit`
    mod should_resubmit_tests {
        use super::*;
        use crate::models::TransactionError;

        #[tokio::test]
        async fn test_should_resubmit_true() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            // Set sent_at to 600 seconds ago to force resubmission
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(600)).to_rfc3339());

            // Mock network repository to return a regular network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let res = evm_transaction.should_resubmit(&tx).await.unwrap();
            assert!(res, "Transaction should be resubmitted after timeout.");
        }

        #[tokio::test]
        async fn test_should_resubmit_false() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            // Make a transaction with status Submitted but recently sent
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some(Utc::now().to_rfc3339());

            // Mock network repository to return a regular network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let res = evm_transaction.should_resubmit(&tx).await.unwrap();
            assert!(!res, "Transaction should not be resubmitted immediately.");
        }

        #[tokio::test]
        async fn test_should_resubmit_true_for_no_mempool_network() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            // Set up a transaction that would normally be resubmitted (sent_at long ago)
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(600)).to_rfc3339());

            // Set chain_id to match the no-mempool network
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.chain_id = 42161; // Arbitrum chain ID
            }

            // Mock network repository to return a no-mempool network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_no_mempool_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let res = evm_transaction.should_resubmit(&tx).await.unwrap();
            assert!(
                res,
                "Transaction should be resubmitted for no-mempool networks."
            );
        }

        #[tokio::test]
        async fn test_should_resubmit_network_not_found() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(600)).to_rfc3339());

            // Mock network repository to return None (network not found)
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(None));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.should_resubmit(&tx).await;

            assert!(
                result.is_err(),
                "should_resubmit should return error when network not found"
            );
            let error = result.unwrap_err();
            match error {
                TransactionError::UnexpectedError(msg) => {
                    assert!(msg.contains("Network with chain id 1 not found"));
                }
                _ => panic!("Expected UnexpectedError for network not found"),
            }
        }

        #[tokio::test]
        async fn test_should_resubmit_network_conversion_error() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(600)).to_rfc3339());

            // Create a network model with invalid EVM config (missing chain_id)
            let invalid_evm_config = EvmNetworkConfig {
                common: NetworkConfigCommon {
                    network: "invalid-network".to_string(),
                    from: None,
                    rpc_urls: Some(vec!["https://rpc.example.com".to_string()]),
                    explorer_urls: Some(vec!["https://explorer.example.com".to_string()]),
                    average_blocktime_ms: Some(12000),
                    is_testnet: Some(false),
                    tags: Some(vec!["testnet".to_string()]),
                },
                chain_id: None, // This will cause the conversion to fail
                required_confirmations: Some(12),
                features: Some(vec!["eip1559".to_string()]),
                symbol: Some("ETH".to_string()),
            };
            let invalid_network = NetworkRepoModel {
                id: "evm:invalid".to_string(),
                name: "invalid-network".to_string(),
                network_type: NetworkType::Evm,
                config: NetworkConfigData::Evm(invalid_evm_config),
            };

            // Mock network repository to return the invalid network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(move |_, _| Ok(Some(invalid_network.clone())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.should_resubmit(&tx).await;

            assert!(
                result.is_err(),
                "should_resubmit should return error when network conversion fails"
            );
            let error = result.unwrap_err();
            match error {
                TransactionError::UnexpectedError(msg) => {
                    assert!(msg.contains("Error converting network model to EvmNetwork"));
                }
                _ => panic!("Expected UnexpectedError for network conversion failure"),
            }
        }
    }

    // Tests for `should_noop`
    mod should_noop_tests {
        use super::*;

        #[tokio::test]
        async fn test_expired_transaction_triggers_noop() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            // Force the transaction to be "expired" by setting valid_until in the past
            tx.valid_until = Some((Utc::now() - Duration::seconds(10)).to_rfc3339());

            // Mock network repository to return a test network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let res = evm_transaction.should_noop(&tx).await.unwrap();
            assert!(res, "Expired transaction should be replaced with a NOOP.");
        }
    }

    // Tests for `update_transaction_status_if_needed`
    mod update_transaction_status_tests {
        use super::*;

        #[tokio::test]
        async fn test_no_update_when_status_is_same() {
            // Create mocks, relayer, and a transaction with status Submitted.
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let tx = make_test_transaction(TransactionStatus::Submitted);
            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            // When new status is the same as current, update_transaction_status_if_needed
            // should simply return the original transaction.
            let updated_tx = evm_transaction
                .update_transaction_status_if_needed(tx.clone(), TransactionStatus::Submitted)
                .await
                .unwrap();
            assert_eq!(updated_tx.status, TransactionStatus::Submitted);
            assert_eq!(updated_tx.id, tx.id);
        }
    }

    // Tests for `prepare_noop_update_request`
    mod prepare_noop_update_request_tests {
        use super::*;

        #[tokio::test]
        async fn test_noop_request_without_cancellation() {
            // Create a transaction with an initial noop_count of 2 and is_canceled set to false.
            let mocks = default_test_mocks_with_network();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.noop_count = Some(2);
            tx.is_canceled = Some(false);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let update_req = evm_transaction
                .prepare_noop_update_request(&tx, false)
                .await
                .unwrap();

            // NOOP count should be incremented: 2 becomes 3.
            assert_eq!(update_req.noop_count, Some(3));
            // When not cancelling, the is_canceled flag should remain as in the original transaction.
            assert_eq!(update_req.is_canceled, Some(false));
        }

        #[tokio::test]
        async fn test_noop_request_with_cancellation() {
            // Create a transaction with no initial noop_count (None) and is_canceled false.
            let mocks = default_test_mocks_with_network();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.noop_count = None;
            tx.is_canceled = Some(false);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let update_req = evm_transaction
                .prepare_noop_update_request(&tx, true)
                .await
                .unwrap();

            // NOOP count should default to 1.
            assert_eq!(update_req.noop_count, Some(1));
            // When cancelling, the is_canceled flag should be forced to true.
            assert_eq!(update_req.is_canceled, Some(true));
        }
    }

    // Tests for `handle_submitted_state`
    mod handle_submitted_state_tests {
        use super::*;

        #[tokio::test]
        async fn test_schedules_resubmit_job() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            // Set sent_at far in the past to force resubmission
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(600)).to_rfc3339());

            // Mock network repository to return a test network model for should_noop check
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            // Expect the resubmit job to be produced
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Expect status check to be scheduled
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let updated_tx = evm_transaction.handle_submitted_state(tx).await.unwrap();

            // We remain in "Submitted" after scheduling the resubmit
            assert_eq!(updated_tx.status, TransactionStatus::Submitted);
        }
    }

    // Tests for `handle_pending_state`
    mod handle_pending_state_tests {
        use super::*;

        #[tokio::test]
        async fn test_pending_state_no_noop() {
            // Create a pending transaction that is fresh (created now).
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Pending);
            tx.created_at = Utc::now().to_rfc3339(); // less than one minute old

            // Mock network repository to return a test network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            // Expect status check to be scheduled when not doing NOOP
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .handle_pending_state(tx.clone())
                .await
                .unwrap();

            // When should_noop returns false the original transaction is returned unchanged.
            assert_eq!(result.id, tx.id);
            assert_eq!(result.status, tx.status);
            assert_eq!(result.noop_count, tx.noop_count);
        }

        #[tokio::test]
        async fn test_pending_state_with_noop() {
            // Create a pending transaction that is old (created 2 minutes ago)
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Pending);
            tx.created_at = (Utc::now() - Duration::minutes(2)).to_rfc3339();

            // Mock network repository to return a test network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            // Expect partial_update to be called and simulate a NOOP update by setting noop_count.
            let tx_clone = tx.clone();
            mocks
                .tx_repo
                .expect_partial_update()
                .returning(move |_, update| {
                    let mut updated_tx = tx_clone.clone();
                    updated_tx.noop_count = update.noop_count;
                    Ok(updated_tx)
                });
            // Expect that a submit job and notification are produced.
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .handle_pending_state(tx.clone())
                .await
                .unwrap();

            // Since should_noop returns true, the returned transaction should have a nonzero noop_count.
            assert!(result.noop_count.unwrap_or(0) > 0);
        }
    }

    // Tests for `handle_mined_state`
    mod handle_mined_state_tests {
        use super::*;

        #[tokio::test]
        async fn test_updates_status_and_schedules_check() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            // Create a transaction in Submitted state (the mined branch is reached via status check).
            let tx = make_test_transaction(TransactionStatus::Submitted);

            // Expect schedule_status_check to be called with delay 5.
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            // Expect partial_update to update the transaction status to Mined.
            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .handle_mined_state(tx.clone())
                .await
                .unwrap();
            assert_eq!(result.status, TransactionStatus::Mined);
        }
    }

    // Tests for `handle_final_state`
    mod handle_final_state_tests {
        use super::*;

        #[tokio::test]
        async fn test_final_state_confirmed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let tx = make_test_transaction(TransactionStatus::Submitted);

            // Expect partial_update to update status to Confirmed.
            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .handle_final_state(tx.clone(), TransactionStatus::Confirmed)
                .await
                .unwrap();
            assert_eq!(result.status, TransactionStatus::Confirmed);
        }

        #[tokio::test]
        async fn test_final_state_failed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let tx = make_test_transaction(TransactionStatus::Submitted);

            // Expect partial_update to update status to Failed.
            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .handle_final_state(tx.clone(), TransactionStatus::Failed)
                .await
                .unwrap();
            assert_eq!(result.status, TransactionStatus::Failed);
        }

        #[tokio::test]
        async fn test_final_state_expired() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let tx = make_test_transaction(TransactionStatus::Submitted);

            // Expect partial_update to update status to Expired.
            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .handle_final_state(tx.clone(), TransactionStatus::Expired)
                .await
                .unwrap();
            assert_eq!(result.status, TransactionStatus::Expired);
        }
    }

    // Integration tests for `handle_status_impl`
    mod handle_status_impl_tests {
        use super::*;

        #[tokio::test]
        async fn test_impl_submitted_branch() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(120)).to_rfc3339());
            // Set a dummy hash so check_transaction_status can proceed.
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }
            // Simulate no receipt found.
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(None) }));
            // Mock network repository for should_resubmit check
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));
            // Expect that a status check job is scheduled.
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            // Expect update_transaction_status_if_needed to update status to Submitted.
            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Submitted);
        }

        #[tokio::test]
        async fn test_impl_mined_branch() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            // Set a dummy hash.
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }
            // Simulate a receipt with a block number of 100 and a successful receipt.
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(Some(make_mock_receipt(true, Some(100)))) }));
            // Simulate that the current block number is 100 (so confirmations are insufficient).
            mocks
                .provider
                .expect_get_block_number()
                .return_once(|| Box::pin(async { Ok(100) }));
            // Mock network repository to return a test network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));
            // Expect a status check job to be scheduled.
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            // Expect update_transaction_status_if_needed to update status to Mined.
            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Mined);
        }

        #[tokio::test]
        async fn test_impl_final_confirmed_branch() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            // Create a transaction with status Confirmed.
            let tx = make_test_transaction(TransactionStatus::Confirmed);

            // In this branch, check_transaction_status returns the final status immediately,
            // so we expect partial_update to update the transaction status to Confirmed.
            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Confirmed);
        }

        #[tokio::test]
        async fn test_impl_final_failed_branch() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            // Create a transaction with status Failed.
            let tx = make_test_transaction(TransactionStatus::Failed);

            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Failed);
        }

        #[tokio::test]
        async fn test_impl_final_expired_branch() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            // Create a transaction with status Expired.
            let tx = make_test_transaction(TransactionStatus::Expired);

            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Expired);
        }
    }
}
