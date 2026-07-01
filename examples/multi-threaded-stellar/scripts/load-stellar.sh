#!/usr/bin/env bash
#
# Drives concurrent self-payments through the mt-stellar relayer so the
# submission-ordering gate (submit_gate) is exercised under load.
#
# Source account == destination account (self-payment) so every transaction
# from this burst contends the same sequence number. Without the gate and
# monotonic sync_floor, out-of-order submission across worker threads would
# produce TxBadSeq on Horizon. This is the live validation for spec 005,
# task T025, Constitution III non-negotiable gate.
#
# Steps:
#   1. Resolve the relayer account address (G...) via the relayer API.
#   2. Fund the account on Stellar testnet via friendbot.
#   3. Fire CONCURRENCY self-payments per round, ROUNDS rounds, each round
#      backgrounding all requests at once then waiting — this is the burst
#      that contends the sequence number.
#
# Requirements: bash, curl, jq.
#
# Usage:
#   API_KEY=... ./scripts/load-stellar.sh
#   CONCURRENCY=50 ROUNDS=3 API_KEY=... ./scripts/load-stellar.sh
set -euo pipefail

BASE_URL="${BASE_URL:-http://localhost:8080}"
RELAYER_ID="${RELAYER_ID:-mt-stellar}"
# NOTE: avoid apostrophes inside the :? message text — bash 3.2 (macOS) breaks on them.
API_KEY="${API_KEY:?set API_KEY, must match the relayer API_KEY}"
CONCURRENCY="${CONCURRENCY:-50}"
ROUNDS="${ROUNDS:-2}"

auth=(-H "Authorization: Bearer ${API_KEY}" -H "Content-Type: application/json")

# ── Step 1: resolve relayer address ──────────────────────────────────────────
echo "Looking up relayer ${RELAYER_ID} address..."
addr=""
for _ in $(seq 1 30); do
  # NOTE: unquoted assignment RHS (not word-split) keeps the embedded jq filter
  # portable across bash 3.2 (macOS) and bash 5.x.
  resp=$(curl -fsS "${auth[@]}" "${BASE_URL}/api/v1/relayers/${RELAYER_ID}" 2>/dev/null || true)
  addr=$(printf '%s' "${resp}" | jq -r .data.address)
  [ -n "${addr}" ] && [ "${addr}" != "null" ] && break
  addr=""
  echo "  relayer not ready yet, retrying..."
  sleep 2
done
[ -n "${addr}" ] || { echo "ERROR: could not resolve relayer address"; exit 1; }
echo "Relayer address: ${addr}"

# ── Step 2: fund via friendbot ───────────────────────────────────────────────
echo "Funding ${addr} on Stellar testnet via friendbot..."
funded=false

if curl -fsS "https://friendbot.stellar.org/?addr=${addr}" >/dev/null 2>&1; then
  echo "Friendbot funded the account."
  funded=true
else
  # Friendbot returns an error if the account already exists — that is fine.
  if curl -fsS "https://friendbot.stellar.org/?addr=${addr}" 2>&1 | grep -q "already exists\|createAccountAlreadyExist"; then
    echo "Account already exists on testnet — skipping friendbot."
    funded=true
  else
    # Try the alternate Horizon friendbot endpoint.
    if curl -fsS "https://horizon-testnet.stellar.org/friendbot?addr=${addr}" >/dev/null 2>&1; then
      echo "Funded via horizon-testnet friendbot."
      funded=true
    else
      echo "WARNING: friendbot call failed; the account may already be funded or testnet may be slow."
      funded=true
    fi
  fi
fi

# Poll Horizon until the account exists before sending any transactions.
echo "Waiting for account to appear on Horizon..."
for _ in $(seq 1 30); do
  status=$(curl -o /dev/null -s -w "%{http_code}" "https://horizon-testnet.stellar.org/accounts/${addr}")
  if [ "${status}" = "200" ]; then
    echo "Account confirmed on Horizon."
    break
  fi
  echo "  account not found yet (HTTP ${status}), retrying..."
  sleep 3
done
if [ "${status}" != "200" ]; then
  echo "ERROR: account ${addr} did not appear on Horizon after funding"
  exit 1
fi

# ── Step 3: burst self-payments ──────────────────────────────────────────────
body="{\"network\":\"testnet\",\"operations\":[{\"type\":\"payment\",\"destination\":\"${addr}\",\"asset\":{\"type\":\"native\"},\"amount\":\"1\"}]}"
total=$(( CONCURRENCY * ROUNDS ))

echo "Sending ${CONCURRENCY} concurrent self-payments x ${ROUNDS} rounds (${total} total) to ${RELAYER_ID}..."
sent=0
for round in $(seq 1 "${ROUNDS}"); do
  echo "  Round ${round}/${ROUNDS}: firing ${CONCURRENCY} requests..."
  for _ in $(seq 1 "${CONCURRENCY}"); do
    curl -fsS -X POST "${auth[@]}" \
      "${BASE_URL}/api/v1/relayers/${RELAYER_ID}/transactions" \
      --data "${body}" >/dev/null 2>&1 &
    sent=$(( sent + 1 ))
  done
  wait
  echo "  Round ${round} complete (${sent}/${total} queued so far)."
done
echo "Done — queued ${sent} transactions."
