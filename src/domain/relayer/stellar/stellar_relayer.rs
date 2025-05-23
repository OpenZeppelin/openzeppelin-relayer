/// This module defines the `StellarRelayer` struct and its associated functionality for
/// interacting with Stellar networks. The `StellarRelayer` is responsible for managing
/// transactions, synchronizing sequence numbers, and ensuring the relayer's state is
/// consistent with the Stellar blockchain.
///
/// # Components
///
/// - `StellarRelayer`: The main struct that encapsulates the relayer's state and operations for Stellar.
/// - `RelayerRepoModel`: Represents the relayer's data model.
/// - `StellarProvider`: Provides blockchain interaction capabilities, such as fetching account details.
/// - `TransactionCounterService`: Manages the sequence number for transactions to ensure correct ordering.
/// - `JobProducer`: Produces jobs for processing transactions and sending notifications.
///
/// # Error Handling
///
/// The module uses the `RelayerError` enum to handle various errors that can occur during
/// operations, such as provider errors, sequence synchronization failures, and transaction failures.
///
/// # Usage
///
/// To use the `StellarRelayer`, create an instance using the `new` method, providing the necessary
/// components. Then, call the appropriate methods to process transactions and manage the relayer's state.
use crate::{
    constants::STELLAR_SMALLEST_UNIT_NAME,
    domain::{
        next_sequence_u64, BalanceResponse, JsonRpcRequest, JsonRpcResponse, SignDataRequest,
        SignDataResponse, SignTypedDataRequest,
    },
    jobs::{JobProducer, JobProducerTrait, TransactionRequest},
    models::{
        produce_relayer_disabled_payload, NetworkRpcRequest, NetworkRpcResult, NetworkSpecificData,
        NetworkTransactionRequest, RelayerRepoModel, RelayerStatus, RepositoryError,
        StellarNetwork, StellarRpcResult, StellarStatusData, TransactionRepoModel,
        TransactionStatus,
    },
    repositories::{
        InMemoryRelayerRepository, InMemoryTransactionCounter, InMemoryTransactionRepository,
        RelayerRepository, RelayerRepositoryStorage, Repository, TransactionRepository,
    },
    services::{
        StellarProvider, StellarProviderTrait, TransactionCounterService,
        TransactionCounterServiceTrait,
    },
};
use async_trait::async_trait;
use eyre::Result;
use log::{info, warn};
use std::sync::Arc;

use crate::domain::relayer::{Relayer, RelayerError};

#[allow(dead_code)]
pub struct StellarRelayer<P, R, T, J, C>
where
    P: StellarProviderTrait + Send + Sync,
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    C: TransactionCounterServiceTrait + Send + Sync,
{
    relayer: RelayerRepoModel,
    network: StellarNetwork,
    provider: P,
    relayer_repository: Arc<R>,
    transaction_repository: Arc<T>,
    transaction_counter_service: Arc<C>,
    job_producer: Arc<J>,
}

pub type DefaultStellarRelayer = StellarRelayer<
    StellarProvider,
    RelayerRepositoryStorage<InMemoryRelayerRepository>,
    InMemoryTransactionRepository,
    JobProducer,
    TransactionCounterService<InMemoryTransactionCounter>,
>;

