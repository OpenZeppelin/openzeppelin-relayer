#!/bin/bash
set -euo pipefail

# Default values
ANVIL_CONTAINER="${ANVIL_CONTAINER:-integration-anvil}"
RPC_URL="${RPC_URL:-http://localhost:8545}"
REGISTRY_PATH="${REGISTRY_PATH:-tests/integration/config/local/registry.json}"
# Network name to update in registry - defaults to localhost-integration for Docker mode
NETWORK_NAME="${NETWORK_NAME:-localhost-integration}"

echo "Deploying contracts to Anvil..."
echo "Container: $ANVIL_CONTAINER"
echo "RPC URL: $RPC_URL"
echo "Registry: $REGISTRY_PATH"
echo "Network: $NETWORK_NAME"

# Wait for Anvil RPC to be ready
echo "Waiting for Anvil RPC to be ready..."

# First, wait for container to be running
max_attempts=10
attempt=0
while [ $attempt -lt $max_attempts ]; do
    if docker ps --format '{{.Names}}' | grep -q "^${ANVIL_CONTAINER}$"; then
        break
    fi
    attempt=$((attempt + 1))
    sleep 1
done

if [ $attempt -eq $max_attempts ]; then
    echo "Error: Container $ANVIL_CONTAINER is not running"
    exit 1
fi

# Now wait for RPC to be ready
max_attempts=30
attempt=0
while [ $attempt -lt $max_attempts ]; do
    # Try using cast from inside the container (more reliable for Docker Compose)
    if docker exec "$ANVIL_CONTAINER" cast client --rpc-url http://localhost:8545 2>/dev/null | grep -q "anvil"; then
        echo "Anvil RPC is ready!"
        break
    fi

    # Fallback: try curl from host (for standalone mode or if docker exec fails)
    if curl -s -f -X POST -H "Content-Type: application/json" \
        --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
        "$RPC_URL" 2>/dev/null | grep -q "result"; then
        echo "Anvil RPC is ready!"
        break
    fi

    attempt=$((attempt + 1))
    if [ $((attempt % 5)) -eq 0 ]; then
        echo "Attempt $attempt/$max_attempts: Waiting for Anvil RPC..."
        # Debug: show container status
        docker ps --filter "name=$ANVIL_CONTAINER" --format "Container status: {{.Status}}" 2>/dev/null || true
    fi
    sleep 1
done

if [ $attempt -eq $max_attempts ]; then
    echo "Error: Anvil RPC did not become ready after $max_attempts attempts"
    echo "Checking container status..."
    docker ps --filter "name=$ANVIL_CONTAINER" --format "{{.Names}}: {{.Status}}"
    echo "Checking container logs..."
    docker logs "$ANVIL_CONTAINER" --tail 20
    exit 1
fi

# Deploy contracts directly using forge create
# Use Anvil's unlocked account #0 (pre-funded, no private key needed)
# Anvil account #0 address: 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
ANVIL_ACCOUNT_0="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"

# Copy contracts to writable directory since /contracts is read-only
# This allows forge to write compilation artifacts
echo "Copying contracts to writable directory..."
docker exec "$ANVIL_CONTAINER" sh -c "rm -rf /tmp/contracts && cp -r /contracts /tmp/contracts" > /dev/null 2>&1

echo "Deploying Counter contract..."
if ! counter_output=$(docker exec "$ANVIL_CONTAINER" sh -c \
    "cd /tmp/contracts && forge create src/Counter.sol:Counter --rpc-url http://localhost:8545 --from $ANVIL_ACCOUNT_0 --unlocked --broadcast --legacy" 2>&1); then
    echo "Error: Failed to deploy Counter contract"
    echo "$counter_output"
    exit 1
fi

counter_address=$(echo "$counter_output" | grep "Deployed to:" | sed -E 's/.*Deployed to: (0x[0-9a-fA-F]{40}).*/\1/' | head -1)
if [ -z "$counter_address" ]; then
    echo "Error: Failed to parse Counter address from output"
    echo "Output:"
    echo "$counter_output"
    exit 1
fi

echo "Counter deployed at: $counter_address"

# Update registry.json with jq (or Python fallback)
if command -v jq &> /dev/null; then
    echo "Updating registry.json with jq..."
    jq --arg counter "$counter_address" --arg network "$NETWORK_NAME" \
        '.networks[$network].contracts.simple_storage = $counter' \
        "$REGISTRY_PATH" > "${REGISTRY_PATH}.tmp" && \
        mv "${REGISTRY_PATH}.tmp" "$REGISTRY_PATH"
else
    echo "jq not found, using Python fallback..."
    python3 << EOF
import json
import sys

with open("$REGISTRY_PATH", "r") as f:
    registry = json.load(f)

# Ensure network exists
if "$NETWORK_NAME" not in registry["networks"]:
    registry["networks"]["$NETWORK_NAME"] = {"contracts": {}}
elif "contracts" not in registry["networks"]["$NETWORK_NAME"]:
    registry["networks"]["$NETWORK_NAME"]["contracts"] = {}

registry["networks"]["$NETWORK_NAME"]["contracts"]["simple_storage"] = "$counter_address"

with open("$REGISTRY_PATH", "w") as f:
    json.dump(registry, f, indent=2)
EOF
fi

echo "Registry updated successfully!"
echo "Contract addresses written to $REGISTRY_PATH"
