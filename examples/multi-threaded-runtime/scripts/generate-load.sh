#!/usr/bin/env bash
#
# Drives transaction load through the multi-threaded relayer so the apalis queue
# workers are exercised and their thread distribution can be observed (spec T004/D8).
#
# It (1) looks up the relayer's address, (2) funds it on anvil, then (3) fires
# self-transfers at a target rate for a fixed duration.
#
# Requirements on the host: bash, curl, jq.
#
# Usage:
#   API_KEY=... ./scripts/generate-load.sh
#   RATE=30 DURATION=120 API_KEY=... ./scripts/generate-load.sh
set -euo pipefail

BASE_URL="${BASE_URL:-http://localhost:8080}"
ANVIL_URL="${ANVIL_URL:-http://localhost:8545}"
RELAYER_ID="${RELAYER_ID:-mt-relayer}"
API_KEY="${API_KEY:?set API_KEY (must match the relayer API_KEY)}"
RATE="${RATE:-30}"         # transactions per second (target)
DURATION="${DURATION:-60}" # seconds

auth=(-H "Authorization: Bearer ${API_KEY}" -H "Content-Type: application/json")

echo "Looking up relayer ${RELAYER_ID} address..."
addr=""
for _ in $(seq 1 30); do
  # NOTE: unquoted assignment RHS (not word-split) keeps the embedded jq single
  # quotes portable across bash 3.2 (macOS) and bash 5.x.
  resp=$(curl -fsS "${auth[@]}" "${BASE_URL}/api/v1/relayers/${RELAYER_ID}" 2>/dev/null || true)
  addr=$(printf '%s' "${resp}" | jq -r '.data.address // empty')
  [ -n "${addr}" ] && break
  echo "  relayer not ready yet, retrying..."
  sleep 2
done
[ -n "${addr}" ] || { echo "ERROR: could not resolve relayer address"; exit 1; }
echo "Relayer address: ${addr}"

echo "Funding ${addr} on anvil (100 ETH)..."
curl -fsS -H "Content-Type: application/json" "${ANVIL_URL}" \
  --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"anvil_setBalance\",\"params\":[\"${addr}\",\"0x56BC75E2D63100000\"]}" \
  >/dev/null
echo "Funded."

body="{\"value\":1,\"data\":\"0x\",\"to\":\"${addr}\",\"speed\":\"average\",\"gas_limit\":21000}"
interval=$(awk "BEGIN{printf \"%.4f\", 1/${RATE}}")
total=$(( RATE * DURATION ))

echo "Sending ~${RATE} tx/s for ${DURATION}s (${total} txs) to ${RELAYER_ID}..."
sent=0
for _ in $(seq 1 "${total}"); do
  curl -fsS -X POST "${auth[@]}" \
    "${BASE_URL}/api/v1/relayers/${RELAYER_ID}/transactions" \
    --data "${body}" >/dev/null 2>&1 &
  sent=$(( sent + 1 ))
  if [ $(( sent % RATE )) -eq 0 ]; then
    echo "  queued ${sent}/${total}"
  fi
  sleep "${interval}"
done
wait
echo "Done — queued ${sent} transactions."
