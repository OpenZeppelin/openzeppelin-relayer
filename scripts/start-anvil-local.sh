#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

CONTAINER_NAME="standalone-anvil"
RPC_URL="http://localhost:8545"

# Check if Anvil already running
if docker ps --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
    log_warn "Anvil container '$CONTAINER_NAME' is already running"
    log_info "If you need to restart it, run: ./scripts/stop-anvil-local.sh"
    exit 0
fi

log_info "Starting standalone Anvil container..."

# Start Anvil container
cd "$PROJECT_ROOT"
docker run -d \
    --name "$CONTAINER_NAME" \
    --rm \
    --entrypoint="" \
    -p 0.0.0.0:8545:8545 \
    -v "$(pwd)/tests/integration/contracts:/contracts:ro" \
    ghcr.io/foundry-rs/foundry:stable \
    sh -c "anvil --host 0.0.0.0 --chain-id 31337 --block-time 1"

log_success "Anvil container started!"

# Wait for Anvil RPC ready
log_info "Waiting for Anvil RPC to be ready..."
max_attempts=30
attempt=0
while [ $attempt -lt $max_attempts ]; do
    # Try using cast from inside the container (more reliable)
    if docker exec "$CONTAINER_NAME" cast client --rpc-url http://localhost:8545 > /dev/null 2>&1; then
        log_success "Anvil RPC is ready!"
        break
    fi

    # Fallback: try curl from host
    if curl -s -f -X POST -H "Content-Type: application/json" \
        --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
        "$RPC_URL" > /dev/null 2>&1; then
        log_success "Anvil RPC is ready!"
        break
    fi

    attempt=$((attempt + 1))
    if [ $((attempt % 5)) -eq 0 ]; then
        log_info "Attempt $attempt/$max_attempts: Waiting for Anvil..."
    fi
    sleep 1
done

if [ $attempt -eq $max_attempts ]; then
    log_error "Anvil RPC did not become ready after $max_attempts attempts"
    log_info "Checking container logs..."
    docker logs "$CONTAINER_NAME" --tail 20
    docker stop "$CONTAINER_NAME" || true
    exit 1
fi

# Deploy contracts
log_info "Deploying contracts..."
ANVIL_CONTAINER="$CONTAINER_NAME" \
RPC_URL="$RPC_URL" \
REGISTRY_PATH="tests/integration/config/local/registry.json" \
"$PROJECT_ROOT/scripts/deploy-local-contracts.sh"

# Create standalone config directory
STANDALONE_CONFIG_DIR="$PROJECT_ROOT/tests/integration/config/local-standalone"
mkdir -p "$STANDALONE_CONFIG_DIR"

# Generate standalone config.json (same as local but RPC: http://localhost:8545)
log_info "Generating standalone config..."
if command -v jq &> /dev/null; then
    jq '.relayers[0].custom_rpc_urls[0].url = "http://localhost:8545" |
        .relayers[0].name = "Standalone Anvil Relayer"' \
        "$PROJECT_ROOT/tests/integration/config/local/config.json" > \
        "$STANDALONE_CONFIG_DIR/config.json"
else
    python3 << EOF
import json
import sys

with open("$PROJECT_ROOT/tests/integration/config/local/config.json", "r") as f:
    config = json.load(f)

config["relayers"][0]["custom_rpc_urls"][0]["url"] = "http://localhost:8545"
config["relayers"][0]["name"] = "Standalone Anvil Relayer"

with open("$STANDALONE_CONFIG_DIR/config.json", "w") as f:
    json.dump(config, f, indent=2)
EOF
fi

# Create symlink to registry.json
log_info "Creating registry symlink..."
ln -sf "../local/registry.json" "$STANDALONE_CONFIG_DIR/registry.json"

log_success "Standalone Anvil setup complete!"
echo ""
echo "Next steps:"
echo "  1. Set CONFIG_PATH=tests/integration/config/local-standalone/config.json"
echo "  2. Run relayer: cargo run"
echo "  3. Run tests: cargo test --features integration-tests --test integration"
echo ""
echo "To stop Anvil: ./scripts/stop-anvil-local.sh"