impl<P, R, T, J, C> StellarRelayer<P, R, T, J, C>
where
    P: StellarProviderTrait + Send + Sync,
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    C: TransactionCounterServiceTrait + Send + Sync,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        relayer: RelayerRepoModel,
        provider: P,
        relayer_repository: Arc<R>,
        transaction_repository: Arc<T>,
        transaction_counter_service: Arc<C>,
        job_producer: Arc<J>,
    ) -> Result<Self, RelayerError> {
        let network = match StellarNetwork::from_network_str(&relayer.network) {
            Ok(network) => network,
            Err(e) => return Err(RelayerError::NetworkConfiguration(e.to_string())),
        };

        Ok(Self {
            relayer,
            network,
            provider,
            relayer_repository,
            transaction_repository,
            transaction_counter_service,
            job_producer,
        })
    }

    async fn sync_sequence(&self) -> Result<(), RelayerError> {
        info!(
            "Fetching sequence for relayer: {} ({})",
            self.relayer.id, self.relayer.address
        );

        let account_entry = self
            .provider
            .get_account(&self.relayer.address)
            .await
            .map_err(|e| RelayerError::ProviderError(format!("Failed to fetch account: {}", e)))?;

        let next = next_sequence_u64(account_entry.seq_num.0)?;

        info!(
            "Setting next sequence {} for relayer {}",
            next, self.relayer.id
        );
        self.transaction_counter_service
            .set(next)
            .await
            .map_err(RelayerError::from)?;
        Ok(())
    }

    async fn disable_relayer(&self, reasons: &[String]) -> Result<(), RelayerError> {
        let reason = reasons.join(", ");
        warn!("Disabling relayer {} due to: {}", self.relayer.id, reason);

        let updated = self
            .relayer_repository
            .disable_relayer(self.relayer.id.clone())
            .await?;

        if let Some(nid) = &self.relayer.notification_id {
            self.job_producer
                .produce_send_notification_job(
                    produce_relayer_disabled_payload(nid, &updated, &reason),
                    None,
                )
                .await?;
        }
        Ok(())
    }
}

