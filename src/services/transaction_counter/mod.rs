use std::sync::Arc;

use crate::repositories::{
    InMemoryTransactionCounter, TransactionCounterError, TransactionCounterTrait,
};

#[derive(Clone)]
pub struct TransactionCounterService {
    relayer_id: String,
    address: String,
    store: Arc<InMemoryTransactionCounter>,
}

#[allow(dead_code)]
impl TransactionCounterService {
    pub fn new(
        relayer_id: String,
        address: String,
        store: Arc<InMemoryTransactionCounter>,
    ) -> Self {
        Self {
            relayer_id,
            address,
            store,
        }
    }

    pub fn get(&self) -> Result<Option<u64>, TransactionCounterError> {
        self.store.get(&self.relayer_id, &self.address)
    }

    pub fn get_and_increment(&self) -> Result<u64, TransactionCounterError> {
        self.store
            .get_and_increment(&self.relayer_id, &self.address)
    }

    pub fn decrement(&self) -> Result<u64, TransactionCounterError> {
        self.store.decrement(&self.relayer_id, &self.address)
    }

    pub fn set(&self, value: u64) -> Result<(), TransactionCounterError> {
        self.store.set(&self.relayer_id, &self.address, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repositories::InMemoryTransactionCounter;

    #[tokio::test]
    async fn test_transaction_counter() {
        let store = Arc::new(InMemoryTransactionCounter::default());
        let service =
            TransactionCounterService::new("relayer_id".to_string(), "address".to_string(), store);

        assert_eq!(service.get().unwrap(), None);
        assert_eq!(service.get_and_increment().unwrap(), 0);
        assert_eq!(service.get_and_increment().unwrap(), 1);
        assert_eq!(service.decrement().unwrap(), 1);
        assert_eq!(service.set(10).unwrap(), ());
        assert_eq!(service.get().unwrap(), Some(10));
    }
}
