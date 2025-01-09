use crate::models::{NetworkTransactionRequest, RelayerError};
// transaction_repository.rs
use crate::repositories::*;
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::TryFrom;
use std::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum TransactionStatus {
    Pending,
    Confirmed,
    Sent,
    Submitted,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransactionRepoModel {
    pub id: String,
    pub relayer_id: String,
    pub hash: String,
    pub status: TransactionStatus,
    pub created_at: u64,
    pub sent_at: u64,
    pub confirmed_at: u64,
    pub network_data: NetworkTransactionData,
    pub network_type: NetworkType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "network_data", content = "data")]
pub enum NetworkTransactionData {
    Evm(EvmTransactionData),
    Solana(SolanaTransactionData),
    Stellar(StellarTransactionData),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvmTransactionData {
    pub gas_price: u64,
    pub gas_limit: u64,
    pub nonce: u64,
    pub value: u64,
    pub data: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolanaTransactionData {
    pub recent_blockhash: String,
    pub fee_payer: String,
    pub instructions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StellarTransactionData {
    pub source_account: String,
    pub fee: u32,
    pub sequence_number: u64,
    pub operations: Vec<String>,
}

impl TryFrom<(&NetworkTransactionRequest, &RelayerRepoModel)> for TransactionRepoModel {
    type Error = RelayerError;

    fn try_from(
        (request, relayer_model): (&NetworkTransactionRequest, &RelayerRepoModel),
    ) -> Result<Self, Self::Error> {
        let now = Utc::now().timestamp() as u64;

        match request {
            NetworkTransactionRequest::Evm(evm_request) => Ok(Self {
                id: Uuid::new_v4().to_string(),
                relayer_id: relayer_model.id.clone(),
                hash: "0x".to_string(),
                status: TransactionStatus::Pending,
                created_at: now,
                sent_at: now,
                confirmed_at: 0,
                network_type: NetworkType::Evm,
                network_data: NetworkTransactionData::Evm(EvmTransactionData {
                    gas_price: evm_request.gas_price,
                    gas_limit: evm_request.gas_limit,
                    nonce: 0, // TODO
                    value: evm_request.value,
                    data: evm_request.data.clone(),
                    from: "0x".to_string(), // TODO
                    to: evm_request.to.clone(),
                }),
            }),
            NetworkTransactionRequest::Solana(solana_request) => Ok(Self {
                id: Uuid::new_v4().to_string(),
                relayer_id: relayer_model.id.clone(),
                hash: "0x".to_string(),
                status: TransactionStatus::Pending,
                created_at: now,
                sent_at: now,
                confirmed_at: 0,
                network_type: NetworkType::Solana,
                network_data: NetworkTransactionData::Solana(SolanaTransactionData {
                    recent_blockhash: solana_request.recent_blockhash.clone(),
                    fee_payer: "0x".to_string(), // TODO
                    instructions: vec![],        // TODO
                }),
            }),
            NetworkTransactionRequest::Stellar(stellar_request) => Ok(Self {
                id: Uuid::new_v4().to_string(),
                relayer_id: relayer_model.id.clone(),
                hash: "0x".to_string(),
                status: TransactionStatus::Pending,
                created_at: now,
                sent_at: now,
                confirmed_at: 0,
                network_type: NetworkType::Stellar,
                network_data: NetworkTransactionData::Stellar(StellarTransactionData {
                    source_account: stellar_request.source_account.clone(),
                    fee: stellar_request.fee,
                    sequence_number: 0, // TODO
                    operations: vec![], // TODO
                }),
            }),
        }
    }
}

pub struct InMemoryTransactionRepository {
    store: Mutex<HashMap<String, TransactionRepoModel>>,
}

impl InMemoryTransactionRepository {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
        }
    }

    pub async fn find_by_relayer_id(
        &self,
        relayer_id: &str,
    ) -> Result<Vec<TransactionRepoModel>, RepositoryError> {
        let store = self.store.lock().unwrap();
        Ok(store
            .values()
            .filter(|tx| tx.relayer_id == relayer_id)
            .cloned()
            .collect())
    }

    pub async fn find_by_status(
        &self,
        status: TransactionStatus,
    ) -> Result<Vec<TransactionRepoModel>, RepositoryError> {
        let store = self.store.lock().unwrap();
        Ok(store
            .values()
            .filter(|tx| tx.status == status)
            .cloned()
            .collect())
    }

    pub async fn find_by_nonce(
        &self,
        relayer_id: &str,
        nonce: u64,
    ) -> Result<Option<TransactionRepoModel>, RepositoryError> {
        let store = self.store.lock().unwrap();
        Ok(store
            .values()
            .find(|tx| {
                tx.relayer_id == relayer_id
                    && matches!(&tx.network_data,
                        NetworkTransactionData::Evm(data) if data.nonce == nonce
                    )
            })
            .cloned())
    }
}

impl Default for InMemoryTransactionRepository {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Repository<TransactionRepoModel, String> for InMemoryTransactionRepository {
    async fn create(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        let mut store = self.store.lock().unwrap();
        if store.contains_key(&tx.id) {
            return Err(RepositoryError::ConstraintViolation(format!(
                "Transaction with ID {} already exists",
                tx.id
            )));
        }
        store.insert(tx.id.clone(), tx.clone());
        Ok(tx)
    }

