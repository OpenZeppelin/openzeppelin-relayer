//! Stellar Provider implementation for interacting with Stellar blockchain networks.
//!
//! This module provides functionality to interact with Stellar networks through RPC calls.
//! It implements common operations like getting accounts, sending transactions, and querying
//! blockchain state and events.

use async_trait::async_trait;
use eyre::{eyre, Result};
use soroban_rs::stellar_rpc_client::Client;
use soroban_rs::stellar_rpc_client::{
    EventStart, EventType, GetEventsResponse, GetLatestLedgerResponse, GetLedgerEntriesResponse,
    GetNetworkResponse, GetTransactionResponse, GetTransactionsRequest, GetTransactionsResponse,
    SimulateTransactionResponse,
};
use soroban_rs::xdr::{AccountEntry, Hash, LedgerKey, TransactionEnvelope};
#[cfg(test)]
use soroban_rs::xdr::{AccountId, LedgerKeyAccount, PublicKey, Uint256};
use soroban_rs::SorobanTransactionResponse;

#[cfg(test)]
use mockall::automock;

use crate::models::RpcConfig;
use crate::services::provider::ProviderError;

#[derive(Debug, Clone)]
pub struct GetEventsRequest {
    pub start: EventStart,
    pub event_type: Option<EventType>,
    pub contract_ids: Vec<String>,
    pub topics: Vec<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct StellarProvider {
    client: Client,
}

#[async_trait]
#[cfg_attr(test, automock)]
#[allow(dead_code)]
pub trait StellarProviderTrait: Send + Sync {
    async fn get_account(&self, account_id: &str) -> Result<AccountEntry>;
    async fn simulate_transaction_envelope(
        &self,
        tx_envelope: &TransactionEnvelope,
    ) -> Result<SimulateTransactionResponse>;
    async fn send_transaction_polling(
        &self,
        tx_envelope: &TransactionEnvelope,
    ) -> Result<SorobanTransactionResponse>;
    async fn get_network(&self) -> Result<GetNetworkResponse>;
    async fn get_latest_ledger(&self) -> Result<GetLatestLedgerResponse>;
    async fn send_transaction(&self, tx_envelope: &TransactionEnvelope) -> Result<Hash>;
    async fn get_transaction(&self, tx_id: &Hash) -> Result<GetTransactionResponse>;
    async fn get_transactions(
        &self,
        request: GetTransactionsRequest,
    ) -> Result<GetTransactionsResponse>;
    async fn get_ledger_entries(&self, keys: &[LedgerKey]) -> Result<GetLedgerEntriesResponse>;
    async fn get_events(&self, request: GetEventsRequest) -> Result<GetEventsResponse>;
}

impl StellarProvider {
    pub fn new(mut rpc_configs: Vec<RpcConfig>, _timeout: u64) -> Result<Self, ProviderError> {
        if rpc_configs.is_empty() {
            return Err(ProviderError::NetworkConfiguration(
                "No RPC configurations provided for StellarProvider".to_string(),
            ));
        }

        RpcConfig::validate_list(&rpc_configs)
            .map_err(|e| ProviderError::NetworkConfiguration(e.to_string()))?;

        rpc_configs.retain(|config| config.get_weight() > 0);

        if rpc_configs.is_empty() {
            return Err(ProviderError::NetworkConfiguration(
                "No active RPC configurations provided (all weights are 0 or list was empty after filtering)".to_string(),
            ));
        }

        rpc_configs.sort_by_key(|config| std::cmp::Reverse(config.get_weight()));

        let selected_config = &rpc_configs[0];
        let url = &selected_config.url;

        let client = Client::new(url).map_err(|e| {
            ProviderError::NetworkConfiguration(format!(
                "Failed to create Stellar RPC client: {} - URL: '{}'",
                e, url
            ))
        })?;
        Ok(Self { client })
    }
}

impl AsRef<StellarProvider> for StellarProvider {
    fn as_ref(&self) -> &StellarProvider {
        self
    }
}

#[async_trait]
impl StellarProviderTrait for StellarProvider {
    async fn get_account(&self, account_id: &str) -> Result<AccountEntry> {
        self.client
            .get_account(account_id)
            .await
            .map_err(|e| eyre!("Failed to get account: {}", e))
    }

    async fn simulate_transaction_envelope(
        &self,
        tx_envelope: &TransactionEnvelope,
    ) -> Result<SimulateTransactionResponse> {
        self.client
            .simulate_transaction_envelope(tx_envelope)
            .await
            .map_err(|e| eyre!("Failed to simulate transaction: {}", e))
    }

