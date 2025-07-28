#[cfg(test)]
pub mod mockutils {
    use std::sync::Arc;

    use alloy::primitives::U256;
    use chrono::Utc;
    use secrets::SecretVec;

    use crate::{
        config::{EvmNetworkConfig, NetworkConfigCommon, RepositoryStorageType, ServerConfig},
        jobs::MockJobProducerTrait,
        models::{
            AppState, EvmTransactionData, EvmTransactionRequest, LocalSignerConfigStorage,
            NetworkRepoModel, NetworkTransactionData, NetworkType, NotificationRepoModel,
            PluginModel, RelayerEvmPolicy, RelayerNetworkPolicy, RelayerRepoModel, SecretString,
            SignerConfigStorage, SignerRepoModel, TransactionRepoModel, TransactionStatus,
        },
        repositories::{
            NetworkRepositoryStorage, NotificationRepositoryStorage, PluginRepositoryStorage,
            PluginRepositoryTrait, RelayerRepositoryStorage, Repository, SignerRepositoryStorage,
            TransactionCounterRepositoryStorage, TransactionRepositoryStorage,
        },
    };

    pub fn create_mock_relayer(id: String, paused: bool) -> RelayerRepoModel {
        RelayerRepoModel {
            id: id.clone(),
            name: format!("Relayer {}", id.clone()),
            network: "test".to_string(),
            paused,
            network_type: NetworkType::Evm,
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
                gas_price_cap: None,
                whitelist_receivers: None,
                eip1559_pricing: Some(false),
                private_transactions: Some(false),
                min_balance: Some(0),
                gas_limit_estimation: Some(false),
            }),
            signer_id: "test".to_string(),
            address: "0x".to_string(),
            notification_id: None,
            system_disabled: false,
            custom_rpc_urls: None,
        }
    }

    pub fn create_mock_notification(id: String) -> NotificationRepoModel {
        NotificationRepoModel {
            id,
            notification_type: crate::models::NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: None,
        }
    }

    pub fn create_mock_signer() -> SignerRepoModel {
        let seed = vec![1u8; 32];
        let raw_key = SecretVec::new(32, |v| v.copy_from_slice(&seed));
        SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfigStorage::Local(LocalSignerConfigStorage { raw_key }),
        }
    }

    pub fn create_mock_network() -> NetworkRepoModel {
        NetworkRepoModel {
            id: "test".to_string(),
            name: "test".to_string(),
            network_type: NetworkType::Evm,
            config: crate::models::NetworkConfigData::Evm(EvmNetworkConfig {
                common: NetworkConfigCommon {
                    network: "test".to_string(),
                    from: None,
                    rpc_urls: Some(vec!["http://localhost:8545".to_string()]),
                    explorer_urls: None,
                    average_blocktime_ms: Some(1000),
                    is_testnet: Some(true),
                    tags: None,
                },
                required_confirmations: Some(1),
                features: None,
                symbol: Some("testETH".to_string()),
                chain_id: Some(1),
            }),
        }
    }

    pub fn create_mock_transaction() -> TransactionRepoModel {
        TransactionRepoModel {
            id: "test".to_string(),
            relayer_id: "test".to_string(),
            status: TransactionStatus::Pending,
            status_reason: None,
            created_at: Utc::now().to_string(),
            sent_at: None,
            confirmed_at: None,
            valid_until: None,
            delete_at: None,
            network_data: NetworkTransactionData::Evm(EvmTransactionData::default()),
            priced_at: None,
            hashes: vec![],
            network_type: NetworkType::Evm,
            noop_count: None,
            is_canceled: None,
        }
    }

    pub async fn create_mock_app_state(
        relayers: Option<Vec<RelayerRepoModel>>,
        signers: Option<Vec<SignerRepoModel>>,
        networks: Option<Vec<NetworkRepoModel>>,
        plugins: Option<Vec<PluginModel>>,
        transactions: Option<Vec<TransactionRepoModel>>,
    ) -> AppState<
        MockJobProducerTrait,
        RelayerRepositoryStorage,
        TransactionRepositoryStorage,
        NetworkRepositoryStorage,
        NotificationRepositoryStorage,
        SignerRepositoryStorage,
        TransactionCounterRepositoryStorage,
        PluginRepositoryStorage,
    > {
        let relayer_repository = Arc::new(RelayerRepositoryStorage::new_in_memory());
        if let Some(relayers) = relayers {
            for relayer in relayers {
                relayer_repository.create(relayer).await.unwrap();
            }
        }

        let signer_repository = Arc::new(SignerRepositoryStorage::new_in_memory());
        if let Some(signers) = signers {
            for signer in signers {
                signer_repository.create(signer).await.unwrap();
            }
        }

        let network_repository = Arc::new(NetworkRepositoryStorage::new_in_memory());
        if let Some(networks) = networks {
            for network in networks {
                network_repository.create(network).await.unwrap();
            }
        }

        let plugin_repository = Arc::new(PluginRepositoryStorage::new_in_memory());
        if let Some(plugins) = plugins {
            for plugin in plugins {
                plugin_repository.add(plugin).await.unwrap();
            }
        }

        let transaction_repository = Arc::new(TransactionRepositoryStorage::new_in_memory());
        if let Some(transactions) = transactions {
            for transaction in transactions {
                transaction_repository.create(transaction).await.unwrap();
            }
        }

        let mut mock_job_producer = MockJobProducerTrait::new();

        mock_job_producer
            .expect_produce_transaction_request_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        mock_job_producer
            .expect_produce_submit_transaction_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        mock_job_producer
            .expect_produce_check_transaction_status_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        mock_job_producer
            .expect_produce_send_notification_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        mock_job_producer
            .expect_produce_solana_token_swap_request_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        AppState {
            relayer_repository,
            transaction_repository,
            signer_repository,
            notification_repository: Arc::new(NotificationRepositoryStorage::new_in_memory()),
            network_repository,
            transaction_counter_store: Arc::new(
                TransactionCounterRepositoryStorage::new_in_memory(),
            ),
            job_producer: Arc::new(mock_job_producer),
            plugin_repository,
        }
    }

    pub fn create_mock_evm_transaction_request() -> EvmTransactionRequest {
        EvmTransactionRequest {
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            value: U256::from(0),
            data: Some("0x".to_string()),
            gas_limit: Some(21000),
            gas_price: Some(0),
            speed: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            valid_until: None,
        }
    }

    pub fn create_test_server_config(storage_type: RepositoryStorageType) -> ServerConfig {
        ServerConfig {
            host: "localhost".to_string(),
            port: 8080,
            redis_url: "redis://localhost:6379".to_string(),
            config_file_path: "./config/test.json".to_string(),
            api_key: SecretString::new("test_api_key_1234567890_test_key_32"),
            rate_limit_requests_per_second: 100,
            rate_limit_burst_size: 300,
            metrics_port: 8081,
            enable_swagger: false,
            redis_connection_timeout_ms: 5000,
            redis_key_prefix: "test-oz-relayer".to_string(),
            rpc_timeout_ms: 10000,
            provider_max_retries: 3,
            provider_retry_base_delay_ms: 100,
            provider_retry_max_delay_ms: 2000,
            provider_max_failovers: 3,
            repository_storage_type: storage_type,
            reset_storage_on_start: false,
            storage_encryption_key: Some(SecretString::new(
                "test_encryption_key_1234567890_test_key_32",
            )),
            transaction_expiration_hours: 4,
        }
    }
}
