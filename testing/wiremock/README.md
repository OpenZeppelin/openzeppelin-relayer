# WireMock RPC Test Proxy

Transparent proxy that forwards traffic to a target RPC endpoint, with on-demand mock responses for testing transient error scenarios. Supports Stellar and EVM networks.

## Quick Start (via Claude skill)

```bash
# Setup proxy for Stellar testnet
/wiremock-test setup stellar

# Setup with custom RPC URL
/wiremock-test setup stellar https://my-custom-rpc.example.com

# Setup for EVM (Anvil on localhost:8545)
/wiremock-test setup evm

# Arm a scenario
/wiremock-test try-again-later

# Check metrics after transaction
/wiremock-test check-metrics

# Tear down
/wiremock-test teardown
```

## Manual Quick Start

```bash
# 1. Start WireMock proxy (Stellar testnet by default)
docker compose -f testing/wiremock/docker-compose.yaml up -d

# For EVM (Anvil):
WIREMOCK_PROXY_TARGET=http://host.docker.internal:8545 \
  docker compose -f testing/wiremock/docker-compose.yaml up -d

# For custom RPC:
WIREMOCK_PROXY_TARGET=https://my-rpc.example.com \
  docker compose -f testing/wiremock/docker-compose.yaml up -d

# 2. Point your relayer's network RPC URL to the proxy
#    In config/networks/stellar.json (or your local network config):
#    "rpc_urls": ["http://localhost:9090"]

# 3. Start the relayer normally — all RPC traffic flows through WireMock to the target

# 4. When ready to test, arm a scenario:
./testing/wiremock/scripts/trigger-try-again-later.sh
# or
./testing/wiremock/scripts/trigger-insufficient-fee.sh

# 5. Submit a transaction — it will hit the mock response once, then revert to real RPC

# 6. Check relayer metrics:
curl -s http://localhost:8081/debug/metrics/scrape | grep -E "insufficient_fee|try_again_later"
```

## Available Scenarios

### Generic (any network)

| Scenario | Shorthand | What it does |
|----------|-----------|-------------|
| `rpc-timeout` | `timeout` | 30s delay on next RPC call (any method) |
| `rpc-500` | `500` | HTTP 500 on next RPC call (any method) |
| `rpc-connection-reset` | `reset` | TCP connection reset on next RPC call |

### Stellar-specific

| Scenario | Shorthand | What it does |
|----------|-----------|-------------|
| `stellar-try-again-later` | `tal`, `try-again-later` | Next `sendTransaction` returns `TRY_AGAIN_LATER` |
| `stellar-insufficient-fee` | `fee`, `insufficient-fee` | Next `sendTransaction` returns `ERROR` with `TxInsufficientFee` |
| `stellar-get-transaction-not-found` | `not-found`, `get-tx-not-found` | Next `getTransaction` returns `NOT_FOUND` |

All scenarios fire **exactly once**, then revert to proxying real traffic.

## Helper Scripts

| Script | What it does |
|--------|-------------|
| `trigger-try-again-later.sh` | Arm the TRY_AGAIN_LATER scenario |
| `trigger-insufficient-fee.sh` | Arm the insufficient fee scenario |
| `e2e-fast-resubmit.sh` | Run the Stellar fast-resubmit E2E matrix |
| `check-scenarios.sh` | Show scenario states and recent requests |
| `reset-all.sh` | Reset all scenarios and clear request log |

## E2E fast-resubmit verification

Run the Stellar fast-resubmit matrix from the repo root:

```bash
./testing/wiremock/scripts/e2e-fast-resubmit.sh all
./testing/wiremock/scripts/e2e-fast-resubmit.sh -o /tmp/fast-resubmit-artifacts all
```

Run one scenario by name:

```bash
./testing/wiremock/scripts/e2e-fast-resubmit.sh if-once
./testing/wiremock/scripts/e2e-fast-resubmit.sh if-escalation
./testing/wiremock/scripts/e2e-fast-resubmit.sh if-overcap
./testing/wiremock/scripts/e2e-fast-resubmit.sh tal-window
./testing/wiremock/scripts/e2e-fast-resubmit.sh tal-window-closes
```

The harness verifies the durable regression cases for Stellar fast resubmit on testnet:

