//! Midnight Context Fixture Generator
//!
//! This tool generates complete context fixtures for Midnight blockchain testing.
//! A context fixture includes both wallet state (UTXOs, coins) and ledger state.
//!
//! Usage:
//!     WALLET_SEED=<hex_seed> cargo run --bin generate_midnight_fixtures
//!
//! Environment Variables:
//!   - WALLET_SEED: 32-byte hex string (required for meaningful fixtures)
//!   - START_HEIGHT: Blockchain height to start sync from (default: 0)
//!   - SAVE_INTERVAL: Save progress every N blocks
//!   - RUST_LOG: Log level (default: info)
//!
//! The fixtures will be saved to tests/fixtures/midnight/

use midnight_node_ledger_helpers::{
    DefaultDB, LedgerContext, NetworkId, WalletSeed, mn_ledger_serialize::tagged_serialize,
};
use openzeppelin_relayer::{
    config::network::IndexerUrls,
    repositories::{RelayerStateRepositoryStorage, SyncStateTrait},
    services::sync::midnight::{
        handler::{QuickSyncStrategy, SyncManager},
        indexer::MidnightIndexerClient,
    },
};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

/// Configuration for fixture generation
#[derive(Debug)]
struct FixtureConfig {
    wallet_seed: WalletSeed,
    start_height: u64,
    save_interval: Option<u64>,
}

impl FixtureConfig {
    fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        let seed_hex = env::var("WALLET_SEED").map_err(|_| {
            "WALLET_SEED environment variable is required. Example: WALLET_SEED=0e0cc7db98c60a39a6b0888795ba3f1bb1d61298cce264d4beca1529650e9041"
        })?;

        let wallet_seed = WalletSeed::try_from_hex_str(seed_hex.as_str()).unwrap();

        let start_height = env::var("START_HEIGHT")
            .ok()
            .and_then(|h| h.parse().ok())
            .unwrap_or(0);

        let save_interval = env::var("SAVE_INTERVAL").ok().and_then(|h| h.parse().ok());

        Ok(Self {
            wallet_seed,
            start_height,
            save_interval,
        })
    }
}

/// Gets the fixture file path for a context
fn get_context_fixture_path(seed: &WalletSeed, height: u64) -> PathBuf {
    let seed_hex = hex::encode(seed.as_bytes());
    PathBuf::from("tests/fixtures/midnight").join(format!("context_{}_{}.bin", seed_hex, height))
}

/// Serialize the entire ledger context to bytes
fn serialize_context(
    context: &Arc<LedgerContext<DefaultDB>>,
    seed: &WalletSeed,
    network: NetworkId,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    // Serialize the wallet state for the current seed
    let wallet_state = {
        let wallets_guard = context
            .wallets
            .lock()
            .map_err(|e| format!("Failed to lock wallets: {}", e))?;

        wallets_guard
            .get(seed)
            .map(|wallet| {
                let mut state_bytes = Vec::new();
                tagged_serialize(&wallet.shielded.state, &mut state_bytes)
                    .map_err(|e| format!("Failed to serialize wallet state: {:?}", e))?;
                Ok::<Vec<u8>, Box<dyn std::error::Error>>(state_bytes)
            })
            .transpose()?
    };

    // Serialize the ledger state
    let ledger_state_bytes = {
        let ledger_state_guard = context
            .ledger_state
            .lock()
            .map_err(|e| format!("Failed to lock ledger state: {}", e))?;

        let mut bytes = Vec::new();
        tagged_serialize(&*ledger_state_guard, &mut bytes)
            .map_err(|e| format!("Failed to serialize ledger state: {:?}", e))?;
        bytes
    };

    // Combine wallet state and ledger state into a single serialized context
    bincode::serialize(&(wallet_state, ledger_state_bytes))
        .map_err(|e| format!("Failed to serialize context: {}", e).into())
}

