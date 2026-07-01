#!/usr/bin/env bash
#
# A/B benchmark: drains a fixed burst of transactions through the relayer and
# measures pipeline throughput, submission success rate, peak CPU, and the number
# of worker threads that carried the work — for a given runtime sizing.
#
# Recreates the relayer with the requested TOKIO_WORKER_THREADS / ACTIX_WORKERS,
# flushes Redis (clean queue) first, bursts M transactions, then watches the
# relayer account's *pending* nonce climb on anvil (each successful submission
# bumps it) until it plateaus.
#
# Requirements: bash, curl, jq.
# Usage:
#   API_KEY=... TWT=1 AW=1 M=500 ./scripts/benchmark.sh
#   API_KEY=... TWT=4 AW=2 M=500 ./scripts/benchmark.sh
set -euo pipefail

BASE_URL="${BASE_URL:-http://localhost:8080}"
ANVIL_URL="${ANVIL_URL:-http://localhost:8545}"
RELAYER_ID="${RELAYER_ID:-mt-relayer}"
API_KEY="${API_KEY:?set API_KEY}"
TWT="${TWT:-4}"   # TOKIO_WORKER_THREADS
AW="${AW:-2}"     # ACTIX_WORKERS
M="${M:-500}"     # number of transactions in the burst
CONC="${CONC:-40}" # concurrent enqueue requests
COMPOSE_DIR="$(cd "$(dirname "$0")/.." && pwd)"

auth=(-H "Authorization: Bearer ${API_KEY}" -H "Content-Type: application/json")

rpc() { # method, params-json -> .result
  curl -fsS -H "Content-Type: application/json" "${ANVIL_URL}" \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":$2}" | jq -r '.result'
}
hex2dec() { printf '%d' "$1"; }
pending_nonce() { hex2dec "$(rpc eth_getTransactionCount "[\"${ADDR}\",\"pending\"]")"; }

echo "############################################################"
echo "# BENCHMARK  TOKIO_WORKER_THREADS=${TWT}  ACTIX_WORKERS=${AW}  burst=${M}"
echo "############################################################"

cd "${COMPOSE_DIR}"
echo "Resetting queue + recreating relayer with the target sizing..."
docker compose exec -T redis redis-cli flushall >/dev/null 2>&1 || true
TOKIO_WORKER_THREADS="${TWT}" ACTIX_WORKERS="${AW}" docker compose up -d --force-recreate relayer >/dev/null 2>&1

# Wait for the API to come up and resolve the relayer address.
ADDR=""
for _ in $(seq 1 40); do
  ADDR=$(curl -fsS "${auth[@]}" "${BASE_URL}/api/v1/relayers/${RELAYER_ID}" 2>/dev/null | jq -r '.data.address // empty' || true)
  [ -n "${ADDR}" ] && break
  sleep 1
done
[ -n "${ADDR}" ] || { echo "ERROR: relayer not ready"; exit 1; }
echo "Relayer address: ${ADDR}"

# Confirm the resolved budget from the startup log.
docker compose logs relayer 2>/dev/null | grep -o '"message":"resolved runtime worker budget"[^}]*' | tail -1 || true

echo "Funding on anvil..."
rpc anvil_setBalance "[\"${ADDR}\",\"0x56BC75E2D63100000\"]" >/dev/null

start_nonce=$(pending_nonce)
body="{\"value\":1,\"data\":\"0x\",\"to\":\"${ADDR}\",\"speed\":\"average\",\"gas_limit\":21000}"

echo "Bursting ${M} tx enqueue requests (concurrency ${CONC})..."
t0=$(date +%s.%N)
inflight=0
for _ in $(seq 1 "${M}"); do
  curl -fsS -X POST "${auth[@]}" "${BASE_URL}/api/v1/relayers/${RELAYER_ID}/transactions" --data "${body}" >/dev/null 2>&1 &
  inflight=$(( inflight + 1 ))
  if [ "${inflight}" -ge "${CONC}" ]; then wait -n 2>/dev/null || wait; inflight=$(( inflight - 1 )); fi
done
wait
t_enq=$(date +%s.%N)
echo "  enqueue wall time: $(awk "BEGIN{printf \"%.1f\", ${t_enq}-${t0}}")s"

# Drain: watch the pending nonce climb until it plateaus (no progress for ~6s).
echo "Draining... (watching pending nonce + CPU)"
peak_cpu=0
last=${start_nonce}; stable=0
for _ in $(seq 1 120); do
  cur=$(pending_nonce)
  cpu=$(docker stats --no-stream --format '{{.CPUPerc}}' multi-threaded-runtime-relayer-1 2>/dev/null | tr -d '%' || echo 0)
  awk "BEGIN{exit !(${cpu:-0}>${peak_cpu})}" && peak_cpu=${cpu}
  submitted=$(( cur - start_nonce ))
  printf "  submitted=%d/%d  pending_nonce=%d  cpu=%s%%\n" "${submitted}" "${M}" "${cur}" "${cpu}"
  if [ "${cur}" -eq "${last}" ]; then stable=$(( stable + 1 )); else stable=0; fi
  last=${cur}
  # done when we've submitted all, or progress has stalled for 6 polls after some work
  { [ "${submitted}" -ge "${M}" ] && break; } || true
  { [ "${stable}" -ge 6 ] && [ "${submitted}" -gt 0 ] && break; } || true
  sleep 1
done
t1=$(date +%s.%N)

final=$(pending_nonce)
submitted=$(( final - start_nonce ))
drain_secs=$(awk "BEGIN{printf \"%.1f\", ${t1}-${t0}}")
throughput=$(awk "BEGIN{printf \"%.1f\", ${submitted}/(${t1}-${t0})}")
rate=$(awk "BEGIN{printf \"%.1f\", 100*${submitted}/${M}}")

# Thread spread over the run window.
threads=$(docker compose logs relayer --since "${drain_secs%.*}s" 2>/dev/null \
  | grep -iE "prepare|submit|status|transaction|job" \
  | grep -oE "ThreadId\([0-9]+\)" | sort -u | tr '\n' ' ')
nthreads=$(echo "${threads}" | wc -w | tr -d ' ')

echo "------------------------------------------------------------"
echo "RESULT  TWT=${TWT} AW=${AW}"
echo "  burst sent ............ ${M}"
echo "  submitted (nonce Δ) ... ${submitted}"
echo "  success rate .......... ${rate}%"
echo "  total drain time ...... ${drain_secs}s"
echo "  submission throughput . ${throughput} tx/s"
echo "  peak relayer CPU ...... ${peak_cpu}%"
echo "  distinct worker threads ${nthreads}  (${threads})"
echo "------------------------------------------------------------"
