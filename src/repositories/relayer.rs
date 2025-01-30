use crate::{
    config::{ConfigFileNetworkType, ConfigFileRelayerNetworkPolicy, RelayerFileConfig},
    domain::RelayerUpdateRequest,
    models::{
        NetworkType, RelayerEvmPolicy, RelayerNetworkPolicy, RelayerRepoModel, RelayerSolanaPolicy,
        RelayerStellarPolicy, RepositoryError,
    },
    repositories::*,
};
use async_trait::async_trait;
use eyre::Result;
use std::collections::HashMap;
use thiserror::Error;
use tokio::sync::{Mutex, MutexGuard};

#[derive(Debug)]
pub struct InMemoryRelayerRepository {
    store: Mutex<HashMap<String, RelayerRepoModel>>,
}

#[allow(dead_code)]
impl InMemoryRelayerRepository {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
        }
    }

    async fn acquire_lock<T>(lock: &Mutex<T>) -> Result<MutexGuard<T>, RepositoryError> {
        Ok(lock.lock().await)
    }

    pub async fn list_active(&self) -> Result<Vec<RelayerRepoModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        let active_relayers: Vec<RelayerRepoModel> = store
            .values()
            .filter(|&relayer| !relayer.paused)
            .cloned()
            .collect();
        Ok(active_relayers)
    }

    pub async fn partial_update(
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

    pub async fn disable_relayer(
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

    pub async fn enable_relayer(
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

impl Default for InMemoryRelayerRepository {
    fn default() -> Self {
        Self::new()
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
        let relayers: Vec<RelayerRepoModel> = store.values().cloned().collect();
        Ok(relayers)
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<RelayerRepoModel>, RepositoryError> {
        let total = self.count().await?;
        let start = ((query.page - 1) * query.per_page) as usize;
        let items: Vec<RelayerRepoModel> = self
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
        let store = Self::acquire_lock(&self.store).await?;
        let relayers_length = store.len();
        Ok(relayers_length)
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
            Some(
                RelayerNetworkPolicy::try_from(config_policies).map_err(|_| {
                    ConversionError::InvalidNetworkType(
                        "Failed to convert network policy".to_string(),
                    )
                })?,
            )
        } else {
            None
        };

        Ok(RelayerRepoModel {
            id: config.id,
            name: config.name,
            network: config.network,
            paused: config.paused,
            network_type,
            policies,
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
                    eip1559_pricing: evm.eip1559_pricing,
                    private_transactions: evm.private_transactions,
                }))
            }
            ConfigFileRelayerNetworkPolicy::Solana(solana) => {
                Ok(RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
                    max_retries: solana.max_retries,
                    confirmation_blocks: solana.confirmation_blocks,
                    timeout_seconds: solana.timeout_seconds,
                }))
            }
            ConfigFileRelayerNetworkPolicy::Stellar(stellar) => {
                Ok(RelayerNetworkPolicy::Stellar(RelayerStellarPolicy {
                    max_fee: stellar.max_fee,
                    timeout_seconds: stellar.timeout_seconds,
                    min_account_balance: stellar.min_account_balance,
                }))
            }
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
            policies: None,
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
        assert_eq!(updated_relayer.paused, true);
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
        assert_eq!(disabled_relayer.system_disabled, true);
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
        assert_eq!(enabled_relayer.system_disabled, false);
    }
}
