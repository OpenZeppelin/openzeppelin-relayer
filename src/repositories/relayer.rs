use crate::{
    config::{ConfigFileNetworkType, ConfigFileRelayerNetworkPolicy, RelayerFileConfig},
    constants::{
        DEFAULT_EVM_MIN_BALANCE, DEFAULT_SOLANA_MIN_BALANCE, DEFAULT_STELLAR_MIN_BALANCE,
        MAX_SOLANA_TX_DATA_SIZE,
    },
    domain::RelayerUpdateRequest,
    models::{
        NetworkType, RelayerEvmPolicy, RelayerNetworkPolicy, RelayerRepoModel, RelayerSolanaPolicy,
        RelayerStellarPolicy, RepositoryError, SolanaAllowedTokensPolicy,
    },
};
use eyre::Result;
use std::collections::HashMap;
use thiserror::Error;
use tokio::sync::{Mutex, MutexGuard};

use crate::models::PaginationQuery;
use async_trait::async_trait;

use super::{PaginatedResult, Repository};

#[async_trait]
pub trait RelayerRepository: Repository<RelayerRepoModel, String> + Send + Sync {
    async fn list_active(&self) -> Result<Vec<RelayerRepoModel>, RepositoryError>;
    async fn partial_update(
        &self,
        id: String,
        update: RelayerUpdateRequest,
    ) -> Result<RelayerRepoModel, RepositoryError>;
    async fn enable_relayer(&self, relayer_id: String)
        -> Result<RelayerRepoModel, RepositoryError>;
    async fn disable_relayer(
        &self,
        relayer_id: String,
    ) -> Result<RelayerRepoModel, RepositoryError>;
    async fn update_policy(
        &self,
        id: String,
        policy: RelayerNetworkPolicy,
    ) -> Result<RelayerRepoModel, RepositoryError>;
}

#[derive(Debug)]
pub struct InMemoryRelayerRepository {
    store: Mutex<HashMap<String, RelayerRepoModel>>,
}

impl InMemoryRelayerRepository {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
        }
    }
    async fn acquire_lock<T>(lock: &Mutex<T>) -> Result<MutexGuard<T>, RepositoryError> {
        Ok(lock.lock().await)
    }
}

impl Default for InMemoryRelayerRepository {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RelayerRepository for InMemoryRelayerRepository {
    async fn list_active(&self) -> Result<Vec<RelayerRepoModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        let active_relayers: Vec<RelayerRepoModel> = store
            .values()
            .filter(|&relayer| !relayer.paused)
            .cloned()
            .collect();
        Ok(active_relayers)
    }

    async fn partial_update(
        &self,
        id: String,
        update: RelayerUpdateRequest,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        if let Some(relayer) = store.get_mut(&id) {
            if let Some(paused) = update.paused {
                relayer.paused = paused;
            }
            Ok(relayer.clone())
        } else {
            Err(RepositoryError::NotFound(format!(
                "Relayer with ID {} not found",
                id
            )))
        }
    }

    async fn update_policy(
        &self,
        id: String,
        policy: RelayerNetworkPolicy,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        let relayer = store.get_mut(&id).ok_or_else(|| {
            RepositoryError::NotFound(format!("Relayer with ID {} not found", id))
        })?;
        relayer.policies = policy;
        Ok(relayer.clone())
    }

    async fn disable_relayer(
        &self,
        relayer_id: String,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        let mut store = self.store.lock().await;
        if let Some(relayer) = store.get_mut(&relayer_id) {
            relayer.system_disabled = true;
            Ok(relayer.clone())
        } else {
            Err(RepositoryError::NotFound(format!(
                "Relayer with ID {} not found",
                relayer_id
            )))
        }
    }

    async fn enable_relayer(
        &self,
        relayer_id: String,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        let mut store = self.store.lock().await;
        if let Some(relayer) = store.get_mut(&relayer_id) {
            relayer.system_disabled = false;
            Ok(relayer.clone())
        } else {
            Err(RepositoryError::NotFound(format!(
                "Relayer with ID {} not found",
                relayer_id
            )))
        }
    }
}