| Scenario | Injected responses | Expected behavior |
|----------|--------------------|-------------------|
| `if-once` | 1 insufficient-fee response | One fast retry after about 5s, then submission proceeds |
| `if-escalation` | 3 insufficient-fee responses | Fast retries after about 5s, 10s, and 15s |
| `if-overcap` | 7 insufficient-fee responses | Six fast retries, then failure at the retry cap |
| `tal-window` | 3 TRY_AGAIN_LATER responses | Fast retries after about 5s, 10s, and 15s |
| `tal-window-closes` | 4 TRY_AGAIN_LATER responses | Three fast retries, then status-checker handoff |

The one-shot WireMock mappings remain static. Multi-response chains are registered dynamically by
the runner at scenario arm time through `POST /__admin/mappings`, then deleted before the next
scenario. This keeps the fixture set small while preserving the same Stellar RPC response bodies.

Prerequisites:

- Docker with Compose support.
- Rust/Cargo for `cargo build --bin openzeppelin-relayer`.
- `curl` and `jq`.
- Network access to Stellar testnet, Horizon testnet, and friendbot.
- The local signer keystore is generated automatically at
  `testing/wiremock/e2e/local-signer.keystore` if missing, using
  `KEYSTORE_PASSPHRASE`, and the generated testnet account is funded through friendbot before
  the relayer starts.

The runner starts WireMock, starts a disposable Redis container for the Redis queue backend, builds
and starts `target/debug/openzeppelin-relayer`, funds the configured Stellar testnet account if
needed, runs the selected scenario set, writes artifacts, and tears the stack down.

`if-overcap` runs last in the full matrix because it intentionally never lands on-chain and can
leave the relayer-side local sequence cache ahead of chain state. `tal-window-closes` sends an
explicit 10-minute `valid_until` because it waits for status-checker handoff after the fast retry
window closes.

Artifacts default to `/tmp/oz-relayer-fast-resubmit` and can be overridden with `-o` or
`ARTIFACT_DIR`. The runner writes:

- `verdicts.json`: JSON Lines, one verdict per scenario.
- `relayer.log`: relayer stdout/stderr.
- `harness.log`: stack setup and harness diagnostics.
- `report.md`: line-count summary, deleted/kept scope, and matrix verdicts.

Each verdict line has this schema:

```json
{
  "scenario": "if-once",
  "send_calls": 2,
  "gaps_s": [5],
  "final_status": "confirmed",
  "fast_log_seen": true,
  "checker_log_seen": false,
  "pass": true,
  "notes": ""
}
```

Gap checks use a tolerance of expected minus 1 second through expected plus 8 seconds.
`tal-window-closes` treats its final checker-rescue gap as `>= 10s`.

## How It Works

WireMock runs as a transparent proxy (`--proxy-all`) to the target RPC endpoint (default: `https://soroban-testnet.stellar.org`). Pre-loaded stub mappings use **scenarios** (state machines) to intercept specific requests:

1. Scenario starts in `"Started"` state (WireMock default) — stubs require `"armed"` state, so all traffic is proxied
2. You arm a scenario by setting its state to `"armed"` via the admin API
3. Next matching request triggers the mock response and transitions back to `"Started"`
4. All subsequent requests proxy normally again

## Directory Structure

```
testing/wiremock/
  docker-compose.yaml
  mappings/
    generic/          # Scenarios that work with any JSON-RPC network
    stellar/          # Stellar-specific scenarios (sendTransaction, getTransaction)
    evm/              # Future: EVM-specific scenarios
  scripts/            # Helper scripts for arming/checking scenarios
```

## Ports

| Port | Purpose |
|------|---------|
| `9090` | Proxy endpoint (point relayer here) |
| `9090/__admin` | WireMock admin API |

## Adding Custom Scenarios

Create a JSON file in the appropriate mappings subdirectory (`generic/`, `stellar/`, or `evm/`) following the existing pattern. Key fields:

```json
{
  "scenarioName": "my-scenario",
  "requiredScenarioState": "armed",
  "newScenarioState": "Started",
  "priority": 1,
  "request": { "method": "POST", "bodyPatterns": [...] },
  "response": { "status": 200, "jsonBody": { "jsonrpc": "2.0", ... } }
}
```

Restart WireMock to load new mappings, or POST them via `/__admin/mappings`.

## Switching RPC Target

Use the `WIREMOCK_PROXY_TARGET` environment variable:

```bash
# Stellar mainnet
WIREMOCK_PROXY_TARGET=https://mainnet.sorobanrpc.com \
  docker compose -f testing/wiremock/docker-compose.yaml up -d

# Local Anvil
WIREMOCK_PROXY_TARGET=http://host.docker.internal:8545 \
  docker compose -f testing/wiremock/docker-compose.yaml up -d
```

Default (no env var): `https://soroban-testnet.stellar.org`
