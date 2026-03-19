#!/usr/bin/env bash
# Trigger TRY_AGAIN_LATER on next sendTransaction call.
# The scenario fires once, then WireMock reverts to proxying.
#
# Usage: ./trigger-try-again-later.sh [wiremock_url]

WIREMOCK_URL="${1:-http://localhost:9090}"

echo "Arming TRY_AGAIN_LATER scenario..."
curl -s -X PUT "${WIREMOCK_URL}/__admin/scenarios/stellar-try-again-later/state" \
  -H 'Content-Type: application/json' \
  -d '{"state": "armed"}' | jq .

echo ""
echo "Next sendTransaction to ${WIREMOCK_URL} will return TRY_AGAIN_LATER."
echo "Subsequent calls will proxy to real Stellar RPC."
