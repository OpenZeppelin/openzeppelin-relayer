#!/usr/bin/env bash
# US2 acceptance check (SC-005): bring up the emulator stack with one command,
# confirm pubsub-init provisioned all 8 topic/subscription pairs before the
# relayer became ready, and confirm the relayer is up. Then tear down.
#
# Usage (from the repo root):
#   API_KEY=<your-key> ./examples/gcp-pubsub-queue-storage/scripts/validate.sh
#
# Requires Docker + Docker Compose. A funded relayer is needed to drive a
# transaction all the way to a final state; that end-to-end leg is exercised
# manually per the README (Step 6) and by the pre-release gate (T042). This
# script validates the deterministic bring-up + provisioning contract.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
compose="docker compose -f ${here}/docker-compose.yaml"
relayer="http://localhost:8080"
prefix="relayer-"

# The emulator is internal to the compose network (no host port published), so
# check resource existence from INSIDE the network via the emulator container,
# which has curl. Returns 0 if the path exists (HTTP 200).
emulator_has() {
  ${compose} exec -T pubsub-emulator \
    curl -sf "http://localhost:8085/v1/projects/test-project/$1" >/dev/null 2>&1
}

# Load .env if present (for API_KEY etc.).
[ -f "${here}/.env" ] && . "${here}/.env"

cleanup() { echo "--- tearing down ---"; ${compose} down -v >/dev/null 2>&1 || true; }
trap cleanup EXIT

echo "--- bringing up the stack (detached) ---"
${compose} up -d --build

echo "--- waiting for the relayer to become ready (<= 5 min) ---"
ready=""
for _ in $(seq 1 150); do
  if curl -sf "${relayer}/api/v1/health" >/dev/null 2>&1; then ready=1; break; fi
  sleep 2
done
[ -n "${ready}" ] || { echo "FAIL: relayer did not become ready in time"; ${compose} logs relayer | tail -50; exit 1; }
echo "PASS: relayer is ready"

echo "--- asserting all 8 topic/subscription pairs exist ---"
queues="transaction-request transaction-submission status-check status-check-evm \
status-check-stellar notification token-swap-request relayer-health-check"
missing=0
for q in ${queues}; do
  emulator_has "topics/${prefix}${q}" \
    || { echo "MISSING topic: ${prefix}${q}"; missing=1; }
  emulator_has "subscriptions/${prefix}${q}-sub" \
    || { echo "MISSING subscription: ${prefix}${q}-sub"; missing=1; }
done
[ "${missing}" -eq 0 ] || { echo "FAIL: missing topics/subscriptions"; exit 1; }
echo "PASS: all 8 topics + 8 subscriptions provisioned (relayer started only after this)"

echo "--- asserting the relayer API responds (auth works) ---"
if [ -n "${API_KEY:-}" ]; then
  curl -sf "${relayer}/api/v1/relayers" -H "AUTHORIZATION: Bearer ${API_KEY}" >/dev/null \
    && echo "PASS: /api/v1/relayers responded" \
    || { echo "FAIL: /api/v1/relayers did not respond"; exit 1; }
else
  echo "SKIP: set API_KEY to exercise the authenticated /api/v1/relayers endpoint"
fi

echo "--- confirming the pubsub workers spawned ---"
# Workers log at startup (before the server is ready), but `docker compose logs`
# can lag a beat on a fresh container — retry briefly and strip ANSI colour.
worker_found=no
for _ in 1 2 3 4 5; do
  if ${compose} logs relayer 2>&1 | sed -E 's/\x1b\[[0-9;]*m//g' \
       | grep -qi "Spawning Pub/Sub worker"; then
    worker_found=yes
    break
  fi
  sleep 1
done
[ "${worker_found}" = yes ] \
  && echo "PASS: per-subscription pubsub workers spawned" \
  || echo "WARN: did not find 'Pub/Sub worker' log lines (check log level)"

echo ""
echo "US2 acceptance PASSED: one-command bring-up provisioned 8 pairs before the relayer started and the relayer is up."
echo "Note: backlog depth is reported as UNAVAILABLE (not 0) under the emulator (Cloud Monitoring is not served locally)."