#[async_trait]
impl<P, R, T, J, C> Relayer for StellarRelayer<P, R, T, J, C>
where
    P: StellarProviderTrait + Send + Sync,
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    C: TransactionCounterServiceTrait + Send + Sync,
{
    async fn process_transaction_request(
        &self,
        network_transaction: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError> {
        let transaction = TransactionRepoModel::try_from((&network_transaction, &self.relayer))?;

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

        Ok(transaction)
    }

    async fn get_balance(&self) -> Result<BalanceResponse, RelayerError> {
        let account_entry = self
            .provider
            .get_account(&self.relayer.address)
            .await
            .map_err(|e| {
                RelayerError::ProviderError(format!("Failed to fetch account for balance: {}", e))
            })?;

        Ok(BalanceResponse {
            balance: account_entry.balance as u128,
            unit: STELLAR_SMALLEST_UNIT_NAME.to_string(),
        })
    }

    async fn get_status(&self) -> Result<RelayerStatus, RelayerError> {
        let relayer_model = &self.relayer;

        let account_entry = self
            .provider
            .get_account(&relayer_model.address)
            .await
            .map_err(|e| {
                RelayerError::ProviderError(format!("Failed to get account details: {}", e))
            })?;

        let sequence_number_str = account_entry.seq_num.0.to_string();

        let balance_response = self.get_balance().await?;

        let pending_statuses = [TransactionStatus::Pending, TransactionStatus::Submitted];
        let pending_transactions = self
            .transaction_repository
            .find_by_status(&relayer_model.id, &pending_statuses[..])
            .await
            .map_err(RelayerError::from)?;
        let pending_transactions_count = pending_transactions.len() as u64;

        let confirmed_statuses = [TransactionStatus::Confirmed];
        let confirmed_transactions = self
            .transaction_repository
            .find_by_status(&relayer_model.id, &confirmed_statuses[..])
            .await
            .map_err(RelayerError::from)?;

        let last_confirmed_transaction_timestamp = confirmed_transactions
            .iter()
            .filter_map(|tx| tx.confirmed_at.as_ref())
            .max()
            .cloned();

        Ok(RelayerStatus {
            balance: balance_response.balance.to_string(),
            pending_transactions_count,
            last_confirmed_transaction_timestamp,
            system_disabled: relayer_model.system_disabled,
            paused: relayer_model.paused,
            network_specific: NetworkSpecificData::Stellar(StellarStatusData {
                sequence_number: sequence_number_str,
            }),
        })
    }

    async fn delete_pending_transactions(&self) -> Result<bool, RelayerError> {
        println!("Stellar delete_pending_transactions...");
        Ok(true)
    }

    async fn sign_data(&self, _request: SignDataRequest) -> Result<SignDataResponse, RelayerError> {
        Err(RelayerError::NotSupported(
            "Signing data not supported for Stellar".to_string(),
        ))
    }

    async fn sign_typed_data(
        &self,
        _request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, RelayerError> {
        Err(RelayerError::NotSupported(
            "Signing typed data not supported for Stellar".to_string(),
        ))
    }

    async fn rpc(
        &self,
        _request: JsonRpcRequest<NetworkRpcRequest>,
    ) -> Result<JsonRpcResponse<NetworkRpcResult>, RelayerError> {
        println!("Stellar rpc...");
        Ok(JsonRpcResponse {
            id: Some(1),
            jsonrpc: "2.0".to_string(),
            result: Some(NetworkRpcResult::Stellar(
                StellarRpcResult::GenericRpcResult("".to_string()),
            )),
            error: None,
        })
    }

    async fn validate_min_balance(&self) -> Result<(), RelayerError> {
        Ok(())
    }

    async fn initialize_relayer(&self) -> Result<(), RelayerError> {
        info!("Initializing Stellar relayer: {}", self.relayer.id);

        let seq_res = self.sync_sequence().await.err();

        let mut failures: Vec<String> = Vec::new();
        if let Some(e) = seq_res {
            failures.push(format!("Sequence sync failed: {}", e));
        }

        if !failures.is_empty() {
            self.disable_relayer(&failures).await?;
            return Ok(()); // same semantics as EVM
        }

        info!(
            "Stellar relayer initialized successfully: {}",
            self.relayer.id
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::NetworkType;
    use crate::{
        constants::STELLAR_SMALLEST_UNIT_NAME,
        jobs::MockJobProducerTrait,
        models::{RelayerNetworkPolicy, RelayerRepoModel, RelayerStellarPolicy},
        repositories::{MockRelayerRepository, MockTransactionRepository},
        services::{MockStellarProviderTrait, MockTransactionCounterServiceTrait},
    };
    use eyre::eyre;
    use mockall::predicate::*;
    use soroban_rs::xdr::{
        AccountEntry, AccountEntryExt, AccountId, PublicKey, SequenceNumber, String32, Thresholds,
        Uint256, VecM,
    };
    use std::future::ready;
    use std::sync::Arc;

    fn create_test_relayer_model() -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test-relayer-id".to_string(),
            name: "Test Relayer".to_string(),
            network: "testnet".to_string(),
            paused: false,
            network_type: NetworkType::Stellar,
            signer_id: "signer-id".to_string(),
            policies: RelayerNetworkPolicy::Stellar(RelayerStellarPolicy::default()),
            address: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            notification_id: Some("notification-id".to_string()),
            system_disabled: false,
            custom_rpc_urls: None,
        }
    }

    #[tokio::test]
    async fn test_sync_sequence_success() {
        let relayer_model = create_test_relayer_model();
        let mut provider = MockStellarProviderTrait::new();
        provider
            .expect_get_account()
            .with(eq(relayer_model.address.clone()))
            .returning(|_| {
                Box::pin(async {
                    Ok(AccountEntry {
                        account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                        balance: 0,
                        ext: AccountEntryExt::V0,
                        flags: 0,
                        home_domain: String32::default(),
                        inflation_dest: None,
                        seq_num: SequenceNumber(5),
                        num_sub_entries: 0,
                        signers: VecM::default(),
                        thresholds: Thresholds([0, 0, 0, 0]),
                    })
                })
            });
        let mut counter = MockTransactionCounterServiceTrait::new();
        counter
            .expect_set()
            .with(eq(6u64))
            .returning(|_| Box::pin(async { Ok(()) }));
        let relayer_repo = MockRelayerRepository::new();
        let tx_repo = MockTransactionRepository::new();
        let job_producer = MockJobProducerTrait::new();

        let relayer = StellarRelayer::new(
            relayer_model.clone(),
            provider,
            Arc::new(relayer_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.sync_sequence().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sync_sequence_provider_error() {
        let relayer_model = create_test_relayer_model();
        let mut provider = MockStellarProviderTrait::new();
        provider
            .expect_get_account()
            .with(eq(relayer_model.address.clone()))
            .returning(|_| Box::pin(async { Err(eyre!("fail")) }));
        let counter = MockTransactionCounterServiceTrait::new();
        let relayer_repo = MockRelayerRepository::new();
        let tx_repo = MockTransactionRepository::new();
        let job_producer = MockJobProducerTrait::new();

        let relayer = StellarRelayer::new(
            relayer_model.clone(),
            provider,
            Arc::new(relayer_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.sync_sequence().await;
        assert!(matches!(result, Err(RelayerError::ProviderError(_))));
    }

    #[tokio::test]
    async fn test_disable_relayer() {
        let relayer_model = create_test_relayer_model();
        let provider = MockStellarProviderTrait::new();
        let mut relayer_repo = MockRelayerRepository::new();
        let mut updated_model = relayer_model.clone();
        updated_model.system_disabled = true;
        relayer_repo
            .expect_disable_relayer()
            .with(eq(relayer_model.id.clone()))
            .returning(move |_| Ok::<RelayerRepoModel, RepositoryError>(updated_model.clone()));
        let mut job_producer = MockJobProducerTrait::new();
        job_producer
            .expect_produce_send_notification_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));
        let tx_repo = MockTransactionRepository::new();
        let counter = MockTransactionCounterServiceTrait::new();

        let relayer = StellarRelayer::new(
            relayer_model.clone(),
            provider,
            Arc::new(relayer_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let reasons = vec!["reason1".to_string(), "reason2".to_string()];
        let result = relayer.disable_relayer(&reasons).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_status_success_stellar() {
        let relayer_model = create_test_relayer_model();
        let mut provider_mock = MockStellarProviderTrait::new();
        let mut tx_repo_mock = MockTransactionRepository::new();
        let relayer_repo_mock = MockRelayerRepository::new();
        let job_producer_mock = MockJobProducerTrait::new();
        let counter_mock = MockTransactionCounterServiceTrait::new();

        provider_mock.expect_get_account().times(2).returning(|_| {
            Box::pin(ready(Ok(AccountEntry {
                account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                balance: 10000000,
                seq_num: SequenceNumber(12345),
                ext: AccountEntryExt::V0,
                flags: 0,
                home_domain: String32::default(),
                inflation_dest: None,
                num_sub_entries: 0,
                signers: VecM::default(),
                thresholds: Thresholds([0, 0, 0, 0]),
            })))
        });

        tx_repo_mock
            .expect_find_by_status()
            .withf(|relayer_id, statuses| {
                relayer_id == "test-relayer-id"
                    && statuses == [TransactionStatus::Pending, TransactionStatus::Submitted]
            })
            .returning(|_, _| Ok(vec![]) as Result<Vec<TransactionRepoModel>, RepositoryError>)
            .once();

        let confirmed_tx = TransactionRepoModel {
            id: "tx1_stellar".to_string(),
            relayer_id: relayer_model.id.clone(),
            status: TransactionStatus::Confirmed,
            confirmed_at: Some("2023-02-01T12:00:00Z".to_string()),
            ..TransactionRepoModel::default()
        };
        tx_repo_mock
            .expect_find_by_status()
            .withf(|relayer_id, statuses| {
                relayer_id == "test-relayer-id" && statuses == [TransactionStatus::Confirmed]
            })
            .returning(move |_, _| {
                Ok(vec![confirmed_tx.clone()]) as Result<Vec<TransactionRepoModel>, RepositoryError>
            })
            .once();

        let stellar_relayer = StellarRelayer::new(
            relayer_model.clone(),
            provider_mock,
            Arc::new(relayer_repo_mock),
            Arc::new(tx_repo_mock),
            Arc::new(counter_mock),
            Arc::new(job_producer_mock),
        )
        .unwrap();

        let status = stellar_relayer.get_status().await.unwrap();

        assert_eq!(status.balance, "10000000");
        assert_eq!(status.pending_transactions_count, 0);
        assert_eq!(
            status.last_confirmed_transaction_timestamp,
            Some("2023-02-01T12:00:00Z".to_string())
        );
        assert_eq!(status.system_disabled, relayer_model.system_disabled);
        assert_eq!(status.paused, relayer_model.paused);
        match status.network_specific {
            NetworkSpecificData::Stellar(stellar_data) => {
                assert_eq!(stellar_data.sequence_number, "12345");
            }
            _ => panic!("Expected Stellar specific data"),
        }
    }

    #[tokio::test]
    async fn test_get_status_stellar_provider_error() {
        let relayer_model = create_test_relayer_model();
        let mut provider_mock = MockStellarProviderTrait::new();
        let tx_repo_mock = MockTransactionRepository::new();
        let relayer_repo_mock = MockRelayerRepository::new();
        let job_producer_mock = MockJobProducerTrait::new();
        let counter_mock = MockTransactionCounterServiceTrait::new();

        provider_mock
            .expect_get_account()
            .with(eq(relayer_model.address.clone()))
            .returning(|_| Box::pin(async { Err(eyre!("Stellar provider down")) }));

        let stellar_relayer = StellarRelayer::new(
            relayer_model.clone(),
            provider_mock,
            Arc::new(relayer_repo_mock),
            Arc::new(tx_repo_mock),
            Arc::new(counter_mock),
            Arc::new(job_producer_mock),
        )
        .unwrap();

        let result = stellar_relayer.get_status().await;
        assert!(result.is_err());
        match result.err().unwrap() {
            RelayerError::ProviderError(msg) => {
                assert!(msg.contains("Failed to get account details"))
            }
            _ => panic!("Expected ProviderError for get_account failure"),
        }
    }

    #[tokio::test]
    async fn test_get_balance_success() {
        let relayer_model = create_test_relayer_model();
        let mut provider = MockStellarProviderTrait::new();
        let expected_balance = 100_000_000i64; // 10 XLM in stroops

        provider
            .expect_get_account()
            .with(eq(relayer_model.address.clone()))
            .returning(move |_| {
                Box::pin(async move {
                    Ok(AccountEntry {
                        account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
                        balance: expected_balance,
                        ext: AccountEntryExt::V0,
                        flags: 0,
                        home_domain: String32::default(),
                        inflation_dest: None,
                        seq_num: SequenceNumber(5),
                        num_sub_entries: 0,
                        signers: VecM::default(),
                        thresholds: Thresholds([0, 0, 0, 0]),
                    })
                })
            });

        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());
        let counter = Arc::new(MockTransactionCounterServiceTrait::new());

        let relayer = StellarRelayer::new(
            relayer_model,
            provider,
            relayer_repo,
            tx_repo,
            counter,
            job_producer,
        )
        .unwrap();

        let result = relayer.get_balance().await;
        assert!(result.is_ok());
        let balance_response = result.unwrap();
        assert_eq!(balance_response.balance, expected_balance as u128);
        assert_eq!(balance_response.unit, STELLAR_SMALLEST_UNIT_NAME);
    }

    #[tokio::test]
    async fn test_get_balance_provider_error() {
        let relayer_model = create_test_relayer_model();
        let mut provider = MockStellarProviderTrait::new();

        provider
            .expect_get_account()
            .with(eq(relayer_model.address.clone()))
            .returning(|_| Box::pin(async { Err(eyre!("provider failed")) }));

        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());
        let counter = Arc::new(MockTransactionCounterServiceTrait::new());

        let relayer = StellarRelayer::new(
            relayer_model,
            provider,
            relayer_repo,
            tx_repo,
            counter,
            job_producer,
        )
        .unwrap();

        let result = relayer.get_balance().await;
        assert!(result.is_err());
        match result.err().unwrap() {
            RelayerError::ProviderError(msg) => {
                assert!(msg.contains("Failed to fetch account for balance: provider failed"));
            }
            _ => panic!("Unexpected error type"),
        }
    }
}
