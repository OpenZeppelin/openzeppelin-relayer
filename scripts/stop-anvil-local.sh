#!/bin/bash
set -euo pipefail

CONTAINER_NAME="standalone-anvil"

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

if docker ps --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
    log_info "Stopping Anvil container..."
    docker stop "$CONTAINER_NAME"
    log_success "Anvil container stopped!"
elif docker ps -a --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
    log_warn "Anvil container exists but is not running"
    docker rm "$CONTAINER_NAME" || true
else
    log_warn "Anvil container '$CONTAINER_NAME' not found"
fi
