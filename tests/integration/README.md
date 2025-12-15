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

```bash
# 1. Copy and configure environment
cp .env.integration.example .env.integration
# Edit .env.integration with your API key and passphrase

# 2. Run tests via Docker
./scripts/run-integration-docker.sh
```

---

## Prerequisites

### Docker-based Testing (Recommended)

- Docker and Docker Compose
- `.env.integration` file configured

### Local Testing

- Rust 1.86+
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

### Network Selection

**Network selection is controlled via `tests/integration/registry.json`**, not environment variables.

To enable or disable networks for testing, edit the `enabled` flag in `registry.json`:

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

# Show specific module logs
RUST_LOG=integration::common::context=debug cargo test --features integration-tests --test integration -- --nocapture
```

**Note:** Use `--nocapture` flag to see logs in real-time during test execution.

### Local Signer

The test signer keystore is located at `tests/integration/config/keys/local-signer.json`. This is an Ethereum keystore file encrypted with the `KEYSTORE_PASSPHRASE`.

To create a new signer:

```bash
cargo run --example create_key -- \
  --password "your-passphrase" \
  --output-dir tests/integration/config/keys \
  --filename local-signer.json
```

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

To test specific networks, edit `tests/integration/registry.json` and set `"enabled": true` only for the networks you want to test:

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

### Via Cargo (Local)

Requires Redis and Relayer running locally.

```bash
# Start Redis
docker run -d --name redis -p 6379:6379 redis:7-alpine

# Start Relayer (in another terminal)
cargo run

# Run integration tests
cargo test --features integration-tests --test integration -- --nocapture

# Run specific test
cargo test --features integration-tests --test integration test_evm_basic_transfer -- --nocapture

# Note: Network selection is controlled via registry.json, not environment variables
```

---

## Writing Tests

### Test File Location

```
tests/integration/
├── mod.rs                 # Main module, declares submodules
├── common/                # Shared utilities
│   ├── mod.rs
│   ├── client.rs          # RelayerClient for API calls
│   ├── registry.rs        # RelayerRegistry for test isolation
│   ├── network_selection.rs
│   └── confirmation.rs    # Transaction confirmation helpers
├── api/                   # API endpoint tests
│   ├── mod.rs
│   ├── health.rs
│   └── relayer.rs
└── networks/              # Network-specific tests
    ├── mod.rs
    └── evm/
        ├── mod.rs
        ├── helpers.rs     # EVM-specific utilities
        ├── basic_transfer.rs
        └── contract_interaction.rs
```

---

## Test Structure

### Naming Conventions

- Test files: `snake_case.rs`
- Test functions: `test_<feature>_<scenario>`
- Integration tests are gated behind the `integration-tests` feature flag

### Test Categories

| Category      | Location                              | Description          |
| ------------- | ------------------------------------- | -------------------- |
| API Tests     | `tests/integration/api/`              | Test REST endpoints  |
| EVM Tests     | `tests/integration/networks/evm/`     | EVM chain operations |
| Solana Tests  | `tests/integration/networks/solana/`  | Solana operations    |
| Stellar Tests | `tests/integration/networks/stellar/` | Stellar operations   |

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
tests/integration/registry.json
```

### Schema

```json
{
  "networks": {
    "<network-key>": {
      "network_name": "string", // Network identifier used by relayer
      "network_type": "string", // "evm", "solana", or "stellar"
      "signer": {
        "id": "string", // Signer ID from config.json
        "address": "string" // Derived wallet address
      },
      "contracts": {
        "<contract_name>": "address" // Deployed contract addresses
      },
      "min_balance": "string", // Minimum balance required (in native token)
      "tags": ["string"], // Selection tags: "quick", "ci", "evm", etc.
      "enabled": true // Whether network is active for testing
    }
  },
  "_metadata": {
    "description": "string",
    "version": "string",
    "last_updated": "YYYY-MM-DD"
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
      "signer": {
        "id": "local-signer",
        "address": "0x0a427c6ffdc5588f9dc155c4e89ad0c15daa2655"
      },
      "contracts": {
        "simple_storage": "0x5379E27d181a94550318d4A44124eCd056678879",
        "test_erc20": "0x0000000000000000000000000000000000000000"
      },
      "min_balance": "0.1",
      "tags": ["quick", "ci", "evm", "baseline"],
      "enabled": true
    }
  }
}
```

### Tags

Tags provide metadata and categorization for networks:

| Tag        | Description                        |
| ---------- | ---------------------------------- |
| `evm`      | EVM-compatible network             |
| `solana`   | Solana network                     |
| `stellar`  | Stellar network                    |
| `baseline` | Primary test network for each type |
| `rollup`   | Layer 2 rollup network             |
| `testnet`  | Testnet environment                |

**Note:** Tags are informational only. Network selection is controlled solely by the `enabled` flag.

### Placeholder Addresses

Use placeholder addresses for contracts not yet deployed:

- **EVM:** `0x0000000000000000000000000000000000000000`

The registry automatically detects placeholders and skips tests requiring those contracts.

### Adding a New Network

1. Add entry to `registry.json`:

```json
{
  "networks": {
    "arbitrum-sepolia": {
      "network_name": "arbitrum-sepolia",
      "network_type": "evm",
      "signer": {
        "id": "local-signer",
        "address": "0x0a427c6ffdc5588f9dc155c4e89ad0c15daa2655"
      },
      "contracts": {
        "simple_storage": "0x0000000000000000000000000000000000000000"
      },
      "min_balance": "0.01",
      "tags": ["ci", "evm", "rollup"],
      "enabled": true
    }
  }
}
```

2. Deploy test contracts and update addresses

3. Fund the signer wallet on the new network

4. Ensure `"enabled": true` is set for the new network

5. Run tests: `./scripts/run-integration-docker.sh`

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

The integration tests run automatically in CI using the network configuration from `registry.json`.

Control which networks run in CI by setting their `enabled` flags in `registry.json`:

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
