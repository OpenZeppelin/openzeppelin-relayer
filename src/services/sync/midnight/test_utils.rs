//! Test utilities for Midnight sync and transaction testing
//!
//! This module provides utilities for creating test contexts with funded wallets.
//! Since we cannot directly create coins in wallets without proper Midnight transaction
//! utilities, we support loading serialized wallet states from test fixtures.

use midnight_node_ledger_helpers::{
    DefaultDB, LedgerContext, LedgerState, NetworkId, WalletSeed, WalletState,
    mn_ledger_serialize::{tagged_deserialize, tagged_serialize},
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Test fixture directory for storing serialized wallet states
const TEST_FIXTURE_DIR: &str = "tests/fixtures/midnight";

/// Creates a test LedgerContext with pre-funded wallets
///
/// This function attempts to load wallet states from test fixtures if available.
/// If no fixtures exist, it creates an empty context with initialized wallets.
pub fn create_funded_test_context(
    wallet_seeds: &[WalletSeed],
    _initial_balance: u128,
) -> Arc<LedgerContext<DefaultDB>> {
    // Set required environment variable
    unsafe {
        std::env::set_var(
            "MIDNIGHT_LEDGER_TEST_STATIC_DIR",
            "/tmp/midnight-test-static",
        );
    }
    let context = Arc::new(LedgerContext::new_from_wallet_seeds(
        NetworkId::TestNet,
        wallet_seeds,
    ));

    // Try to load wallet states from fixtures
    for seed in wallet_seeds {
        if let Ok(wallet_state) = load_wallet_state_fixture(seed, NetworkId::TestNet) {
            if let Ok(mut wallets_guard) = context.wallets.lock() {
                if let Some(wallet) = wallets_guard.get_mut(seed) {
                    wallet.shielded.state = wallet_state;
                    log::debug!("Loaded wallet state from fixture for seed: {:?}", seed);
                }
            }
        }
    }

    context
}

/// Creates a mock transaction for testing
pub fn create_test_transaction_data() -> Vec<u8> {
    // This would be a properly serialized Midnight transaction
    // For now, return dummy data
    vec![1, 2, 3, 4, 5]
}

/// Saves a wallet state to a test fixture file
pub fn save_wallet_state_fixture(
    seed: &WalletSeed,
    wallet_state: &WalletState<DefaultDB>,
    network: NetworkId,
) -> Result<(), std::io::Error> {
    let fixture_path = get_wallet_fixture_path(seed);

    // Create directory if it doesn't exist
    if let Some(parent) = fixture_path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Serialize wallet state
    let mut state_bytes = Vec::new();
    tagged_serialize(wallet_state, &mut state_bytes).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Failed to serialize wallet state: {:?}", e),
        )
    })?;

    // Write to file
    fs::write(fixture_path, state_bytes)?;
    Ok(())
}

/// Loads a wallet state from a test fixture file
pub fn load_wallet_state_fixture(
    seed: &WalletSeed,
    network: NetworkId,
) -> Result<WalletState<DefaultDB>, std::io::Error> {
    let fixture_path = get_wallet_fixture_path(seed);

    // Read fixture file
    let state_bytes = fs::read(&fixture_path)?;

    // Deserialize wallet state
    let mut reader = &state_bytes[..];
    tagged_deserialize::<WalletState<DefaultDB>>(&mut reader).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Failed to deserialize wallet state: {:?}", e),
        )
    })
}

/// Gets the fixture file path for a wallet seed
fn get_wallet_fixture_path(seed: &WalletSeed) -> PathBuf {
    let seed_hex = hex::encode(seed.as_bytes());
    Path::new(TEST_FIXTURE_DIR).join(format!("wallet_{}.bin", seed_hex))
}

/// Creates a test context from a serialized ledger context
///
/// This can be used to restore a complete ledger context including wallet states
/// and ledger state from a previous sync operation.
pub fn create_context_from_serialized(
    serialized_context: &[u8],
    wallet_seeds: &[WalletSeed],
    network: NetworkId,
) -> Result<Arc<LedgerContext<DefaultDB>>, String> {
    // Set required environment variable
    unsafe {
        std::env::set_var(
            "MIDNIGHT_LEDGER_TEST_STATIC_DIR",
            "/tmp/midnight-test-static",
        );
    }

    let context = Arc::new(LedgerContext::new_from_wallet_seeds(network, wallet_seeds));

    // Deserialize the combined context (wallet states + ledger state)
    match bincode::deserialize::<(Option<Vec<u8>>, Vec<u8>)>(serialized_context) {
        Ok((wallet_state_bytes, ledger_state_bytes)) => {
            // Restore ledger state
            let mut reader = &ledger_state_bytes[..];
            if let Ok(ledger_state) = tagged_deserialize::<LedgerState<DefaultDB>>(&mut reader) {
                if let Ok(mut ledger_state_guard) = context.ledger_state.lock() {
                    *ledger_state_guard = ledger_state;
                }
            }

            // Restore wallet state if available
            if let Some(state_bytes) = wallet_state_bytes {
                let mut reader = &state_bytes[..];
                if let Ok(wallet_state) = tagged_deserialize::<WalletState<DefaultDB>>(&mut reader)
                {
                    if let Ok(mut wallets_guard) = context.wallets.lock() {
                        // Apply to the first wallet seed
                        if let Some(seed) = wallet_seeds.first() {
                            if let Some(wallet) = wallets_guard.get_mut(seed) {
                                wallet.shielded.state = wallet_state;
                            }
                        }
                    }
                }
            }
            Ok(context)
        }
        Err(e) => Err(format!("Failed to deserialize context: {}", e)),
    }
}