#[async_trait]
impl Repository<RelayerRepoModel, String> for InMemoryRelayerRepository {
    async fn create(&self, relayer: RelayerRepoModel) -> Result<RelayerRepoModel, RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        if store.contains_key(&relayer.id) {
            return Err(RepositoryError::ConstraintViolation(format!(
                "Relayer with ID {} already exists",
                relayer.id
            )));
        }
        store.insert(relayer.id.clone(), relayer.clone());
        Ok(relayer)
    }

    async fn get_by_id(&self, id: String) -> Result<RelayerRepoModel, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        match store.get(&id) {
            Some(relayer) => Ok(relayer.clone()),
            None => Err(RepositoryError::NotFound(format!(
                "Relayer with ID {} not found",
                id
            ))),
        }
    }
    #[allow(clippy::map_entry)]
    async fn update(
        &self,
        id: String,
        relayer: RelayerRepoModel,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        if store.contains_key(&id) {
            // Ensure we update the existing entry
            let mut updated_relayer = relayer;
            updated_relayer.id = id.clone(); // Preserve original ID
            store.insert(id, updated_relayer.clone());
            Ok(updated_relayer)
        } else {
            Err(RepositoryError::NotFound(format!(
                "Relayer with ID {} not found",
                id
            )))
        }
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        if store.remove(&id).is_some() {
            Ok(())
        } else {
            Err(RepositoryError::NotFound(format!(
                "Relayer with ID {} not found",
                id
            )))
        }
    }

    async fn list_all(&self) -> Result<Vec<RelayerRepoModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        Ok(store.values().cloned().collect())
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<RelayerRepoModel>, RepositoryError> {
        let total = self.count().await?;
        let start = ((query.page - 1) * query.per_page) as usize;
        let items = self
            .store
            .lock()
            .await
            .values()
            .skip(start)
            .take(query.per_page as usize)
            .cloned()
            .collect();
        Ok(PaginatedResult {
            items,
            total: total as u64,
            page: query.page,
            per_page: query.per_page,
        })
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        Ok(self.store.lock().await.len())
    }
}

#[derive(Error, Debug)]
pub enum ConversionError {
    #[error("Invalid network type: {0}")]
    InvalidNetworkType(String),
}

impl TryFrom<RelayerFileConfig> for RelayerRepoModel {
    type Error = ConversionError;

    fn try_from(config: RelayerFileConfig) -> Result<Self, Self::Error> {
        let network_type = match config.network_type {
            ConfigFileNetworkType::Evm => NetworkType::Evm,
            ConfigFileNetworkType::Stellar => NetworkType::Stellar,
            ConfigFileNetworkType::Solana => NetworkType::Solana,
        };

        let policies = if let Some(config_policies) = config.policies {
            RelayerNetworkPolicy::try_from(config_policies).map_err(|_| {
                ConversionError::InvalidNetworkType("Failed to convert network policy".to_string())
            })?
        } else {
            // return default policy based on network type
            match network_type {
                NetworkType::Evm => RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
                NetworkType::Stellar => {
                    RelayerNetworkPolicy::Stellar(RelayerStellarPolicy::default())
                }
                NetworkType::Solana => RelayerNetworkPolicy::Solana(RelayerSolanaPolicy::default()),
            }
        };

        Ok(RelayerRepoModel {
            id: config.id,
            name: config.name,
            network: config.network,
            paused: config.paused,
            network_type,
            signer_id: config.signer_id,
            policies,
            address: "".to_string(), /* Default to empty address. This is later updated by the
                                      * relayer */
            notification_id: config.notification_id,
            system_disabled: false,
        })
    }
}

impl TryFrom<ConfigFileRelayerNetworkPolicy> for RelayerNetworkPolicy {
    type Error = eyre::Error;

