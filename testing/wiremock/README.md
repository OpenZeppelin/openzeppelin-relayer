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

Run the deterministic Stellar fast-resubmit matrix from the repo root:

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

Run the production-parity channels profile with LocalStack SQS:

```bash
./testing/wiremock/scripts/e2e-fast-resubmit.sh \
  --profile channels-parity \
  --sqs-queue-type standard \
  -o /tmp/claude-501/codex-parity.lm3FQk \
  all

./testing/wiremock/scripts/e2e-fast-resubmit.sh \
  --profile channels-parity \
  --sqs-queue-type fifo \
  -o /tmp/claude-501/codex-parity.lm3FQk \
  all
```

`channels-parity` derives `testing/wiremock/e2e/env.channels-parity` from the sanitized channels production env JSON in the artifact directory and substitutes local-only values: LocalStack SQS endpoint and queue URL prefix, Docker Redis for repository storage, testnet, WireMock RPC, and dev secrets. The env file is gitignored by `testing/wiremock/.gitignore`. The parity honesty ledger is written to:

```text
/tmp/claude-501/codex-parity.lm3FQk/parity-substitutions.md
```

The SQS queue type is explicit because the production queue type was not known from the sanitized env. Use `standard` to test native `DelaySeconds`; use `fifo` to test the worker-side `target_scheduled_on` visibility-timeout deferral path.

Run soak mode:

```bash
./testing/wiremock/scripts/e2e-fast-resubmit.sh soak \
  --duration 120 \
  --if-pct 40 \
  --tal-pct 20 \
  --relayers 10 \
  --profile channels-parity \
  --sqs-queue-type standard \
  -o /tmp/claude-501/codex-soak4.SaB8rK
```

Soak mode is closed-loop to match the production channels plugin ordering model. `--relayers N` defaults to 10; soak generates N relayers with one local signer keystore each under `testing/wiremock/e2e/generated-relayers/`, funds each account with friendbot retry, and starts one independent loop per account. Each loop submits one transaction, polls that transaction until it reaches a terminal status, then immediately submits the next transaction if the duration window is still open. There is no open-loop arrival rate; throughput emerges from the number of relayer accounts and the observed terminal latency.

This matters for Stellar because sequence numbers are strict per source account. An open-loop harness can queue multiple transactions behind a retrying head transaction, causing head-of-line `TxBadSeq`/`TxTooLate` failures that do not reflect the production channels flow. Closed-loop soak keeps at most one in-flight transaction per channel account.

The verdict reports achieved rejection rates from the WireMock journal, memo-attributed rescue residuals, duplicate sends within 1s for the same tx with safe/unsafe classification, IF cohort success rate, clean cohort success rate, expired-without-any-submission count, head-of-line `TxBadSeq` count, per-relayer creation/send distribution, and SQS submission queue depth when observable. The aggregate verdict is written to `soak-verdict.json`; tx-attributed send events and classified double-fire evidence are written beside it.

Soak pass criteria are: clean cohort success rate at least 98%, insufficient-fee cohort success rate at least 90%, p95 rescue residual at or below 10s, zero unsafe same-tx double fires less than 1s apart, and zero expired transactions that never reached `sendTransaction`.

Rescue residuals subtract the designed fast-resubmit delay from the observed gap between a rejected `sendTransaction` and the next same-memo send. Insufficient-fee attempt `n` expects a `5s * n` delay through the retry cap. TRY_AGAIN_LATER attempts 1-3 also expect `5s * n`; later TRY_AGAIN_LATER sends are status-check ladder territory and are reported separately instead of scored against fast-resubmit residuals. This keeps designed 20s/25s escalation delays from being misread as latency violations.

Same-tx double fires are classified rather than failed unconditionally. A pair is safe when the envelope is unchanged and the transaction still reaches terminal success, or when the extra send is accepted/duplicate. These near-simultaneous deliveries can happen under submission-queue backlog; Stellar treats a repeated identical envelope as idempotent, so the pass criterion is zero unsafe pairs, not zero total pairs.

Prerequisites:

- Docker with Compose support.
- Rust/Cargo for `cargo build --bin openzeppelin-relayer`.
- `curl`, `jq`, and the AWS CLI for the LocalStack SQS profile.
- Network access to Stellar testnet, Horizon testnet, and friendbot.

The runner starts WireMock with `testing/wiremock/docker-compose.yaml`, starts a disposable Docker Redis container on a random localhost port, builds the relayer, and runs `target/debug/openzeppelin-relayer`. Matrix runs use `CONFIG_DIR=testing/wiremock/e2e` and `CONFIG_FILE_NAME=fast-resubmit-config.json`; soak runs use a generated per-run config directory under `testing/wiremock/e2e/generated-relayers/`.

The full matrix runs `if-overcap` last for isolation. That scenario intentionally exhausts fast-resubmit attempts without ever landing the transaction on-chain, so the relayer-side local sequence cache advances while the chain sequence does not. Running it before other scenarios can make later transactions start from a stale high sequence and thrash through `TxBadSeq` until `TxTooLate`.

`tal-window-closes` sends an explicit 10-minute `valid_until` in every profile and uses a longer poll timeout. This is a scenario-scoped validity exception: the case verifies checker handoff mechanics, and the ladder rescue can land at roughly 110s+ transaction age, which collides with default Stellar envelope validity plus confirmation latency.

The script discovers the relayer account from `GET /api/v1/relayers/stellar-fast-resubmit`. If the account is missing on testnet, it funds it with:

```bash
curl "https://friendbot.stellar.org/?addr=<address>"
```

Artifacts default to `/tmp/claude-501/codex-parity.lm3FQk` and can be overridden with `-o` or `ARTIFACT_DIR`. The runner writes:

- `verdicts.json`: JSON Lines, one verdict per scenario.
- `verdicts-parity-standard.json`: JSON Lines for `channels-parity --sqs-queue-type standard`.
- `verdicts-parity-fifo.json`: JSON Lines for `channels-parity --sqs-queue-type fifo`.
- `soak-verdict.json`: aggregate soak verdict.
- `relayer.log`: relayer stdout/stderr.
- `harness.log`: stack setup and harness diagnostics.
- `report.md`: compact run summary.

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

Gap checks use the requested tolerance of expected minus 1 second through expected plus 8 seconds to absorb LocalStack FIFO visibility-timeout jitter. `tal-window-closes` treats its final checker-rescue gap as `>= 10s`.

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
