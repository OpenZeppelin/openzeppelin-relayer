use dashmap::DashMap;

use super::{TransactionCounterError, TransactionCounterTrait};

#[derive(Debug, Default, Clone)]
pub struct InMemoryTransactionCounter {
    store: DashMap<(String, String), u64>, // (relayer_id, address) -> nonce/sequence
}

impl InMemoryTransactionCounter {
    pub fn new() -> Self {
        Self {
            store: DashMap::new(),
        }
    }
}

impl TransactionCounterTrait for InMemoryTransactionCounter {
    fn get(&self, relayer_id: &str, address: &str) -> Result<Option<u64>, TransactionCounterError> {
        Ok(self
            .store
            .get(&(relayer_id.to_string(), address.to_string()))
            .map(|n| *n))
    }

    fn increment(&self, relayer_id: &str, address: &str) -> Result<u64, TransactionCounterError> {
        let mut entry = self
            .store
            .entry((relayer_id.to_string(), address.to_string()))
            .or_insert(0);
        *entry += 1;
        Ok(*entry)
    }

    fn decrement(&self, relayer_id: &str, address: &str) -> Result<u64, TransactionCounterError> {
        let mut entry = self
            .store
            .get_mut(&(relayer_id.to_string(), address.to_string()))
            .ok_or_else(|| {
                TransactionCounterError::NotFound(format!("Counter not found for {}", address))
            })?;
        if *entry > 0 {
            *entry -= 1;
        }
        Ok(*entry)
    }

    fn set(
        &self,
        relayer_id: &str,
        address: &str,
        value: u64,
    ) -> Result<(), TransactionCounterError> {
        self.store
            .insert((relayer_id.to_string(), address.to_string()), value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decrement_not_found() {
        let store = InMemoryTransactionCounter::new();
        let result = store.decrement("nonexistent", "0x1234");
        assert!(matches!(result, Err(TransactionCounterError::NotFound(_))));
    }

    #[test]
    fn test_nonce_store() {
        let store = InMemoryTransactionCounter::new();
        let relayer_id = "relayer_1";
        let address = "0x1234";

        // Initially should be None
        assert_eq!(store.get(relayer_id, address).unwrap(), None);

        // Set a value explicitly
        store.set(relayer_id, address, 100).unwrap();
        assert_eq!(store.get(relayer_id, address).unwrap(), Some(100));

        // Increment
        assert_eq!(store.increment(relayer_id, address).unwrap(), 101);
        assert_eq!(store.get(relayer_id, address).unwrap(), Some(101));

        // Decrement
        assert_eq!(store.decrement(relayer_id, address).unwrap(), 100);
        assert_eq!(store.get(relayer_id, address).unwrap(), Some(100));
    }

    #[test]
    fn test_multiple_relayers() {
        let store = InMemoryTransactionCounter::new();

        // Setup different relayer/address combinations
        store.set("relayer_1", "0x1234", 100).unwrap();
        store.set("relayer_1", "0x5678", 200).unwrap();
        store.set("relayer_2", "0x1234", 300).unwrap();

        // Verify independent counters
        assert_eq!(store.get("relayer_1", "0x1234").unwrap(), Some(100));
        assert_eq!(store.get("relayer_1", "0x5678").unwrap(), Some(200));
        assert_eq!(store.get("relayer_2", "0x1234").unwrap(), Some(300));

        // Verify independent increments
        assert_eq!(store.increment("relayer_1", "0x1234").unwrap(), 101);
        assert_eq!(store.increment("relayer_1", "0x5678").unwrap(), 201);
        assert_eq!(store.get("relayer_2", "0x1234").unwrap(), Some(300));
    }
}
