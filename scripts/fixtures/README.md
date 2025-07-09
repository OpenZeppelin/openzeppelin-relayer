# Midnight Test Fixture Scripts

This directory contains scripts for generating test fixtures for the Midnight blockchain integration.

## Prerequisites

1. A Midnight testnet wallet seed (32-byte hex string)
2. The wallet should have some tDUST tokens for meaningful test fixtures
3. Access to Midnight testnet (the examples use the public testnet endpoints)

## Scripts

### generate_midnight_fixtures

The unified fixture generator that can create both wallet state and complete context fixtures.

```bash
# Generate wallet state fixture only (default)
WALLET_SEED=your_32_byte_hex_seed cargo run --bin generate_midnight_fixtures

# Generate complete context fixture only
WALLET_SEED=your_seed CONTEXT=true cargo run --bin generate_midnight_fixtures

# Generate both wallet and context fixtures
WALLET_SEED=your_seed CONTEXT=both cargo run --bin generate_midnight_fixtures

# Starting sync from a specific height
WALLET_SEED=your_seed START_HEIGHT=1000 cargo run --bin generate_midnight_fixtures

# With progress tracking (context mode only)
WALLET_SEED=your_seed CONTEXT=true SAVE_INTERVAL=1000 cargo run --bin generate_midnight_fixtures
```

Environment variables:

- `WALLET_SEED`: 32-byte hex string (required)
- `START_HEIGHT`: Blockchain height to start sync from (default: 0)
- `CONTEXT`: "true" for context only, "both" for both types (default: wallet only)
- `SAVE_INTERVAL`: Save progress every N blocks (context mode only)
- `RUST_LOG`: Log level (default: info)

The fixtures will be saved to: `tests/fixtures/midnight/`

- `wallet_<seed_hex>.bin` - Wallet state fixture
- `context_<seed_hex>_<height>.bin` - Complete context fixture
- `stored_context_<seed_hex>_<height>.bin` - Context from sync state (if available)

## Using the Fixtures

Once generated, these fixtures can be used in tests:

```rust
use openzeppelin_relayer::services::sync::midnight::test_utils::{
    create_funded_test_context,
    create_context_from_serialized,
};

// The test_utils will automatically load wallet fixtures
let context = create_funded_test_context(&[seed], initial_balance);

// Or load a complete context
let context_bytes = fs::read("tests/fixtures/midnight/context_..._....bin")?;
let context = create_context_from_serialized(&context_bytes, &[seed], NetworkId::TestNet)?;
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
