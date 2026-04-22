# Midnight Relayer — Local Testing Guide

Runnable instructions for starting the relayer against Midnight preview testnet and exercising the unshielded transaction flow end-to-end. Companion to `midnight-architecture.md`.

---

## 1. Prerequisites

| Component | Why | How |
|---|---|---|
| Redis (6.x+) | Job queue + repo storage | `brew install redis && brew services start redis` |
| Midnight proof server | ZK proof generation (localhost:6300) | See [Midnight docs](https://docs.midnight.network/) — typically `docker run -p 6300:6300 midnightnetwork/proof-server` |
| Rust toolchain | Build | `rustup` — stable |
| Preview-testnet DUST + NIGHT | Gas + transferable value | Fund the dev wallet via faucet |

The shipped dev keystore lives at `tests/utils/test_keys/unit-test-local-signer.json` (passphrase `test`) and is already funded on preview testnet. Use it for smoke testing before creating your own signer.

---

## 2. Configuration

### Environment (`.env` at repo root)

```dotenv
# Required
API_KEY=test_key
REDIS_URL=redis://localhost:6379
KEYSTORE_PASSPHRASE=test

# Point at the midnight-specific config bundle
CONFIG_DIR=./config
CONFIG_FILE_NAME=midnight-test-config.json

# Logging (logs are file-only when LOG_MODE=file)
LOG_LEVEL=debug
LOG_MODE=file            # or "stdout"
LOG_DATA_DIR=./logs
```

### Shipped config files

- `config/midnight-test-config.json` — relayer + signer entries for preview testnet
- `config/networks/midnight.json` — network definition (RPC, indexer, prover URLs)

Inspect these to see what a minimal midnight relayer entry looks like. A new entry at minimum needs: `id`, `network` (`preview`), `network_type` (`midnight`), `signer_id`, `paused: false`, and a `policies.min_balance`.

---

## 3. Start the server

```bash
# Fresh start — ensures no stale state from prior runs
redis-cli flushall

# Build + launch (first build is ~2 min, incremental is seconds)
cargo run --features midnight

# In another terminal, follow logs
tail -f ./logs/relayer.log
```

**What you should see**:
1. Midnight sync bootstrap enumerates configured midnight relayers and starts one `SharedDustSyncTask` per network.
2. The task subscribes to `dustLedgerEvents` (wss://indexer.preview.midnight.network/...) and starts replaying ~34K events.
3. Per-relayer watchers flip relayer state from `disabled (reason: Syncing)` → `enabled` once the task reports Ready (~30–60s).
4. Actix web starts on `:8080` — logs: `"Actix-web service started on port 8080"`.

HTTP endpoints are responsive as soon as step 4 lands, but calls that read wallet balance will report 0 until the relayer enters the enabled state.

---

## 4. Authentication

All endpoints except `/health` and `/ready` require bearer auth:

```
Authorization: Bearer <API_KEY>
```

Where `<API_KEY>` matches the env var set above. Missing/wrong headers return `401`.

Set it once in your shell to shorten the examples below:

```bash
export API_KEY=test_key
export RELAYER_ID=midnight-testnet   # matches config/midnight-test-config.json
```

---

## 5. Health checks (no auth)

```bash
curl -s http://localhost:8080/health     # liveness
curl -s http://localhost:8080/ready      # readiness (includes redis + config)
```

---

## 6. Inspect relayer state

```bash
# Relayer info + configured policies
curl -s -H "Authorization: Bearer $API_KEY" \
  http://localhost:8080/api/v1/relayers/$RELAYER_ID | jq

# Real-time status: address, DUST + NIGHT balances, last-seen block
curl -s -H "Authorization: Bearer $API_KEY" \
  http://localhost:8080/api/v1/relayers/$RELAYER_ID/status | jq

# Just the balance (unshielded NIGHT)
curl -s -H "Authorization: Bearer $API_KEY" \
  http://localhost:8080/api/v1/relayers/$RELAYER_ID/balance | jq
```

**Expected readiness progression** (watch `/status`):
1. Shortly after boot: response has `system_disabled: true`, `disabled_reason: {type: "Syncing", details: "..."}`.
2. After initial DUST sync completes: `system_disabled: false`, `dust_balance` > 0, `night_balance` > 0.

If DUST sync fails, `disabled_reason` becomes `{type: "SyncFailed", details: "..."}` — check the log for the underlying indexer/WS error.

---

## 7. Submit an unshielded NIGHT transfer

### Request shape

`POST /api/v1/relayers/{relayer_id}/transactions` with the body matching `MidnightTransactionRequest`:

```jsonc
{
  "guaranteed_offer": {
    "inputs":  [ { "origin": "<sender>",    "token_type": "NIGHT", "value": "<amount>" } ],
    "outputs": [ { "destination": "<recv>", "token_type": "NIGHT", "value": "<amount>" } ]
  },
  "intents": [],
  "fallible_offers": [],
  "ttl": "300"
}
```

**Current builder constraints** (enforced later in the pipeline, not at request-parse time):
- `token_type` must be `"NIGHT"` — other tokens are rejected during build.
- `origin` is accepted but coerced to the relayer's own wallet seed — the relayer can only spend UTXOs it owns.
- `destination` must be either the literal string `"self"` (loops back to the relayer) or a valid bech32m unshielded Midnight address.
- `value` is a decimal string in base units. NIGHT has 6 decimals, so `"1000000"` = 1 NIGHT.
- Shielded addresses / shielded outputs are **not supported yet** — see `midnight-architecture.md` for the roadmap.

### Send one

```bash
# Self-transfer 1 NIGHT — the easiest smoke test since it needs no external address
curl -s -X POST \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
        "guaranteed_offer": {
          "inputs":  [ { "origin": "self", "token_type": "NIGHT", "value": "1000000" } ],
          "outputs": [ { "destination": "self", "token_type": "NIGHT", "value": "1000000" } ]
        },
        "ttl": "300"
      }' \
  http://localhost:8080/api/v1/relayers/$RELAYER_ID/transactions | jq
```

The response includes the relayer-assigned `id`. Save it:

```bash
export TX_ID=<copy-from-response>
```

### Track the transaction

```bash
# Polls the tx — watch status transition: Pending → Sent → Confirmed (or Failed)
curl -s -H "Authorization: Bearer $API_KEY" \
  http://localhost:8080/api/v1/relayers/$RELAYER_ID/transactions/$TX_ID | jq '.status, .hash'

# Or list recent transactions
curl -s -H "Authorization: Bearer $API_KEY" \
  "http://localhost:8080/api/v1/relayers/$RELAYER_ID/transactions?limit=10" | jq
```

Expected timing on preview: `Pending → Sent` within ~2s, `Sent → Confirmed` within ~6-12s (one block). The response includes `hash` (pallet-level) and `extrinsic_hash` (node RPC) — both should be non-null after submission.

---

## 8. CRUD: add a signer + relayer at runtime

Use this flow to exercise the hybrid isolation path (seed-specific shared slot) without restarting the process.

### 8.1 Generate a fresh seed

```bash
cargo run --features midnight --example midnight_dust_prep
```

The helper prints a 32-byte seed (hex), a viewing key, and a bech32m unshielded address. Fund the printed address on the faucet, then register it:

```bash
export NEW_SIGNER_ID=alice-signer
export NEW_RELAYER_ID=alice-relayer
export NEW_SEED_HEX=<hex-from-helper>
```

### 8.2 Create the signer

```bash
curl -s -X POST \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d "{
        \"id\": \"$NEW_SIGNER_ID\",
        \"type\": \"local\",
        \"config\": { \"raw_key\": \"$NEW_SEED_HEX\" }
      }" \
  http://localhost:8080/api/v1/signers | jq
```

### 8.3 Create the relayer

```bash
curl -s -X POST \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d "{
        \"id\": \"$NEW_RELAYER_ID\",
        \"network\": \"preview\",
        \"network_type\": \"midnight\",
        \"signer_id\": \"$NEW_SIGNER_ID\",
        \"paused\": false,
        \"policies\": { \"min_balance\": 0 }
      }" \
  http://localhost:8080/api/v1/relayers | jq
```

### 8.4 Watch it sync

The new relayer starts with `system_disabled: true, disabled_reason: {type: "Syncing", ...}`. Background sync runs via an **isolated** slot (not the shared boot-time slot) because the seed wasn't known at startup:

```bash
watch -n 2 "curl -s -H 'Authorization: Bearer $API_KEY' \
  http://localhost:8080/api/v1/relayers/$NEW_RELAYER_ID/status | jq '.system_disabled, .disabled_reason'"
```

Once `system_disabled: false`, submit a tx the same way as in §7 (targeting `$NEW_RELAYER_ID`).

---

## 9. Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `401 Unauthorized` | Missing / wrong `Authorization` header | Re-export `API_KEY`, ensure literal `Bearer ` prefix |
| `/status` returns 500 | Corrupted repo state after a schema change | `redis-cli flushall` + restart |
| Stuck in `Syncing` > 5 min | Indexer WS flapping, or `dustLedgerEvents` cursor stuck | Check log for `"Caught up with quiet period"` — absence means the stream never stabilized |
| Transaction stuck in `Sent` | Chain accepted but confirmer isn't polling | Verify `hash` is non-null; check `transactions/:id` raw log for `InvalidDustSpendProof` → DUST sync regression |
| Proof server timeout | `prover_url` unreachable | `curl http://localhost:6300` — start the proof-server container |
| `400` on submit with cryptic field error | `deny_unknown_fields` rejecting a typo | Re-check field names against §7; everything is `snake_case` |

---

## 10. Smoke tests

```bash
# Live preview-testnet; takes ~60s; requires the unit-test keystore to be funded
cargo test --test midnight_shared_sync_smoke \
  --features midnight -- --ignored --nocapture
```

The test asserts:
- Shared DUST sync task subscribes, reaches `Ready`, exposes ≥1 UTXO.
- `dust_balance` > 0.

---

## Quick reference — endpoint cheat sheet

```
GET    /health
GET    /ready
GET    /api/v1/relayers
POST   /api/v1/relayers                               # create
GET    /api/v1/relayers/{id}                          # info
PATCH  /api/v1/relayers/{id}                          # update
DELETE /api/v1/relayers/{id}
GET    /api/v1/relayers/{id}/status                   # real-time balances + disabled_reason
GET    /api/v1/relayers/{id}/balance
GET    /api/v1/relayers/{id}/transactions
POST   /api/v1/relayers/{id}/transactions             # submit
GET    /api/v1/relayers/{id}/transactions/{tx_id}
GET    /api/v1/signers
POST   /api/v1/signers
GET    /api/v1/signers/{id}
PATCH  /api/v1/signers/{id}
DELETE /api/v1/signers/{id}
```

All authenticated endpoints share the `Authorization: Bearer $API_KEY` header.
