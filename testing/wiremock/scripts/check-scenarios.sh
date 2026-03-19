#!/usr/bin/env bash
# Show current state of all WireMock scenarios.
#
# Usage: ./check-scenarios.sh [wiremock_url]

WIREMOCK_URL="${1:-http://localhost:9090}"

echo "=== WireMock Scenarios ==="
curl -s "${WIREMOCK_URL}/__admin/scenarios" | jq '.scenarios[] | {name: .name, state: .state}'

echo ""
echo "=== Recent Requests (last 5) ==="
curl -s "${WIREMOCK_URL}/__admin/requests?limit=5" | jq '.requests[] | {url: .request.url, method: .request.method, matched: .wasMatched, timestamp: .request.loggedDate}'
