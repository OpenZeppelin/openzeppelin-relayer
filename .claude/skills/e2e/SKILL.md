---
name: e2e
description: Set up and run e2e/integration tests. Configures environment, keystores, Docker services, and test execution for local (Anvil), standalone, or testnet modes. Supports chaos testing via WireMock proxy.
allowed-tools: Bash, Read, Write, Edit, Grep, Glob, Agent, Skill
argument-hint: "setup [local|standalone|testnet] | run [local|standalone|testnet] | chaos [scenario] | status | logs | stop | clean | add-network <name>"
---

# E2E / Integration Test Setup

You help configure and run the integration test environment for the OpenZeppelin Relayer.

## Project paths

- Test source: `tests/integration/tests/` (test files), `tests/integration/common/` (helpers)
- Config templates: `tests/integration/config/config.example.json`, `tests/integration/config/registry.example.json`
- Standalone test config: `config/config.test.json` (tracked in git, used via `CONFIG_FILE_NAME=config.test.json`)
- Local Docker config: `tests/integration/config/local/` (config.json, registry.json, keys/)
- Standalone registry + keys: `tests/integration/config/local-standalone/` (registry.json, keys/)
- Testnet config: `tests/integration/config/testnet/` (config.json, registry.json, keys/)
- Docker compose: `docker-compose.integration.yml`
- Run script: `scripts/run-integration-docker.sh`
- Env example: `.env.integration.example`
- Env file: `.env.integration`
- Makefile tasks: `Makefile.toml` (integration-test-local, integration-test-standalone)
- Smart contracts: `tests/integration/contracts/`
- Plugin: `plugins/examples/e2e-echo.ts`

### WireMock chaos testing paths (managed by /wiremock-test skill)

- WireMock docker compose: `testing/wiremock/docker-compose.yaml`
- Generic fault mappings: `testing/wiremock/mappings/generic/` (timeout, 500, connection reset)
- Stellar fault mappings: `testing/wiremock/mappings/stellar/` (TRY_AGAIN_LATER, insufficient fee, tx not found)
- Helper scripts: `testing/wiremock/scripts/`
- WireMock admin: `http://localhost:9090/__admin`

## How to handle user arguments ($ARGUMENTS)

### `setup` or `setup local` — Docker-based Anvil setup (default)

1. **Check prerequisites:**
   - Verify Docker and Docker Compose are available: `docker --version && docker compose version`
   - Check if `.env.integration` exists; if not, copy from `.env.integration.example`

2. **Configure environment file (.env.integration):**
   - Read `.env.integration` and check if values are still placeholder defaults
   - For local mode, `API_KEY` can be any value (e.g., `local-integration-test-key-00000`)
   - For local mode, `KEYSTORE_PASSPHRASE` should be empty or match the Anvil keystore password (`test`)
   - Set `WEBHOOK_SIGNING_KEY` to a UUID if not already set

3. **Check keystore:**
   - Verify `tests/integration/config/local/keys/anvil-test.json` exists
   - If missing, guide the user to create it:
     ```bash
     # First start Anvil to get private keys
     anvil &
     # Use Account 0's private key (shown in Anvil output)
     cast wallet import anvil-test \
       --private-key <ANVIL_PRIVATE_KEY> \
       --keystore-dir tests/integration/config/local/keys \
       --unsafe-password "test"
     mv tests/integration/config/local/keys/anvil-test \
        tests/integration/config/local/keys/anvil-test.json
     kill %1  # Stop Anvil
     ```

4. **Verify config files:**
   - Check `tests/integration/config/local/config.json` exists (already tracked in git)
   - Check `tests/integration/config/local/registry.json` exists; if not, copy from `registry.example.json`
   - Show which networks are enabled in registry.json

5. **Report readiness:**
   - Summarize what's configured and what's missing
   - Show the command to run: `./scripts/run-integration-docker.sh`

