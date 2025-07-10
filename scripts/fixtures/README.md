# Midnight Test Fixture Scripts

This directory contains scripts for generating test fixtures for the Midnight blockchain integration.

## Prerequisites

1. A Midnight testnet wallet seed (32-byte hex string)
2. The wallet should have some tDUST tokens for meaningful test fixtures
3. Access to Midnight testnet (the examples use the public testnet endpoints)

## Scripts

### generate_midnight_fixtures.rs

A unified fixture generator that creates complete context fixtures (including both wallet state and ledger state) for testing.

```bash
# Generate complete context fixture
WALLET_SEED=your_32_byte_hex_seed cargo run --bin generate_midnight_fixtures

# Starting sync from a specific height
WALLET_SEED=your_seed START_HEIGHT=1000 cargo run --bin generate_midnight_fixtures

# With progress tracking
WALLET_SEED=your_seed SAVE_INTERVAL=1000 cargo run --bin generate_midnight_fixtures
```

### generate_midnight_fixtures.sh

A convenient shell script wrapper that provides a user-friendly interface to the fixture generator:

```bash
# Run the script (it will check for WALLET_SEED)
./scripts/fixtures/generate_midnight_fixtures.sh

# Or with environment variables
WALLET_SEED=your_seed START_HEIGHT=1000 ./scripts/fixtures/generate_midnight_fixtures.sh
```

Environment variables:

- `WALLET_SEED`: 32-byte hex string (required)
- `START_HEIGHT`: Blockchain height to start sync from (default: 0)
- `SAVE_INTERVAL`: Save progress every N blocks (optional)
- `RUST_LOG`: Log level (default: info)

The fixtures will be saved to: `tests/fixtures/midnight/`

- `context_<seed_hex>_<height>.bin` - Complete context fixture (includes wallet + ledger state)

## Using the Fixtures

Once generated, these fixtures can be used in tests:

```rust
use openzeppelin_relayer::services::sync::midnight::test_utils::create_context_from_serialized;

// Load a complete context fixture
let context_bytes = fs::read("tests/fixtures/midnight/context_<seed>_<height>.bin")?;
let context = create_context_from_serialized(&context_bytes, &[seed], NetworkId::TestNet)?;

// Or in transaction tests, the helper functions will automatically load context fixtures:
let sync_manager = create_sync_manager_with_fixture(&wallet_seed, &network, relayer_id);
```

## Important Notes

1. **Testnet Only**: These examples connect to Midnight testnet. Never use mainnet wallets.
2. **Fund the Wallet**: Make sure your wallet has received tDUST tokens before generating fixtures.
3. **Sync Time**: Initial sync can take a while depending on the blockchain height.
4. **Storage**: Fixtures can be large if the wallet has extensive history.

## Troubleshooting

- If sync fails, check your internet connection and that the testnet endpoints are accessible.
- If the wallet state shows `first_free = 0`, the wallet is empty and needs funding.
- For detailed logs, set `RUST_LOG=debug` before running.
