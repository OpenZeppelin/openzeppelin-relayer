use crate::{
    config::{ConfigFileNetworkType, ConfigFileRelayerNetworkPolicy, RelayerFileConfig},
    models::{
        NetworkType, RelayerEvmPolicy, RelayerNetworkPolicy, RelayerRepoModel, RelayerSolanaPolicy,
        RelayerStellarPolicy, RepositoryError,
    },
    repositories::*,
};
use async_trait::async_trait;
use eyre::Result;
use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard},
};
use thiserror::Error;

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

    fn acquire_lock<T>(lock: &Mutex<T>) -> Result<MutexGuard<T>, RepositoryError> {
        lock.lock()
            .map_err(|_| RepositoryError::LockError("Failed to acquire lock".to_string()))
    }

    async fn list_active(&self) -> Result<Vec<RelayerRepoModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store)?;
        let active_relayers: Vec<RelayerRepoModel> = store
            .values()
            .filter(|&relayer| !relayer.paused)
            .cloned()
            .collect();
        Ok(active_relayers)
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
        let mut store = Self::acquire_lock(&self.store)?;
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
        let store = Self::acquire_lock(&self.store)?;
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
        let mut store = Self::acquire_lock(&self.store)?;
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
        let mut store = Self::acquire_lock(&self.store)?;
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
        let store = Self::acquire_lock(&self.store)?;
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
            .map_err(|_| RepositoryError::LockError("Failed to acquire lock".to_string()))?
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
        let store = Self::acquire_lock(&self.store)?;
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
}