/// Example serialized context with a funded wallet for testing
///
/// This is a placeholder - in a real implementation, this would be generated
/// by running a sync operation on testnet and serializing the resulting context.
pub const EXAMPLE_FUNDED_CONTEXT: &[u8] = &[
    // This would contain actual serialized wallet state with coins
    // Generated by syncing a funded testnet wallet
    0x00, 0x01, 0x02, 0x03,
];

/// Builder for creating mock wallet states for testing
///
/// This provides a way to create wallet states with specific properties
/// without needing real blockchain data.
pub struct MockWalletStateBuilder {
    first_free: u64,
    // Additional fields would be added here as needed
}

impl MockWalletStateBuilder {
    pub fn new() -> Self {
        Self { first_free: 0 }
    }

    pub fn with_first_free(mut self, first_free: u64) -> Self {
        self.first_free = first_free;
        self
    }

    /// Builds a context with the mock wallet state
    ///
    /// Note: This creates a wallet with the specified first_free value
    /// but without actual coins. For tests requiring real UTXOs,
    /// use fixture-based approaches instead.
    pub fn build(self, wallet_seeds: &[WalletSeed]) -> Arc<LedgerContext<DefaultDB>> {
        unsafe {
            std::env::set_var(
                "MIDNIGHT_LEDGER_TEST_STATIC_DIR",
                "/tmp/midnight-test-static",
            );
        }
        let context = Arc::new(LedgerContext::new_from_wallet_seeds(
            NetworkId::TestNet,
            wallet_seeds,
        ));

        // Apply mock state to all wallets
        if let Ok(mut wallets_guard) = context.wallets.lock() {
            for seed in wallet_seeds {
                if let Some(wallet) = wallets_guard.get_mut(seed) {
                    wallet.shielded.state.first_free = self.first_free;
                }
            }
        }

        context
    }
}

impl Default for MockWalletStateBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_funded_test_context() {
        let seed = WalletSeed::Medium([1u8; 32]);
        let context = create_funded_test_context(&[seed], 1_000_000_000);

        // Verify wallet is initialized
        let wallets_guard = context.wallets.lock().unwrap();
        let wallet = wallets_guard.get(&seed).unwrap();
        // With empty fixtures, first_free will be 0, but the wallet should exist
        assert_eq!(wallet.shielded.state.first_free, 0);
    }

    #[test]
    fn test_create_funded_test_context_multiple_wallets() {
        let seed1 = WalletSeed::Medium([1u8; 32]);
        let seed2 = WalletSeed::Medium([2u8; 32]);
        let context = create_funded_test_context(&[seed1, seed2], 500_000_000);

        let wallets_guard = context.wallets.lock().unwrap();

        let wallet1 = wallets_guard.get(&seed1).unwrap();
        // With fixtures, first_free depends on the actual wallet state
        assert_eq!(wallet1.shielded.state.first_free, 0);

        let wallet2 = wallets_guard.get(&seed2).unwrap();
        // Without fixture for seed2, it will have default value of 0
        assert_eq!(wallet2.shielded.state.first_free, 0);
    }

    #[test]
    fn test_wallet_fixture_path() {
        let seed = WalletSeed::Medium([1u8; 32]);
        let path = get_wallet_fixture_path(&seed);

        let expected = format!(
            "{}/wallet_{}.bin",
            TEST_FIXTURE_DIR, "0101010101010101010101010101010101010101010101010101010101010101"
        );
        assert_eq!(path.to_str().unwrap(), expected);
    }

    #[test]
    fn test_create_context_from_serialized_empty() {
        let seed = WalletSeed::Medium([1u8; 32]);
        let empty_context = bincode::serialize(&(None::<Vec<u8>>, vec![0u8; 0]))
            .expect("Failed to serialize empty context");

        let result = create_context_from_serialized(&empty_context, &[seed], NetworkId::TestNet);
        assert!(result.is_ok());
    }

    #[test]
    fn test_mock_wallet_state_builder() {
        let seed = WalletSeed::Medium([1u8; 32]);

        let context = MockWalletStateBuilder::new()
            .with_first_free(42)
            .build(&[seed]);

        let wallets_guard = context.wallets.lock().unwrap();
        let wallet = wallets_guard.get(&seed).unwrap();
        assert_eq!(wallet.shielded.state.first_free, 42);
    }

    #[test]
    fn test_mock_wallet_state_builder_multiple_wallets() {
        let seed1 = WalletSeed::Medium([1u8; 32]);
        let seed2 = WalletSeed::Medium([2u8; 32]);

        let context = MockWalletStateBuilder::new()
            .with_first_free(100)
            .build(&[seed1, seed2]);

        let wallets_guard = context.wallets.lock().unwrap();

        let wallet1 = wallets_guard.get(&seed1).unwrap();
        assert_eq!(wallet1.shielded.state.first_free, 100);

        let wallet2 = wallets_guard.get(&seed2).unwrap();
        assert_eq!(wallet2.shielded.state.first_free, 100);
    }
}