    fn try_from(policy: ConfigFileRelayerNetworkPolicy) -> Result<Self, Self::Error> {
        match policy {
            ConfigFileRelayerNetworkPolicy::Evm(evm) => {
                Ok(RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
                    gas_price_cap: evm.gas_price_cap,
                    whitelist_receivers: evm.whitelist_receivers,
                    eip1559_pricing: evm.eip1559_pricing.unwrap_or(false),
                    private_transactions: evm.private_transactions.unwrap_or(false),
                    min_balance: evm.min_balance.unwrap_or(DEFAULT_EVM_MIN_BALANCE),
                }))
            }
            ConfigFileRelayerNetworkPolicy::Solana(solana) => {
                // Create a new variable for solana.allowed_tokens.
                // If solana.allowed_tokens is None, the resulting variable will be None;
                // otherwise, each entry will be mapped using
                // SolanaAllowedTokensPolicy::new_partial.
                let mapped_allowed_tokens = solana
                    .allowed_tokens
                    .filter(|tokens| !tokens.is_empty())
                    .map(|tokens| {
                        tokens
                            .into_iter()
                            .map(|token| {
                                SolanaAllowedTokensPolicy::new_partial(
                                    token.mint,
                                    token.max_allowed_fee,
                                    token.conversion_slippage_percentage,
                                )
                            })
                            .collect::<Vec<_>>()
                    });
                Ok(RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
                    min_balance: solana.min_balance.unwrap_or(DEFAULT_SOLANA_MIN_BALANCE),
                    allowed_accounts: solana.allowed_accounts,
                    allowed_programs: solana.allowed_programs,
                    allowed_tokens: mapped_allowed_tokens,
                    disallowed_accounts: solana.disallowed_accounts,
                    max_signatures: solana.max_signatures,
                    max_tx_data_size: solana.max_tx_data_size.unwrap_or(MAX_SOLANA_TX_DATA_SIZE),
                    max_allowed_transfer_amount_lamports: solana
                        .max_allowed_transfer_amount_lamports,
                }))
            }
            ConfigFileRelayerNetworkPolicy::Stellar(stellar) => {
                Ok(RelayerNetworkPolicy::Stellar(RelayerStellarPolicy {
                    max_fee: stellar.max_fee,
                    timeout_seconds: stellar.timeout_seconds,
                    min_balance: stellar.min_balance.unwrap_or(DEFAULT_STELLAR_MIN_BALANCE),
                }))
            }
        }
    }
}

#[derive(Debug)]
pub enum RelayerRepositoryStorage {
    InMemory(InMemoryRelayerRepository),
}

#[async_trait]
impl RelayerRepository for RelayerRepositoryStorage {
    #[allow(dead_code)]
    async fn list_active(&self) -> Result<Vec<RelayerRepoModel>, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.list_active().await,
        }
    }

    async fn partial_update(
        &self,
        id: String,
        update: RelayerUpdateRequest,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.partial_update(id, update).await,
        }
    }

    async fn update_policy(
        &self,
        id: String,
        policy: RelayerNetworkPolicy,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.update_policy(id, policy).await,
        }
    }

    async fn disable_relayer(
        &self,
        relayer_id: String,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.disable_relayer(relayer_id).await,
        }
    }

    async fn enable_relayer(
        &self,
        relayer_id: String,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.enable_relayer(relayer_id).await,
        }
    }
}

