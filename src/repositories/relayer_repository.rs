use crate::config::{NetworkType as ConfigNetworkType, RelayerConfig};
use crate::repositories::*;
use async_trait::async_trait;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum NetworkType {
    Evm,
    Stellar,
    Solana,
}

#[derive(Debug, Clone, Serialize)]
pub struct RelayerRepoModel {
    pub id: String,
    pub name: String,
    pub network: String,
    pub paused: bool,
    pub network_type: NetworkType,
}

pub struct InMemoryRelayerRepository {
    store: Mutex<HashMap<String, RelayerRepoModel>>,
}

impl InMemoryRelayerRepository {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
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
        let mut store = self.store.lock().unwrap();
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
        let store = self.store.lock().unwrap();
        match store.get(&id) {
            Some(relayer) => Ok(relayer.clone()),
            None => Err(RepositoryError::NotFound(format!(
                "Relayer with ID {} not found",
                id
            ))),
        }
    }

    async fn update(
        &self,
        id: String,
        relayer: RelayerRepoModel,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        let mut store = self.store.lock().unwrap();
        if store.contains_key(&id) {
            // Ensure we update the existing entry
            let mut updated_relayer = relayer;
            updated_relayer.id = id; // Preserve original ID
                                     // store.insert(id.clone(), updated_relayer.clone());
            Ok(updated_relayer)
        } else {
            Err(RepositoryError::NotFound(format!(
                "Relayer with ID {} not found",
                id
            )))
        }
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        let mut store = self.store.lock().unwrap();
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
        let store = self.store.lock().unwrap();
        let active_relayers: Vec<RelayerRepoModel> = store
            .values()
            .cloned()
            .filter(|relayer| relayer.paused == false)
            .collect();
        Ok(active_relayers)
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        let store = self.store.lock().unwrap();
        let relayers_length = store.len();
        Ok(relayers_length)
    }
}

#[derive(Error, Debug)]
pub enum ConversionError {
    #[error("Invalid network type: {0}")]
    InvalidNetworkType(String),
}

impl TryFrom<RelayerConfig> for RelayerRepoModel {
    type Error = ConversionError;

    fn try_from(config: RelayerConfig) -> Result<Self, Self::Error> {
        let network_type = match config.network_type {
            ConfigNetworkType::Evm => NetworkType::Evm,
            ConfigNetworkType::Stellar => NetworkType::Stellar,
            ConfigNetworkType::Solana => NetworkType::Solana,
        };

        Ok(RelayerRepoModel {
            id: config.id,
            name: config.name,
            network: config.network,
            paused: config.paused,
            network_type,
        })
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
    async fn test_list_active_relayers() {
        let repo = InMemoryRelayerRepository::new();
        let mut relayer1 = create_test_relayer("test".to_string());
        let mut relayer2 = create_test_relayer("test2".to_string());

        relayer2.paused = true;

        repo.create(relayer1.clone()).await.unwrap();
        repo.create(relayer2).await.unwrap();

        let active_relayers = repo.list_all().await.unwrap();
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
