#!/usr/bin/env bash
# Reset all WireMock scenarios and request log.
#
# Usage: ./reset-all.sh [wiremock_url]

WIREMOCK_URL="${1:-http://localhost:9090}"

echo "Resetting all scenarios..."
curl -s -X POST "${WIREMOCK_URL}/__admin/scenarios/reset"

echo "Clearing request log..."
curl -s -X DELETE "${WIREMOCK_URL}/__admin/requests"

echo "Done. All scenarios reset to initial state."