### `setup standalone` — Local development setup

Standalone mode uses `config/config.test.json` — a dedicated test config checked into git. No need to modify `config/config.json`.

1. **Check prerequisites:**
   - Verify `cargo` and `cast` (foundry) are available
   - Check if Redis is running: `redis-cli ping`
   - Check if Anvil is available: `which anvil`

2. **Check keystore:**
   - Verify `tests/integration/config/local-standalone/keys/anvil-test.json` exists
   - If missing, guide the user to create it:
     ```bash
     anvil &  # Start Anvil to see pre-funded accounts
     # Use Account 0's private key from Anvil output
     cast wallet import anvil-test \
       --private-key <ANVIL_PRIVATE_KEY> \
       --keystore-dir tests/integration/config/local-standalone/keys \
       --unsafe-password "test"
     mv tests/integration/config/local-standalone/keys/anvil-test \
        tests/integration/config/local-standalone/keys/anvil-test.json
     kill %1
     ```

3. **Check test config (`config/config.test.json`):**
   - Verify it exists (tracked in git, should be present)
   - It contains: anvil-relayer (network `localhost-anvil`), anvil-signer, and e2e-echo plugin
   - The relayer reads it via `CONFIG_FILE_NAME=config.test.json` env var

4. **Check network config:**
   - Verify `config/networks/local-anvil.json` exists and has `rpc_urls: ["http://localhost:8545"]`
   - This file is tracked in git and should already be present

5. **Check standalone registry:**
   - Verify `tests/integration/config/local-standalone/registry.json` exists
   - Show which networks are enabled

6. **Report readiness:**
   - Summarize what's ready and what's missing
   - If everything is OK, suggest: `Use /e2e run standalone to start services and run tests`

### `setup testnet` — Testnet configuration

1. **Check .env.integration:**
   - Ensure `API_KEY` is set to a real value (not placeholder)
   - Ensure `KEYSTORE_PASSPHRASE` is set (unless `USE_KMS=true`)

2. **Check testnet config files:**
   - If `tests/integration/config/testnet/config.json` missing, copy from example
   - If `tests/integration/config/testnet/registry.json` missing, copy from example
   - Show which networks are enabled in registry

3. **Check keystore or KMS:**
   - If using local signer: verify `tests/integration/config/testnet/keys/local-signer.json` exists
   - If using KMS: verify `USE_KMS=true` is set in `.env.integration`

4. **Report readiness and show command:**
   ```bash
   MODE=testnet ./scripts/run-integration-docker.sh
   ```

### `run` or `run local` — Run Docker-based tests

1. First perform setup checks (same as `setup local` but non-interactive — just verify)
2. Execute: `./scripts/run-integration-docker.sh`
3. Stream output and report results

### `run standalone` — Run standalone tests (full orchestration)

This command should get tests running with minimal manual steps. It starts services if needed.

1. **Check prerequisites** (same as `setup standalone` — verify cargo, cast, redis, keystore, config files)

2. **Start Anvil if not running:**
   ```bash
   cast client --rpc-url http://localhost:8545 2>/dev/null
   ```
   If unreachable, start it in the background:
   ```bash
   anvil &
   ```
   Wait for it to be healthy (retry `cast client` a few times).

3. **Start relayer if not running:**
   ```bash
   curl -sf http://localhost:8080/api/v1/health
   ```
   If unreachable, start it in the background with the test config:
   ```bash
   CONFIG_FILE_NAME=config.test.json cargo run &
   ```
   Wait for health check to pass (retry up to 30s).
   - The relayer MUST be started with `CONFIG_FILE_NAME=config.test.json` to use the test config
   - If the relayer IS running but was started without the test config, warn the user and ask them to restart it