#[async_trait]
impl Repository<RelayerRepoModel, String> for RelayerRepositoryStorage {
    async fn create(&self, entity: RelayerRepoModel) -> Result<RelayerRepoModel, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.create(entity).await,
        }
    }

    async fn get_by_id(&self, id: String) -> Result<RelayerRepoModel, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.get_by_id(id).await,
        }
    }

    async fn update(
        &self,
        id: String,
        entity: RelayerRepoModel,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.update(id, entity).await,
        }
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.delete_by_id(id).await,
        }
    }

    async fn list_all(&self) -> Result<Vec<RelayerRepoModel>, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.list_all().await,
        }
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<RelayerRepoModel>, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.list_paginated(query).await,
        }
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.count().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_relayer(id: String) -> RelayerRepoModel {
        RelayerRepoModel {
            id: id.clone(),
            name: format!("Relayer {}", id.clone()),
            network: "TestNet".to_string(),
            paused: false,
            network_type: NetworkType::Evm,
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
                gas_price_cap: None,
                whitelist_receivers: None,
                eip1559_pricing: false,
                private_transactions: false,
                min_balance: 0,
            }),
            signer_id: "test".to_string(),
            address: "0x".to_string(),
            notification_id: None,
            system_disabled: false,
        }
    }

    #[actix_web::test]
    async fn test_new_repository_is_empty() {
        let repo = InMemoryRelayerRepository::new();
        assert_eq!(repo.count().await.unwrap(), 0);
    }

    #[actix_web::test]
    async fn test_add_relayer() {
        let repo = InMemoryRelayerRepository::new();
        let relayer = create_test_relayer("test".to_string());

        repo.create(relayer.clone()).await.unwrap();
        assert_eq!(repo.count().await.unwrap(), 1);

        let stored = repo.get_by_id("test".to_string()).await.unwrap();
        assert_eq!(stored.id, relayer.id);
        assert_eq!(stored.name, relayer.name);
    }

    #[actix_web::test]
    async fn test_update_relayer() {
        let repo = InMemoryRelayerRepository::new();
        let mut relayer = create_test_relayer("test".to_string());

        repo.create(relayer.clone()).await.unwrap();

        relayer.name = "Updated Name".to_string();
        repo.update("test".to_string(), relayer.clone())
            .await
            .unwrap();

        let updated = repo.get_by_id("test".to_string()).await.unwrap();
        assert_eq!(updated.name, "Updated Name");
    }

    #[actix_web::test]
    async fn test_list_relayers() {
        let repo = InMemoryRelayerRepository::new();
        let relayer1 = create_test_relayer("test".to_string());
        let relayer2 = create_test_relayer("test2".to_string());

        repo.create(relayer1.clone()).await.unwrap();
        repo.create(relayer2).await.unwrap();

        let relayers = repo.list_all().await.unwrap();
        assert_eq!(relayers.len(), 2);
    }

    #[actix_web::test]
    async fn test_list_active_relayers() {
        let repo = InMemoryRelayerRepository::new();
        let relayer1 = create_test_relayer("test".to_string());
        let mut relayer2 = create_test_relayer("test2".to_string());

        relayer2.paused = true;

        repo.create(relayer1.clone()).await.unwrap();
        repo.create(relayer2).await.unwrap();

        let active_relayers = repo.list_active().await.unwrap();
        assert_eq!(active_relayers.len(), 1);
        assert_eq!(active_relayers[0].id, "test".to_string());
    }

    #[actix_web::test]
    async fn test_update_nonexistent_relayer() {
        let repo = InMemoryRelayerRepository::new();
        let relayer = create_test_relayer("test".to_string());

        let result = repo.update("test".to_string(), relayer).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[actix_web::test]
    async fn test_get_nonexistent_relayer() {
        let repo = InMemoryRelayerRepository::new();

        let result = repo.get_by_id("test".to_string()).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[actix_web::test]
    async fn test_partial_update_relayer() {
        let repo = InMemoryRelayerRepository::new();

        // Add a relayer to the repository
        let relayer_id = "test_relayer".to_string();
        let initial_relayer = create_test_relayer(relayer_id.clone());

        repo.create(initial_relayer.clone()).await.unwrap();

        // Perform a partial update on the relayer
        let update_req = RelayerUpdateRequest { paused: Some(true) };

        let updated_relayer = repo
            .partial_update(relayer_id.clone(), update_req)
            .await
            .unwrap();

        assert_eq!(updated_relayer.id, initial_relayer.id);
        assert!(updated_relayer.paused);
    }

    #[actix_web::test]
    async fn test_disable_relayer() {
        let repo = InMemoryRelayerRepository::new();

        // Add a relayer to the repository
        let relayer_id = "test_relayer".to_string();
        let initial_relayer = create_test_relayer(relayer_id.clone());

        repo.create(initial_relayer.clone()).await.unwrap();

        // Disable the relayer
        let disabled_relayer = repo.disable_relayer(relayer_id.clone()).await.unwrap();

        assert_eq!(disabled_relayer.id, initial_relayer.id);
        assert!(disabled_relayer.system_disabled);
    }

    #[actix_web::test]
    async fn test_enable_relayer() {
        let repo = InMemoryRelayerRepository::new();

        // Add a relayer to the repository
        let relayer_id = "test_relayer".to_string();
        let mut initial_relayer = create_test_relayer(relayer_id.clone());

        initial_relayer.system_disabled = true;

        repo.create(initial_relayer.clone()).await.unwrap();

        // Enable the relayer
        let enabled_relayer = repo.enable_relayer(relayer_id.clone()).await.unwrap();

        assert_eq!(enabled_relayer.id, initial_relayer.id);
        assert!(!enabled_relayer.system_disabled);
    }

    #[actix_web::test]
    async fn test_update_policy() {
        let repo = InMemoryRelayerRepository::new();
        let relayer = create_test_relayer("test".to_string());

        repo.create(relayer.clone()).await.unwrap();
    }
}