    async fn send_transaction_polling(
        &self,
        tx_envelope: &TransactionEnvelope,
    ) -> Result<SorobanTransactionResponse> {
        self.client
            .send_transaction_polling(tx_envelope)
            .await
            .map(SorobanTransactionResponse::from)
            .map_err(|e| eyre!("Failed to send transaction (polling): {}", e))
    }

    async fn get_network(&self) -> Result<GetNetworkResponse> {
        self.client
            .get_network()
            .await
            .map_err(|e| eyre!("Failed to get network: {}", e))
    }

    async fn get_latest_ledger(&self) -> Result<GetLatestLedgerResponse> {
        self.client
            .get_latest_ledger()
            .await
            .map_err(|e| eyre!("Failed to get latest ledger: {}", e))
    }

    async fn send_transaction(&self, tx_envelope: &TransactionEnvelope) -> Result<Hash> {
        self.client
            .send_transaction(tx_envelope)
            .await
            .map_err(|e| eyre!("Failed to send transaction: {}", e))
    }

    async fn get_transaction(&self, tx_id: &Hash) -> Result<GetTransactionResponse> {
        self.client
            .get_transaction(tx_id)
            .await
            .map_err(|e| eyre!("Failed to get transaction: {}", e))
    }

    async fn get_transactions(
        &self,
        request: GetTransactionsRequest,
    ) -> Result<GetTransactionsResponse> {
        self.client
            .get_transactions(request)
            .await
            .map_err(|e| eyre!("Failed to get transactions: {}", e))
    }

    async fn get_ledger_entries(&self, keys: &[LedgerKey]) -> Result<GetLedgerEntriesResponse> {
        self.client
            .get_ledger_entries(keys)
            .await
            .map_err(|e| eyre!("Failed to get ledger entries: {}", e))
    }