4. **Run tests:**
   ```bash
   export $(grep -v '^#' .env | grep -v '^$' | xargs) && \
   TEST_REGISTRY_PATH=tests/integration/config/local-standalone/registry.json \
   cargo test --features integration-tests --test integration
   ```
   - The `API_KEY` env var must match the relayer's configured API key
   - Do NOT use `cargo make integration-test-standalone` — it may try to start a Docker Anvil container

5. **Report results** — show pass/fail summary

### `run testnet` — Run testnet tests

1. Perform testnet setup checks
2. Execute: `MODE=testnet ./scripts/run-integration-docker.sh`

### `status` — Check environment status

1. Check Docker containers: `docker compose -f docker-compose.integration.yml ps`
2. Check if relayer is healthy (try both Docker and local):
   - `curl -sf http://localhost:8080/api/v1/health`
3. Check if Anvil is running: `cast client --rpc-url http://localhost:8545 2>/dev/null`
4. Check if Redis is running: `redis-cli ping 2>/dev/null`
5. Show which config files exist and which networks are enabled
6. Report overall status

### `logs` — View test/service logs

1. Run: `./scripts/run-integration-docker.sh logs`
2. Or for specific service: `docker compose -f docker-compose.integration.yml logs <service>`
   - Services: `integration-redis`, `integration-anvil`, `integration-relayer`, `integration-tests`

### `stop` or `teardown` — Stop services

1. Run: `./scripts/run-integration-docker.sh down`
2. Optionally stop local Anvil: `./scripts/anvil-local.sh stop`
3. Confirm stopped

### `clean` — Full cleanup

1. Run: `./scripts/run-integration-docker.sh clean`
2. This removes all Docker resources (containers, volumes, networks)

### `chaos` or `chaos setup [evm|stellar]` — Start e2e with WireMock chaos proxy

This command orchestrates both the e2e environment and WireMock fault injection. It delegates WireMock operations to the `/wiremock-test` skill.

1. **Ensure e2e services are running:**
   - Check if the integration Docker stack is up (relayer, redis, anvil)
   - If not, prompt user to run `setup local` or `setup testnet` first

2. **Start WireMock proxy via `/wiremock-test` skill:**
   - Delegate to `/wiremock-test setup <network>` with the appropriate target:
     - For EVM/Anvil: target is `http://host.docker.internal:8545` (standalone) or `http://anvil:8545` (Docker)
     - For Stellar testnet: target is default (`https://soroban-testnet.stellar.org`)
   - Wait for WireMock health check

3. **Rewire the relayer's RPC URL:**
   - Identify which network config to modify based on the network type
   - For standalone mode: update the network JSON in `config/networks/` to point `rpc_urls` to `http://localhost:9090`
   - For Docker mode: update to `http://host.docker.internal:9090`
   - **Save the original RPC URL** so it can be restored on teardown
   - Restart the relayer if it's running in Docker: `docker compose -f docker-compose.integration.yml restart relayer`

4. **Report status:**
   - Show WireMock is proxying, which RPC URL was replaced
   - List available scenarios (delegate to `/wiremock-test status`)
   - Show how to arm scenarios: `/wiremock-test <scenario>` (e.g., `/wiremock-test timeout`, `/wiremock-test 500`)

### `chaos <scenario>` — Arm a specific fault scenario and run tests

1. **Delegate scenario arming to `/wiremock-test`:**
   - Pass the scenario name/shorthand directly: `/wiremock-test <scenario>`
   - Available scenarios:
     - Generic: `timeout`, `500`, `reset` (work with any network)
     - Stellar: `try-again-later` / `tal`, `insufficient-fee` / `fee`, `not-found`

2. **Run e2e tests:**
   - Execute the appropriate test run command (same as `run local` or `run standalone`)
   - The armed scenario will fire once during the test run, then revert to proxying

3. **Report results:**
   - Show test pass/fail
   - Check WireMock request log: `/wiremock-test status` to verify the fault was injected
   - Check relayer metrics: `/wiremock-test check-metrics` for retry/failure counters

### `chaos all` — Arm all fault scenarios and run tests

