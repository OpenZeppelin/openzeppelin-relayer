#!/usr/bin/env bash
# Trigger insufficient fee ERROR on next sendTransaction call.
# The scenario fires once, then WireMock reverts to proxying.
#
# Usage: ./trigger-insufficient-fee.sh [wiremock_url]

WIREMOCK_URL="${1:-http://localhost:9090}"

echo "Arming insufficient fee scenario..."
curl -s -X PUT "${WIREMOCK_URL}/__admin/scenarios/stellar-insufficient-fee/state" \
  -H 'Content-Type: application/json' \
  -d '{"state": "armed"}' | jq .

echo ""
echo "Next sendTransaction to ${WIREMOCK_URL} will return ERROR (TxInsufficientFee)."
echo "Subsequent calls will proxy to real Stellar RPC."