/// Generate context fixture
async fn generate_context_fixture(
    sync_manager: &mut SyncManager<QuickSyncStrategy>,
    config: &FixtureConfig,
    final_height: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("🏗️  Generating complete context fixture...");

    let context = sync_manager.get_context();

    // Serialize the complete context
    let context_bytes = serialize_context(&context, &config.wallet_seed, NetworkId::TestNet)?;

    // Save the context
    let context_fixture_path = get_context_fixture_path(&config.wallet_seed, final_height);
    fs::write(&context_fixture_path, &context_bytes)?;

    println!("✅ Context saved to: {}", context_fixture_path.display());
    println!("   Height: {}", final_height);
    println!("   Size: {} bytes", context_bytes.len());

    // Show wallet info from the context
    let wallets_guard = context.wallets.lock().unwrap();
    if let Some(wallet) = wallets_guard.get(&config.wallet_seed) {
        println!("\n📊 Wallet state in context:");
        println!("   First free: {}", wallet.shielded.state.first_free);

        let mut coin_count = 0;
        for _ in wallet.shielded.state.coins.iter() {
            coin_count += 1;
        }
        println!("   Coins: {}", coin_count);

        if wallet.shielded.state.first_free == 0 && coin_count == 0 {
            println!("\n⚠️  WARNING: Wallet appears to be empty!");
            println!("   Make sure the wallet has received tDUST tokens on testnet.");
        }
    }

    Ok(())
}

/// Set up progress monitoring task
fn setup_progress_monitoring(
    sync_state_store: Arc<RelayerStateRepositoryStorage>,
    relayer_id: String,
    save_interval: Option<u64>,
) {
    if let Some(interval) = save_interval {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

                if let Ok(Some(height)) = sync_state_store.get_last_synced_index(&relayer_id).await
                {
                    if height > 0 && height % interval == 0 {
                        println!("📊 Checkpoint: Synced to height {}", height);
                    }
                }
            }
        });
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    if env::var("RUST_LOG").is_err() {
        unsafe {
            env::set_var("RUST_LOG", "info");
        }
    }

    println!("🌙 Midnight Unified Test Fixture Generator");
    println!("==========================================\n");

    // Parse configuration
    let config = FixtureConfig::from_env()?;

    println!("📋 Configuration:");
    println!(
        "   Wallet seed: {}",
        hex::encode(config.wallet_seed.as_bytes())
    );
    println!("   Start height: {}", config.start_height);
    println!("   Save interval: {:?}", config.save_interval);
    println!("   Network: Midnight Testnet");
    println!();

    // Create fixture directory
    fs::create_dir_all("tests/fixtures/midnight")?;

    // Set up indexer client
    let indexer_urls = IndexerUrls {
        http: "https://indexer.testnet-02.midnight.network/api/v1/graphql".to_string(),
        ws: "wss://indexer.testnet-02.midnight.network/api/v1/graphql/ws".to_string(),
    };

    println!("🌐 Connecting to Midnight testnet indexer...");
    let indexer_client = MidnightIndexerClient::new(indexer_urls);

    // Create sync manager
    let sync_state_store = Arc::new(RelayerStateRepositoryStorage::new_in_memory());

    let relayer_id = "unified-fixture-generator".to_string();

    // Set up progress monitoring if needed
    setup_progress_monitoring(
        sync_state_store.clone(),
        relayer_id.clone(),
        config.save_interval,
    );

    println!("⚙️  Creating sync manager...");
    let mut sync_manager = SyncManager::<QuickSyncStrategy>::new(
        &indexer_client,
        &config.wallet_seed,
        NetworkId::TestNet,
        sync_state_store.clone(),
        relayer_id.clone(),
    )
    .await?;

    // Perform sync
    println!("\n🔄 Starting sync from height {}...", config.start_height);
    println!("   This may take a while depending on the blockchain height and wallet activity.");

    sync_manager.sync(Some(config.start_height)).await?;
    println!("\n✅ Sync completed!");

    // Get final sync height
    let final_height = sync_state_store
        .get_last_synced_index(&relayer_id)
        .await?
        .unwrap_or(config.start_height);

    println!("📊 Final synced height: {}", final_height);
    println!();

    // Generate complete context fixture
    generate_context_fixture(&mut sync_manager, &config, final_height).await?;

    println!("\n🎉 Fixture generation complete!");
    println!("\n📁 Generated fixtures:");

    // List all files in the fixture directory
    if let Ok(entries) = fs::read_dir("tests/fixtures/midnight") {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(metadata) = fs::metadata(&path) {
                println!("   {} ({} bytes)", path.display(), metadata.len());
            }
        }
    }

    println!("\n📖 Usage in tests:");
    println!(
        "   let context_bytes = fs::read(\"tests/fixtures/midnight/context_<seed>_<height>.bin\")?;"
    );
    println!(
        "   let context = create_context_from_serialized(&context_bytes, &[seed], NetworkId::TestNet)?;"
    );

    Ok(())
}