1. Delegate to `/wiremock-test chaos` to arm all scenarios simultaneously
2. Run e2e tests — scenarios fire in priority order (most specific first)
3. Report results and metrics

### `chaos stop` — Tear down WireMock and restore RPC URLs

1. **Restore original RPC URLs:**
   - Revert the network config changes made during `chaos setup`
   - Restart the relayer if running in Docker
2. **Stop WireMock:** delegate to `/wiremock-test teardown`
3. Confirm everything is restored

### `add-network <name>` — Add a new network for testing

1. Determine network type (ask user if not obvious: evm, solana, or stellar)
2. Add entry to the appropriate `registry.json` with `"enabled": true`
3. Add corresponding relayer entry to `config.json`
4. Remind user to:
   - Deploy test contracts on the new network
   - Fund the signer wallet
   - Update contract addresses in registry.json

## Environment variables reference

| Variable | Mode | Description |
|----------|------|-------------|
| `API_KEY` | All | Relayer API auth key (any value for local) |
| `KEYSTORE_PASSPHRASE` | Testnet | Keystore encryption password |
| `WEBHOOK_SIGNING_KEY` | All | Webhook signing key (UUID) |
| `MODE` | Docker | `local` (default) or `testnet` |
| `RUST_LOG` | All | Log level (default: `info`) |
| `STRICT_E2E` | All | Strict mode (default: `true`). Set to `false` to skip tests when relayer is unreachable instead of failing |
| `USE_KMS` | Testnet | Use AWS KMS instead of local keystore |
| `REPOSITORY_STORAGE_TYPE` | All | `in_memory` or `redis` (default: `in_memory`) |
| `RESET_STORAGE_ON_START` | All | `true` to clear Redis state on startup (required for integration tests with redis storage) |
| `STORAGE_ENCRYPTION_KEY` | All | Base64-encoded encryption key for Redis storage (required when using redis storage) |
| `CONFIG_FILE_NAME` | Standalone | Config file name in `config/` dir (default: `config.json`, use `config.test.json` for e2e) |
| `TEST_REGISTRY_PATH` | Standalone | Path to registry.json |

## Important reminders

- Strict mode is ON by default — tests will fail (not silently skip) when the relayer is unreachable or env vars are missing. Set `STRICT_E2E=false` only if you intentionally want skip-on-failure behavior.
- Tests discover relayers via the API (`GET /api/v1/relayers`), not from config files
- Config.json starts the relayer; registry.json controls which networks the tests target
- **Standalone registry must use `localhost-anvil`** (matching the relayer config), NOT `localhost` or `localhost-anvil-docker` (those are Docker mode names)
- Local mode uses Anvil (chain ID 31337) — no real funds needed
- Docker mode network names differ from standalone (e.g., `localhost-anvil-docker` vs `localhost-anvil`)
- The `integration-tests` feature flag must be enabled for test compilation
- Mode-specific registry.json and config.json files are gitignored for local customization
- Coverage reports are written to `./coverage/integration-lcov.info`
- **When using Redis storage**, always set `RESET_STORAGE_ON_START=true` for integration tests — stale Redis state from previous runs can override config.json entries and cause plugin 404s, encryption errors, and config inconsistencies
- The relayer must be restarted after changing `.env` or `config/config.json` for changes to take effect

### Chaos testing reminders
- WireMock chaos commands delegate to the `/wiremock-test` skill — do NOT duplicate WireMock logic
- Always restore original RPC URLs after chaos testing (`chaos stop`)
- WireMock scenarios are one-shot: they fire once then revert to proxying real traffic
- For Docker mode, use `http://host.docker.internal:9090` as WireMock URL; for standalone, use `http://localhost:9090`
- After rewiring RPC URLs, the relayer must be restarted to pick up the change
- Check relayer metrics after chaos tests to verify retry/error handling: `/wiremock-test check-metrics`
