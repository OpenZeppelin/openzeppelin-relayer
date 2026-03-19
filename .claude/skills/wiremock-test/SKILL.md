---
name: wiremock-test
description: Manage WireMock proxy for RPC testing. Starts the proxy, arms scenarios to simulate transient errors (timeouts, 500s, connection resets, and network-specific errors like Stellar TRY_AGAIN_LATER), and checks metrics. Use to verify relayer behavior under failure conditions.
allowed-tools: Bash, Read, Grep, Glob, Agent
argument-hint: "<scenario> | setup <network> [rpc-url] | teardown | status | check-metrics"
---

# WireMock RPC Test Proxy

You manage a WireMock proxy that sits between the relayer and an RPC endpoint. It forwards traffic normally, but can inject mock error responses on demand.

## Project paths

- Docker compose: `testing/wiremock/docker-compose.yaml`
- Generic mappings: `testing/wiremock/mappings/generic/` (work with any JSON-RPC network)
- Stellar mappings: `testing/wiremock/mappings/stellar/`
- Future EVM mappings: `testing/wiremock/mappings/evm/` (not yet created)
- Helper scripts: `testing/wiremock/scripts/`

## Available scenarios

### Generic (any network)

| Scenario name | Shorthand | What it does |
|---|---|---|
| `rpc-timeout` | `timeout` | 30s delay on next RPC call (any method) |
| `rpc-500` | `500` | HTTP 500 on next RPC call |
| `rpc-connection-reset` | `reset` | TCP connection reset on next RPC call |

### Stellar-specific

| Scenario name | Shorthand | What it does |
|---|---|---|
| `stellar-try-again-later` | `tal`, `try-again-later` | Returns `TRY_AGAIN_LATER` on next `sendTransaction` |
| `stellar-insufficient-fee` | `fee`, `insufficient-fee` | Returns `ERROR` with `TxInsufficientFee` on next `sendTransaction` |
| `stellar-get-transaction-not-found` | `not-found`, `get-tx-not-found` | Returns `NOT_FOUND` for next `getTransaction` |

All scenarios fire **exactly once** then revert to proxying real traffic.

## How to handle user arguments ($ARGUMENTS)

Based on the argument, execute the matching flow:

### `setup` — requires a network argument
**Syntax:** `setup <network> [rpc-url]`

A bare `setup` with no network is **not allowed** — ask the user which network they want (stellar or evm).

1. Determine target RPC:
   - `setup stellar` → default: `https://soroban-testnet.stellar.org`
   - `setup evm` or `setup anvil` → default: `http://host.docker.internal:8545`
   - `setup stellar <url>` or `setup evm <url>` → use the provided URL as `WIREMOCK_PROXY_TARGET`
2. Start WireMock:
   ```bash
   WIREMOCK_PROXY_TARGET=<target> docker compose -f testing/wiremock/docker-compose.yaml up -d
   ```
   For stellar with default target, just: `docker compose -f testing/wiremock/docker-compose.yaml up -d`
3. Wait for health: poll `curl -sf http://localhost:9090/__admin/health` (retry up to 10 times with 2s sleep)
4. Verify scenarios loaded: `curl -s http://localhost:9090/__admin/scenarios`
5. Remind the user:
   - WireMock proxy is running on `http://localhost:9090` proxying to `<target>`
   - To use it, update the relevant network config `rpc_urls` to `["http://localhost:9090"]`
   - If the relayer runs in Docker, use `http://host.docker.internal:9090` instead
   - Show only the **relevant** scenarios: generic + the chosen network's specific scenarios
   - For Stellar: mention updating `config/networks/stellar.json` testnet entry
   - For EVM: mention updating the relevant EVM network config

### `teardown` or `stop`
1. Run: `docker compose -f testing/wiremock/docker-compose.yaml down`
2. Confirm stopped

### `status`
1. Check if WireMock is running: `docker compose -f testing/wiremock/docker-compose.yaml ps`
2. If running, show scenario states: `curl -s http://localhost:9090/__admin/scenarios`
3. Show recent requests: `curl -s 'http://localhost:9090/__admin/requests?limit=10'`
4. Parse and display in a readable format

### `check-metrics` or `metrics`
1. Fetch relayer metrics: `curl -s http://localhost:8081/debug/metrics/scrape`
2. Filter and display lines matching: `insufficient_fee|try_again_later|transactions_success|transactions_failed|stellar_submission_failures|stellar_try_again_later|rpc_call_latency|api_rpc_failures`
3. Summarize what the metrics show

### A scenario name or shorthand
1. First ensure WireMock is running (check health endpoint, start if needed)
2. Resolve the scenario name — the user may use shorthand:
   - Generic: `timeout` → `rpc-timeout`, `500` → `rpc-500`, `reset` → `rpc-connection-reset`
   - Stellar: `try-again-later` or `tal` → `stellar-try-again-later`, `insufficient-fee` or `fee` → `stellar-insufficient-fee`, `not-found` or `get-tx-not-found` → `stellar-get-transaction-not-found`
3. Arm the scenario:
   ```bash
   curl -s -X PUT "http://localhost:9090/__admin/scenarios/{scenario-name}/state" \
     -H 'Content-Type: application/json' -d '{"state": "armed"}'
   ```
4. Verify it's armed by checking the scenario state
5. Tell the user what will happen on the next matching RPC call

### `chaos` or `all-chaos`
1. Ensure WireMock is running
2. Arm ALL scenarios simultaneously
3. Explain what will happen: scenarios are priority-based, so the most specific one fires first

### `reset`
1. Reset all scenarios: `curl -s -X POST http://localhost:9090/__admin/scenarios/reset`
2. Clear request log: `curl -s -X DELETE http://localhost:9090/__admin/requests`
3. Confirm all scenarios are back to initial (inactive) state

## Important reminders

Always remind the user about these when relevant:
- The relayer's network `rpc_urls` must point to `http://localhost:9090` (or `http://host.docker.internal:9090` from Docker) for WireMock to intercept traffic
- After testing, they should revert `rpc_urls` back to the real RPC endpoint
- Scenarios are one-shot: they fire once and revert. Re-arm to test again.
- The relayer's metrics endpoint is at `http://localhost:8081/debug/metrics/scrape` (configurable via METRICS_PORT)
- Generic scenarios (timeout, 500, connection-reset) work with any network — no need to change them when switching between Stellar and EVM testing

## Adding new network support

When adding EVM or other network-specific scenarios:
1. Create `testing/wiremock/mappings/<network>/` directory
2. Add scenario JSON files following the existing pattern (use `"requiredScenarioState": "armed"`)
3. Mount the directory in `docker-compose.yaml` (uncomment or add the volume line)
4. Update this skill's scenario tables above
5. Restart WireMock to load new mappings
