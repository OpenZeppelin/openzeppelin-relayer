# Integration Testing Guide

This guide covers how to run, create, and maintain integration tests for the OpenZeppelin Relayer.

## Table of Contents

- [Quick Start](#quick-start)
- [Prerequisites](#prerequisites)
- [Configuration](#configuration)
- [Running Tests](#running-tests)
- [Writing Tests](#writing-tests)
- [Test Structure](#test-structure)
- [Test Registry](#test-registry-registryjson)
- [Troubleshooting](#troubleshooting)
- [CI Integration](#ci-integration)

---

## Quick Start

### Local Mode (Default)

Uses local Anvil node - no testnet funds needed!

```bash
# 1. One-time setup: Create Anvil keystore
cast wallet import anvil-test \
  --private-key PK \
  --keystore-dir tests/integration/config/local/keys \
  --unsafe-password "test"

mv tests/integration/config/local/keys/anvil-test \
   tests/integration/config/local/keys/anvil-test.json

# 2. Copy and configure environment
cp .env.integration.example .env.integration
# Edit .env.integration with your API key (any value works for local mode)

# 3. Run tests via Docker
./scripts/run-integration-docker.sh

# Note: Docker mode uses "anvil:8545" for RPC URL (Docker service name).
# The config at tests/integration/config/local/config.json is already set up correctly.
```

### Standalone Mode (Local Development)

For faster iteration with `cargo run` and `cargo test`:

```bash
# 1. Configure RPC URL for standalone mode
# Edit config/networks/local-anvil.json to use localhost instead of Docker service:
# Change "rpc_urls": ["http://anvil:8545"]
# To:     "rpc_urls": ["http://localhost:8545"]
#
# Note: This is only needed for standalone mode. Docker mode uses "anvil:8545"

# 2. Add Anvil relayer to your config/config.json
# You can copy the relayer and signer configuration from:
# tests/integration/config/local-standalone/config.json
#
# Add the relayer entry to the "relayers" array in config/config.json:
# {
#   "id": "anvil-relayer",
#   "name": "Standalone Anvil Relayer",
#   "network": "localhost",
#   "paused": false,
#   "signer_id": "anvil-signer",
#   "network_type": "evm",
#   "policies": { "min_balance": 0 }
# }
#
# Also add the signer entry to the "signers" array:
# {
#   "id": "anvil-signer",
#   "type": "local",
#   "config": {
#     "path": "tests/integration/config/local/keys/anvil-test.json",
#     "passphrase": { "type": "plain", "value": "test" }
#   }
# }

# 3. Start Anvil and deploy contracts
./scripts/anvil-local.sh start

# 4. In another terminal, run relayer
cargo run

# 5. In another terminal, run tests
cargo test --features integration-tests --test integration

# 6. When done, stop Anvil
./scripts/anvil-local.sh stop

# 7. Don't forget to revert config/networks/local-anvil.json back to "anvil:8545"
#    if you want to use Docker mode again
```

### Testnet Mode

For testing against live testnet networks:

```bash
# 1. Copy and configure environment
cp .env.integration.example .env.integration
# Edit .env.integration with your API key and passphrase

# 2. Copy and configure the testnet config
cp tests/integration/config/config.example.json tests/integration/config/testnet/config.json
cp tests/integration/config/registry.example.json tests/integration/config/testnet/registry.json
# Edit registry.json to enable the networks you want to test

# 3. Run tests via Docker
MODE=testnet ./scripts/run-integration-docker.sh
```

---

## Prerequisites

### Docker-based Testing (Recommended)

- Docker and Docker Compose
- `.env.integration` file configured
- Config files set up for your mode:
  - **Local Mode (Anvil)**: `tests/integration/config/local/config.json` and `registry.json`
  - **Testnet Mode**: `tests/integration/config/testnet/config.json` and `registry.json`
  - Copy from `config.example.json` and `registry.example.json` as needed

### Local Testing

- Rust 1.88+
- Running Redis instance
- Running Relayer service
- Funded test wallet on target networks

---

## Configuration

### Environment Variables

Create `.env.integration` from the example:

```bash
cp .env.integration.example .env.integration
```

The `.env.integration` file is used **only for API keys and secrets**:

| Variable              | Description                        | Example                                |
| --------------------- | ---------------------------------- | -------------------------------------- |
| `API_KEY`             | Relayer API authentication key     | `ecaa0daa-f87e-4044-96b8-986638bf92d5` |
| `KEYSTORE_PASSPHRASE` | Password for local signer keystore | `your-secure-passphrase`               |
| `WEBHOOK_SIGNING_KEY` | Webhook signing key (UUID)         | `your-webhook-signing-key-here`        |
| `LOG_LEVEL`           | Logging verbosity                  | `info`                                 |

### Test Registry Setup

Create mode-specific `registry.json` from the example template:

```bash
# For Local Mode (Anvil with Docker)
cp tests/integration/config/registry.example.json tests/integration/config/local/registry.json

# For Testnet Mode
cp tests/integration/config/registry.example.json tests/integration/config/testnet/registry.json
```

The example file comes with all networks disabled by default (except `base-sepolia`). Enable the networks you want to test by setting `"enabled": true`.

> **Note:** Mode-specific `registry.json` files (`local/registry.json`, `testnet/registry.json`) are gitignored to allow local customization without affecting the repository.

### Network Selection

**Network selection is controlled via mode-specific `registry.json` files**, not environment variables.

To enable or disable networks for testing, edit the `enabled` flag in the appropriate registry file:

- Local Mode: `tests/integration/config/local/registry.json`
- Testnet Mode: `tests/integration/config/testnet/registry.json`

```json
{
  "networks": {
    "sepolia": {
      "enabled": true // ✅ This network will run
    },
    "base-sepolia": {
      "enabled": true // ✅ This network will run
    },
    "bsc-testnet": {
      "enabled": false // ❌ This network will be skipped
    }
  }
}
```

Only networks with `"enabled": true` will be included in test runs.

### Test Logging

Test output is controlled by the `RUST_LOG` environment variable:

```bash
# Default: Show info-level logs from all test modules
cargo test --features integration-tests --test integration

# Show debug logs for more detail
RUST_LOG=debug cargo test --features integration-tests --test integration -- --nocapture

# Show only warnings and errors
RUST_LOG=warn cargo test --features integration-tests --test integration
```

**Note:** Use `--nocapture` flag to see logs in real-time during test execution.

### Local Signer

Test signer keystores are located in mode-specific directories:

- **Local Mode (Anvil)**: `tests/integration/config/local/keys/anvil-test.json`
- **Testnet Mode**: `tests/integration/config/testnet/keys/local-signer.json`

These are Ethereum keystore files encrypted with their respective passphrases.

To create a new signer keystore:

```bash
# For Local Mode (Anvil) - using cast wallet import
cast wallet import anvil-test \
  --private-key YOUR_PRIVATE_KEY \
  --keystore-dir tests/integration/config/local/keys \
  --unsafe-password "PASSWORD"

# Rename to .json extension
mv tests/integration/config/local/keys/anvil-test \
   tests/integration/config/local/keys/anvil-test.json

# For Testnet Mode - using the recommended key generation tool
cargo run --example create_key -- \
  --password "DEFINE_YOUR_PASSWORD" \
  --output-dir tests/integration/config/testnet/keys \
  --filename local-signer.json
```

Then update the `KEYSTORE_PASSPHRASE` field in your `.env.integration` file with the password you used.

---

## Running Tests

### Via Docker (Recommended)

The Docker setup handles Redis, Relayer, and test execution automatically.

```bash
# Run all tests
./scripts/run-integration-docker.sh

# Build images only
./scripts/run-integration-docker.sh build

# Stop services
./scripts/run-integration-docker.sh down

# View logs
./scripts/run-integration-docker.sh logs

# Open shell in test container
./scripts/run-integration-docker.sh shell

# Clean up everything
./scripts/run-integration-docker.sh clean
```

#### Testing Specific Networks

To test specific networks, edit `tests/integration/config/registry.json` and set `"enabled": true` only for the networks you want to test:

```json
{
  "networks": {
    "sepolia": {
      "enabled": true // Only this network will run
    },
    "base-sepolia": {
      "enabled": false // Disabled
    }
  }
}
```

```

---

## Writing Tests

### Directory Structure

The integration tests are organized for easy navigation:

```

tests/integration/
├── README.md # This file
├── tests/ # ✅ ALL test files go here
│ ├── mod.rs
│ ├── authorization.rs # API authorization tests
│ └── evm/ # EVM network tests
│ ├── mod.rs
│ ├── basic_transfer.rs
│ └── contract_interaction.rs
├── common/ # 🔧 Shared utilities and helpers
│ ├── mod.rs
│ ├── client.rs # RelayerClient for API calls
│ ├── confirmation.rs # Transaction confirmation helpers
│ ├── context.rs # Multi-network test runner
│ ├── evm_helpers.rs # EVM-specific utilities
│ ├── network_selection.rs
│ └── registry.rs # Test registry utilities
├── config/ # ⚙️ Configuration files
│ ├── config.example.json
│ └── registry.example.json
└── contracts/ # 📜 Smart contracts (Foundry)
├── README.md
├── foundry.toml
└── src/

```

**Key principle:** Look in `tests/` for test files, `common/` for helpers.

---

## Test Structure

### Naming Conventions

- Test files: `snake_case.rs`
- Test functions: `test_<feature>_<scenario>`
- Integration tests are gated behind the `integration-tests` feature flag

### Test Categories

| Category      | Location                           | Description          |
| ------------- | ---------------------------------- | -------------------- |
| API Tests     | `tests/integration/tests/`         | Test REST endpoints  |
| EVM Tests     | `tests/integration/tests/evm/`     | EVM chain operations |
| Solana Tests  | `tests/integration/tests/solana/`  | Solana operations    |
| Stellar Tests | `tests/integration/tests/stellar/` | Stellar operations   |

### Test Contracts

Pre-deployed test contracts are in `tests/integration/contracts/`:

| Contract      | Address (Sepolia)                            | Purpose                |
| ------------- | -------------------------------------------- | ---------------------- |
| SimpleStorage | `0x5379E27d181a94550318d4A44124eCd056678879` | Basic read/write tests |

---

## Test Registry (`registry.json`)

The test registry is a centralized configuration file that stores network-specific test data including signer addresses, deployed contract addresses, and network metadata. This eliminates hardcoded values and makes it easy to add new networks.

### Location

```

tests/integration/config/
├── config.example.json # Template relayer config (tracked in git)
├── registry.example.json # Template registry config (tracked in git)
├── local/ # Local Mode (Anvil + Docker) configs
│ ├── config.json # Relayer config (gitignored)
│ ├── registry.json # Registry config (gitignored)
│ └── keys/
│ └── anvil-test.json # Anvil keystore (gitignored)
├── local-standalone/ # Standalone Mode config reference
│ └── config.json # Reference config for copying to main config
└── testnet/ # Testnet Mode configs
├── config.json # Relayer config (gitignored)
├── registry.json # Registry config (gitignored)
└── keys/
└── local-signer.json # Testnet keystore (gitignored)

````

Create your mode-specific config files from the examples:

```bash
# For Local Mode (Anvil + Docker)
cp tests/integration/config/config.example.json tests/integration/config/local/config.json
cp tests/integration/config/registry.example.json tests/integration/config/local/registry.json

# For Testnet Mode
cp tests/integration/config/config.example.json tests/integration/config/testnet/config.json
cp tests/integration/config/registry.example.json tests/integration/config/testnet/registry.json
````

### Schema

```json
{
  "networks": {
    "<network-key>": {
      "network_name": "string", // Network identifier used by relayer
      "network_type": "string", // "evm", "solana", or "stellar"
      "contracts": {
        "<contract_name>": "address" // Deployed contract addresses
      },
      "min_balance": "string", // Minimum balance required (in native token)
      "enabled": true // Whether network is active for testing
    }
  }
}
```

### Example Entry

```json
{
  "networks": {
    "sepolia": {
      "network_name": "sepolia",
      "network_type": "evm",
      "contracts": {
        "simple_storage": "0x5379E27d181a94550318d4A44124eCd056678879",
        "test_erc20": "0x0000000000000000000000000000000000000000"
      },
      "min_balance": "0.1",
      "enabled": true
    }
  }
}
```

### Placeholder Addresses

Use placeholder addresses for contracts not yet deployed:

- **EVM:** `0x0000000000000000000000000000000000000000`

The registry automatically detects placeholders and skips tests requiring those contracts.

### Adding a New Network

1. Add entry to the appropriate `registry.json` file:
   - Local Mode: `tests/integration/config/local/registry.json`
   - Testnet Mode: `tests/integration/config/testnet/registry.json`

```json
{
  "networks": {
    "arbitrum-sepolia": {
      "network_name": "arbitrum-sepolia",
      "network_type": "evm",
      "contracts": {
        "simple_storage": "0x0000000000000000000000000000000000000000"
      },
      "min_balance": "0.01",
      "enabled": true
    }
  }
}
```

2. Add a corresponding relayer entry to your `config.json` file with the appropriate signer configuration

3. Deploy test contracts and update addresses in the registry

4. Fund the signer wallet on the new network

5. Ensure `"enabled": true` is set for the new network

6. Run tests: `./scripts/run-integration-docker.sh`

---

## Troubleshooting

### Common Issues

#### MacMismatch Error

The keystore passphrase doesn't match:

```
Error: MacMismatch
```

**Fix:** Ensure `KEYSTORE_PASSPHRASE` in `.env.integration` matches the password used to create the keystore.

#### Connection Refused

```
Error: Connection refused (os error 111)
```

**Fix:** Ensure Redis and Relayer are running. For Docker, check container status:

```bash
docker-compose -f docker-compose.integration.yml ps
```

#### Insufficient Funds

```
Error: insufficient funds for transfer
```

**Fix:** Fund the test wallet. Get the address:

```bash
# Check relayer logs for the signer address
docker-compose -f docker-compose.integration.yml logs relayer | grep address
```

Then use a testnet faucet to fund it.

#### Network Timeout

```
Error: Transaction confirmation timeout
```

**Fix:**

- Check network RPC is accessible
- Increase timeout in test
- Verify the network isn't congested

### Debugging

#### View Container Logs

```bash
# All services
./scripts/run-integration-docker.sh logs

# Specific service
docker-compose -f docker-compose.integration.yml logs relayer
docker-compose -f docker-compose.integration.yml logs integration-tests
```

#### Interactive Shell

```bash
./scripts/run-integration-docker.sh shell
```

#### Run Single Test with Verbose Output

```bash
RUST_LOG=debug \
  cargo test --features integration-tests --test integration test_name -- --nocapture
```

### Cleanup

If tests leave behind resources:

```bash
# Remove all Docker resources
./scripts/run-integration-docker.sh clean

# Or manually
docker-compose -f docker-compose.integration.yml down -v --remove-orphans
```

---

## CI Integration

The integration tests run automatically in CI using the network configuration from `tests/integration/config/registry.json`.

Control which networks run in CI by setting their `enabled` flags in `tests/integration/config/registry.json`:

```json
{
  "networks": {
    "base-sepolia": {
      "enabled": true // ✅ Runs in CI
    },
    "sepolia": {
      "enabled": false // ❌ Skipped in CI
    }
  }
}
```

The test script returns appropriate exit codes:

- `0` - All tests passed
- `1` - Tests failed or configuration error