    async fn get_by_id(&self, id: String) -> Result<TransactionRepoModel, RepositoryError> {
        let store = self.store.lock().unwrap();
        store.get(&id).cloned().ok_or_else(|| {
            RepositoryError::NotFound(format!("Transaction with ID {} not found", id))
        })
    }

    async fn update(
        &self,
        id: String,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        let mut store = self.store.lock().unwrap();
        if store.contains_key(&id) {
            let mut updated_tx = tx;
            updated_tx.id = id;
            // store.insert(id, updated_tx.clone());
            Ok(updated_tx)
        } else {
            Err(RepositoryError::NotFound(format!(
                "Transaction with ID {} not found",
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
                "Transaction with ID {} not found",
                id
            )))
        }
    }

    async fn list_all(&self) -> Result<Vec<TransactionRepoModel>, RepositoryError> {
        let store = self.store.lock().unwrap();
        Ok(store.values().cloned().collect())
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        let store = self.store.lock().unwrap();
        Ok(store.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_transaction(id: &str) -> TransactionRepoModel {
        TransactionRepoModel {
            id: id.to_string(),
            relayer_id: "relayer-1".to_string(),
            hash: format!("0x{}", id),
            status: TransactionStatus::Pending,
            created_at: 1234567890,
            sent_at: 1234567890,
            confirmed_at: 1234567890,
            network_type: NetworkType::Evm,
            network_data: NetworkTransactionData::Evm(EvmTransactionData {
                gas_price: 1000000000,
                gas_limit: 21000,
                nonce: 1,
                value: 1000000000000000000,
                data: "Ox".to_string(),
                from: "0x".to_string(),
                to: "0x".to_string(),
            }),
        }
    }

    #[actix_web::test]
    async fn test_create_transaction() {
        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction("test-1");

        let result = repo.create(tx.clone()).await.unwrap();
        assert_eq!(result.id, tx.id);
        assert_eq!(repo.count().await.unwrap(), 1);
    }

    #[actix_web::test]
    async fn test_get_transaction() {
        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction("test-1");

        repo.create(tx.clone()).await.unwrap();
        let stored = repo.get_by_id("test-1".to_string()).await.unwrap();
        assert_eq!(stored.hash, tx.hash);
    }

    #[actix_web::test]
    async fn test_update_transaction() {
        let repo = InMemoryTransactionRepository::new();
        let mut tx = create_test_transaction("test-1");

        repo.create(tx.clone()).await.unwrap();
        tx.status = TransactionStatus::Confirmed;

        let updated = repo.update("test-1".to_string(), tx).await.unwrap();
        assert!(matches!(updated.status, TransactionStatus::Confirmed));
    }
}
