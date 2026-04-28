---
name: smoke-testing-midnight
description: Use when smoke testing Midnight relayers, restarting the Midnight server, or verifying shielded/unshielded Midnight transactions in this repository.
---

# Smoke Testing Midnight

## Overview

Use this skill to run the OpenZeppelin Relayer locally with the Midnight config and verify transfers end to end. Prefer fresh Redis prefixes for each smoke run so old relayer state does not mask sync bugs.

## Safety Rules

- Never commit or print secrets from `.env.bak`; source it only inside shell commands.
- Check for an existing process on port `8080` before starting a server.
- Use a fresh `REDIS_KEY_PREFIX` for each run unless the user explicitly wants to reuse state.
- For shielded transfers, use `fallible_shielded_offers`, not top-level `guaranteed_offer`.
- If a transaction fails, capture `id`, `hash`, `extrinsic_hash`, `status_reason`, and the final relayer balances.

## Start Or Restart Server

1. Stop any server already listening on `8080`:

```bash
pid=$(lsof -tiTCP:8080 -sTCP:LISTEN || true)
if [ -n "$pid" ]; then kill "$pid"; fi
sleep 2
lsof -nP -iTCP:8080 -sTCP:LISTEN || true
```

2. Start the Midnight relayer in the background with a unique prefix:

```bash
set -a; source .env.bak; set +a
KEYSTORE_PASSPHRASE=test \
CONFIG_FILE_NAME=midnight-test-config.json \
REDIS_KEY_PREFIX=oz-midnight-smoke-$(date +%s) \
RESET_STORAGE_ON_START=true \
RUST_LOG=info,openzeppelin_relayer=debug \
cargo run --features midnight --target-dir target
```

3. Wait until the relayer responds and `system_disabled` is `false`:

```bash
set -a; source .env.bak; set +a
curl -sS -H "Authorization: Bearer $API_KEY" \
  http://localhost:8080/api/v1/relayers/midnight-testnet/status
```

Expected healthy fields:

- `system_disabled: false`
- `paused: false`
- `dust_balance` is positive
- `pending_transactions_count: 0` before starting a clean smoke test

## Check Balances

Use both endpoints when debugging:

```bash
set -a; source .env.bak; set +a
curl -sS -H "Authorization: Bearer $API_KEY" \
  http://localhost:8080/api/v1/relayers/midnight-testnet/status
curl -sS -H "Authorization: Bearer $API_KEY" \
  http://localhost:8080/api/v1/relayers/midnight-testnet/balance
```

For shielded token tests, confirm `status.shielded_balances` contains the token before sending.

## Send Unshielded Self Transfer

Use this to verify unshielded NIGHT sync, DUST spendability, pending-spend rollback, and change handling:

```bash
set -a; source .env.bak; set +a
curl -sS -X POST http://localhost:8080/api/v1/relayers/midnight-testnet/transactions \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "guaranteed_offer": {
      "kind": "unshielded",
      "inputs": [
        { "origin": "self", "token_type": "NIGHT", "value": "1" }
      ],
      "outputs": [
        { "destination": "self", "token_type": "NIGHT", "value": "1" }
      ]
    }
  }'
```

Poll the returned transaction `id` until final:

```bash
set -a; source .env.bak; set +a
TX_ID="replace-with-id"
for i in 1 2 3 4 5 6 7 8 9 10 11 12; do
  curl -sS -H "Authorization: Bearer $API_KEY" \
    "http://localhost:8080/api/v1/relayers/midnight-testnet/transactions/$TX_ID"
  echo
  sleep 10
done
```

Expected result: `status: confirmed`, `status_reason: Transaction succeeded entirely`, balance unchanged for a self-transfer, and `pending_transactions_count: 0`.

## Send Shielded Token Transfer

Set the destination and token type explicitly. Use `fallible_shielded_offers` with a non-reserved segment such as `2`.

```bash
set -a; source .env.bak; set +a
TOKEN="41a056381e1288447e01a8177b5138140de69f759ac83f4bc402c4d8406152e1"
DEST="mn_shield-addr_preview1s7stjkjvn8jkxlq7mcs2qeqgvh674smygnazzu4ew65m55dw5u5rwgu347z8agudxww25whelhw8ygc8dkgxu0ph3m4vxuum93294wgyn4mp2"
curl -sS -X POST http://localhost:8080/api/v1/relayers/midnight-testnet/transactions \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d "{
    \"fallible_shielded_offers\": [
      {
        \"segment_id\": 2,
        \"offer\": {
          \"kind\": \"shielded\",
          \"inputs\": [
            { \"origin\": \"self\", \"token_type\": \"$TOKEN\", \"value\": \"1\" }
          ],
          \"outputs\": [
            { \"destination\": \"$DEST\", \"token_type\": \"$TOKEN\", \"value\": \"1\" }
          ]
        }
      }
    ]
  }"
```

Poll the returned transaction `id` using the same polling loop as above.

Expected result:

- Transaction reaches `confirmed`.
- `status_reason` is `Transaction succeeded entirely`.
- `extrinsic_hash` and `block_hash` are populated.
- The relayer’s shielded balance decreases by the transfer amount, not by the full selected coin value. If it drops to zero unexpectedly, suspect missing shielded change.

## Common Failure Interpretation

- `Relayer disabled`: wait for `system_disabled: false`; do not submit during initial sync.
- `Insufficient DUST`: reported DUST balance may not be spendable in `LedgerContext`; check DUST sync and refresh logic.
- `Custom error: 170`: usually invalid DUST or shielded proof context. Check `ctime`, DUST refresh, and whether proving mutated the canonical wallet before submit.
- `PARTIAL_SUCCESS`: inspect segment results from the indexer; do not treat it as confirmed.
- Shielded balance disappears after failed submit: proving likely mutated canonical shielded pending spends.
- Shielded balance disappears after confirmed send of a smaller amount: missing shielded change output.

## Final Report Template

Report the smoke result with:

- Server prefix used.
- Starting balances.
- Transaction id, hash, extrinsic hash, block hash.
- Final status and status reason.
- Ending balances and pending transaction count.
- Any failed attempt payload shape if a retry was needed.