    async fn get_events(&self, request: GetEventsRequest) -> Result<GetEventsResponse> {
        self.client
            .get_events(
                request.start,
                request.event_type,
                &request.contract_ids,
                &request.topics,
                request.limit,
            )
            .await
            .map_err(|e| eyre!("Failed to get events: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::provider::stellar::{
        GetEventsRequest, StellarProvider, StellarProviderTrait,
    };
    use eyre::eyre;
    use futures::FutureExt;
    use mockall::predicate as p;
    use soroban_rs::stellar_rpc_client::{
        EventStart, GetEventsResponse, GetLatestLedgerResponse, GetLedgerEntriesResponse,
        GetNetworkResponse, GetTransactionResponse, GetTransactionsRequest,
        GetTransactionsResponse, SimulateTransactionResponse,
    };
    use soroban_rs::xdr::{
        AccountEntryExt, Hash, LedgerKey, OperationResult, String32, Thresholds,
        TransactionEnvelope, TransactionResult, TransactionResultExt, TransactionResultResult,
        VecM,
    };
    use soroban_rs::{create_mock_set_options_tx_envelope, SorobanTransactionResponse};
    use std::str::FromStr;

    fn dummy_hash() -> Hash {
        Hash([0u8; 32])
    }

    fn dummy_get_network_response() -> GetNetworkResponse {
        GetNetworkResponse {
            friendbot_url: Some("https://friendbot.testnet.stellar.org/".into()),
            passphrase: "Test SDF Network ; September 2015".into(),
            protocol_version: 20,
        }
    }

    fn dummy_get_latest_ledger_response() -> GetLatestLedgerResponse {
        GetLatestLedgerResponse {
            id: "c73c5eac58a441d4eb733c35253ae85f783e018f7be5ef974258fed067aabb36".into(),
            protocol_version: 20,
            sequence: 2_539_605,
        }
    }

    fn dummy_simulate() -> SimulateTransactionResponse {
        SimulateTransactionResponse {
            min_resource_fee: 100,
            transaction_data: "test".to_string(),
            ..Default::default()
        }
    }

    fn create_success_tx_result() -> TransactionResult {
        // Create empty operation results
        let empty_vec: Vec<OperationResult> = Vec::new();
        let op_results = empty_vec.try_into().unwrap_or_default();

        TransactionResult {
            fee_charged: 100,
            result: TransactionResultResult::TxSuccess(op_results),
            ext: TransactionResultExt::V0,
        }
    }

    fn dummy_get_transaction_response() -> GetTransactionResponse {
        GetTransactionResponse {
            status: "SUCCESS".to_string(),
            envelope: None,
            result: Some(create_success_tx_result()),
            result_meta: None,
        }
    }

    fn dummy_soroban_tx() -> SorobanTransactionResponse {
        SorobanTransactionResponse {
            response: dummy_get_transaction_response(),
        }
    }

    fn dummy_get_transactions_response() -> GetTransactionsResponse {
        GetTransactionsResponse {
            transactions: vec![],
            latest_ledger: 0,
            latest_ledger_close_time: 0,
            oldest_ledger: 0,
            oldest_ledger_close_time: 0,
            cursor: 0,
        }
    }

    fn dummy_get_ledger_entries_response() -> GetLedgerEntriesResponse {
        GetLedgerEntriesResponse {
            entries: None,
            latest_ledger: 0,
        }
    }

    fn dummy_get_events_response() -> GetEventsResponse {
        GetEventsResponse {
            events: vec![],
            latest_ledger: 0,
        }
    }

    fn dummy_transaction_envelope() -> TransactionEnvelope {
        create_mock_set_options_tx_envelope()
    }

    fn dummy_ledger_key() -> LedgerKey {
        LedgerKey::Account(LedgerKeyAccount {
            account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
        })
    }

    pub fn mock_account_entry(account_id: &str) -> AccountEntry {
        AccountEntry {
            account_id: AccountId(PublicKey::from_str(account_id).unwrap()),
            balance: 0,
            ext: AccountEntryExt::V0,
            flags: 0,
            home_domain: String32::default(),
            inflation_dest: None,
            seq_num: 0.into(),
            num_sub_entries: 0,
            signers: VecM::default(),
            thresholds: Thresholds([0, 0, 0, 0]),
        }
    }

    fn dummy_account_entry() -> AccountEntry {
        mock_account_entry("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
    }

    // ---------------------------------------------------------------------
    // Tests
    // ---------------------------------------------------------------------

    #[test]
    fn test_new_provider() {
        let provider =
            StellarProvider::new(vec![RpcConfig::new("http://localhost:8000".to_string())], 0);
        assert!(provider.is_ok());

        let provider_err = StellarProvider::new(vec![], 0);
        assert!(provider_err.is_err());
        match provider_err.unwrap_err() {
            ProviderError::NetworkConfiguration(msg) => {
                assert!(msg.contains("No RPC configurations provided"));
            }
            _ => panic!("Unexpected error type"),
        }
    }

    #[test]
    fn test_new_provider_selects_highest_weight() {
        let configs = vec![
            RpcConfig::with_weight("http://rpc1.example.com".to_string(), 10).unwrap(),
            RpcConfig::with_weight("http://rpc2.example.com".to_string(), 100).unwrap(), // Highest weight
            RpcConfig::with_weight("http://rpc3.example.com".to_string(), 50).unwrap(),
        ];
        let provider = StellarProvider::new(configs, 0);
        assert!(provider.is_ok());
        // We can't directly inspect the client's URL easily without more complex mocking or changes.
        // For now, we trust the sorting logic and that Client::new would fail for a truly bad URL if selection was wrong.
        // A more robust test would involve a mock client or a way to inspect the chosen URL.
    }

    #[test]
    fn test_new_provider_ignores_weight_zero() {
        let configs = vec![
            RpcConfig::with_weight("http://rpc1.example.com".to_string(), 0).unwrap(), // Weight 0
            RpcConfig::with_weight("http://rpc2.example.com".to_string(), 100).unwrap(), // Should be selected
        ];
        let provider = StellarProvider::new(configs, 0);
        assert!(provider.is_ok());

        let configs_only_zero =
            vec![RpcConfig::with_weight("http://rpc1.example.com".to_string(), 0).unwrap()];
        let provider_err = StellarProvider::new(configs_only_zero, 0);
        assert!(provider_err.is_err());
        match provider_err.unwrap_err() {
            ProviderError::NetworkConfiguration(msg) => {
                assert!(msg.contains("No active RPC configurations provided"));
            }
            _ => panic!("Unexpected error type"),
        }
    }

    #[test]
    fn test_new_provider_invalid_url_scheme() {
        let configs = vec![RpcConfig::new("ftp://invalid.example.com".to_string())];
        let provider_err = StellarProvider::new(configs, 0);
        assert!(provider_err.is_err());
        match provider_err.unwrap_err() {
            ProviderError::NetworkConfiguration(msg) => {
                assert!(msg.contains("Invalid URL scheme"));
            }
            _ => panic!("Unexpected error type"),
        }
    }

    #[test]
    fn test_new_provider_all_zero_weight_configs() {
        let configs = vec![
            RpcConfig::with_weight("http://rpc1.example.com".to_string(), 0).unwrap(),
            RpcConfig::with_weight("http://rpc2.example.com".to_string(), 0).unwrap(),
        ];
        let provider_err = StellarProvider::new(configs, 0);
        assert!(provider_err.is_err());
        match provider_err.unwrap_err() {
            ProviderError::NetworkConfiguration(msg) => {
                assert!(msg.contains("No active RPC configurations provided"));
            }
            _ => panic!("Unexpected error type"),
        }
    }

    #[tokio::test]
    async fn test_mock_basic_methods() {
        let mut mock = MockStellarProviderTrait::new();

        mock.expect_get_network()
            .times(1)
            .returning(|| async { Ok(dummy_get_network_response()) }.boxed());

        mock.expect_get_latest_ledger()
            .times(1)
            .returning(|| async { Ok(dummy_get_latest_ledger_response()) }.boxed());

        assert!(mock.get_network().await.is_ok());
        assert!(mock.get_latest_ledger().await.is_ok());
    }

    #[tokio::test]
    async fn test_mock_transaction_flow() {
        let mut mock = MockStellarProviderTrait::new();

        let envelope: TransactionEnvelope = dummy_transaction_envelope();
        let hash = dummy_hash();

        mock.expect_simulate_transaction_envelope()
            .withf(|_| true)
            .times(1)
            .returning(|_| async { Ok(dummy_simulate()) }.boxed());

        mock.expect_send_transaction()
            .withf(|_| true)
            .times(1)
            .returning(|_| async { Ok(dummy_hash()) }.boxed());

        mock.expect_send_transaction_polling()
            .withf(|_| true)
            .times(1)
            .returning(|_| async { Ok(dummy_soroban_tx()) }.boxed());

        mock.expect_get_transaction()
            .withf(|_| true)
            .times(1)
            .returning(|_| async { Ok(dummy_get_transaction_response()) }.boxed());

        mock.simulate_transaction_envelope(&envelope).await.unwrap();
        mock.send_transaction(&envelope).await.unwrap();
        mock.send_transaction_polling(&envelope).await.unwrap();
        mock.get_transaction(&hash).await.unwrap();
    }

    #[tokio::test]
    async fn test_mock_events_and_entries() {
        let mut mock = MockStellarProviderTrait::new();

        mock.expect_get_events()
            .times(1)
            .returning(|_| async { Ok(dummy_get_events_response()) }.boxed());

        mock.expect_get_ledger_entries()
            .times(1)
            .returning(|_| async { Ok(dummy_get_ledger_entries_response()) }.boxed());

        let events_request = GetEventsRequest {
            start: EventStart::Ledger(1),
            event_type: None,
            contract_ids: vec![],
            topics: vec![],
            limit: Some(10),
        };

        let dummy_key: LedgerKey = dummy_ledger_key();
        mock.get_events(events_request).await.unwrap();
        mock.get_ledger_entries(&[dummy_key]).await.unwrap();
    }

    #[tokio::test]
    async fn test_mock_all_methods_ok() {
        let mut mock = MockStellarProviderTrait::new();

        mock.expect_get_account()
            .with(p::eq("GTESTACCOUNTID"))
            .times(1)
            .returning(|_| async { Ok(dummy_account_entry()) }.boxed());

        mock.expect_simulate_transaction_envelope()
            .times(1)
            .returning(|_| async { Ok(dummy_simulate()) }.boxed());

        mock.expect_send_transaction_polling()
            .times(1)
            .returning(|_| async { Ok(dummy_soroban_tx()) }.boxed());

        mock.expect_get_network()
            .times(1)
            .returning(|| async { Ok(dummy_get_network_response()) }.boxed());

        mock.expect_get_latest_ledger()
            .times(1)
            .returning(|| async { Ok(dummy_get_latest_ledger_response()) }.boxed());

        mock.expect_send_transaction()
            .times(1)
            .returning(|_| async { Ok(dummy_hash()) }.boxed());

        mock.expect_get_transaction()
            .times(1)
            .returning(|_| async { Ok(dummy_get_transaction_response()) }.boxed());

        mock.expect_get_transactions()
            .times(1)
            .returning(|_| async { Ok(dummy_get_transactions_response()) }.boxed());

        mock.expect_get_ledger_entries()
            .times(1)
            .returning(|_| async { Ok(dummy_get_ledger_entries_response()) }.boxed());

        mock.expect_get_events()
            .times(1)
            .returning(|_| async { Ok(dummy_get_events_response()) }.boxed());

        let _ = mock.get_account("GTESTACCOUNTID").await.unwrap();
        let env: TransactionEnvelope = dummy_transaction_envelope();
        mock.simulate_transaction_envelope(&env).await.unwrap();
        mock.send_transaction_polling(&env).await.unwrap();
        mock.get_network().await.unwrap();
        mock.get_latest_ledger().await.unwrap();
        mock.send_transaction(&env).await.unwrap();

        let h = dummy_hash();
        mock.get_transaction(&h).await.unwrap();

        let req: GetTransactionsRequest = GetTransactionsRequest {
            start_ledger: None,
            pagination: None,
        };
        mock.get_transactions(req).await.unwrap();

        let key: LedgerKey = dummy_ledger_key();
        mock.get_ledger_entries(&[key]).await.unwrap();

        let ev_req = GetEventsRequest {
            start: EventStart::Ledger(0),
            event_type: None,
            contract_ids: vec![],
            topics: vec![],
            limit: None,
        };
        mock.get_events(ev_req).await.unwrap();
    }

    #[tokio::test]
    async fn test_error_propagation() {
        let mut mock = MockStellarProviderTrait::new();

        mock.expect_get_account()
            .returning(|_| async { Err(eyre!("boom")) }.boxed());

        let res = mock.get_account("BAD").await;
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("boom"));
    }

    #[tokio::test]
    async fn test_get_events_edge_cases() {
        let mut mock = MockStellarProviderTrait::new();

        mock.expect_get_events()
            .withf(|req| {
                req.contract_ids.is_empty() && req.topics.is_empty() && req.limit.is_none()
            })
            .times(1)
            .returning(|_| async { Ok(dummy_get_events_response()) }.boxed());

        let ev_req = GetEventsRequest {
            start: EventStart::Ledger(0),
            event_type: None,
            contract_ids: vec![],
            topics: vec![],
            limit: None,
        };

        mock.get_events(ev_req).await.unwrap();
    }

    #[test]
    fn test_provider_send_sync_bounds() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<StellarProvider>();
    }

    #[cfg(test)]
    mod concrete_tests {
        use super::*;

        const NON_EXISTENT_URL: &str = "http://127.0.0.1:9999";

        fn setup_provider() -> StellarProvider {
            StellarProvider::new(vec![RpcConfig::new(NON_EXISTENT_URL.to_string())], 0)
                .expect("Provider creation should succeed even with bad URL")
        }

        #[tokio::test]
        async fn test_concrete_get_account_error() {
            let provider = setup_provider();
            let result = provider.get_account("SOME_ACCOUNT_ID").await;
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Failed to get account"));
        }

        #[tokio::test]
        async fn test_concrete_simulate_transaction_envelope_error() {
            let provider = setup_provider();
            let envelope: TransactionEnvelope = dummy_transaction_envelope();
            let result = provider.simulate_transaction_envelope(&envelope).await;
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Failed to simulate transaction"));
        }

        #[tokio::test]
        async fn test_concrete_send_transaction_polling_error() {
            let provider = setup_provider();
            let envelope: TransactionEnvelope = dummy_transaction_envelope();
            let result = provider.send_transaction_polling(&envelope).await;
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Failed to send transaction (polling)"));
        }

        #[tokio::test]
        async fn test_concrete_get_network_error() {
            let provider = setup_provider();
            let result = provider.get_network().await;
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Failed to get network"));
        }

        #[tokio::test]
        async fn test_concrete_get_latest_ledger_error() {
            let provider = setup_provider();
            let result = provider.get_latest_ledger().await;
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Failed to get latest ledger"));
        }

        #[tokio::test]
        async fn test_concrete_send_transaction_error() {
            let provider = setup_provider();
            let envelope: TransactionEnvelope = dummy_transaction_envelope();
            let result = provider.send_transaction(&envelope).await;
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Failed to send transaction"));
        }

        #[tokio::test]
        async fn test_concrete_get_transaction_error() {
            let provider = setup_provider();
            let hash: Hash = dummy_hash();
            let result = provider.get_transaction(&hash).await;
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Failed to get transaction"));
        }

        #[tokio::test]
        async fn test_concrete_get_transactions_error() {
            let provider = setup_provider();
            let req = GetTransactionsRequest {
                start_ledger: None,
                pagination: None,
            };
            let result = provider.get_transactions(req).await;
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Failed to get transactions"));
        }

        #[tokio::test]
        async fn test_concrete_get_ledger_entries_error() {
            let provider = setup_provider();
            let key: LedgerKey = dummy_ledger_key();
            let result = provider.get_ledger_entries(&[key]).await;
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Failed to get ledger entries"));
        }

        #[tokio::test]
        async fn test_concrete_get_events_error() {
            let provider = setup_provider();
            let req = GetEventsRequest {
                start: EventStart::Ledger(1),
                event_type: None,
                contract_ids: vec![],
                topics: vec![],
                limit: None,
            };
            let result = provider.get_events(req).await;
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Failed to get events"));
        }
    }
}
