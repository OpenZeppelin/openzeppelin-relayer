#!/usr/bin/env bash
# Run integration tests in Docker
# This script orchestrates the full integration test environment

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Helper functions
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

# Check if .env.integration exists
if [ ! -f "$PROJECT_ROOT/.env.integration" ]; then
    log_warn ".env.integration not found"
    log_info "Creating from .env.integration.example..."

    if [ -f "$PROJECT_ROOT/.env.integration.example" ]; then
        cp "$PROJECT_ROOT/.env.integration.example" "$PROJECT_ROOT/.env.integration"
        log_warn "Please edit .env.integration with your API keys before running tests"
        exit 1
    else
        log_error ".env.integration.example not found!"
        exit 1
    fi
fi

# Load environment variables
set -a
source "$PROJECT_ROOT/.env.integration"
set +a

# MODE handling (local/testnet)
MODE=${MODE:-local}  # Default to local

if [ "$MODE" = "local" ]; then
    PROFILE="--profile local"
    CONFIG_SOURCE="./tests/integration/config/local"
    export TEST_REGISTRY_PATH="tests/integration/config/local/registry.json"

    # Verify keystore exists
    if [ ! -f "$PROJECT_ROOT/tests/integration/config/local/keys/anvil-test.json" ]; then
        log_error "Anvil keystore not found!"
        log_info "Please create the keystore using Anvil's default test account (account 0)."
        log_info "See: tests/integration/README.md for setup instructions"
        echo ""
        exit 1
    fi

elif [ "$MODE" = "testnet" ]; then
    PROFILE=""
    CONFIG_SOURCE="./tests/integration/config/testnet"
    export TEST_REGISTRY_PATH="tests/integration/config/testnet/registry.json"
    # Create from examples if needed
    if [ ! -f "$PROJECT_ROOT/tests/integration/config/testnet/config.json" ]; then
        log_warn "testnet config.json not found, creating from example..."
        mkdir -p "$PROJECT_ROOT/tests/integration/config/testnet"
        if [ -f "$PROJECT_ROOT/tests/integration/config/config.example.json" ]; then
            cp "$PROJECT_ROOT/tests/integration/config/config.example.json" "$PROJECT_ROOT/tests/integration/config/testnet/config.json"
        fi
    fi
    if [ ! -f "$PROJECT_ROOT/tests/integration/config/testnet/registry.json" ]; then
        log_warn "testnet registry.json not found, creating from example..."
        mkdir -p "$PROJECT_ROOT/tests/integration/config/testnet"
        if [ -f "$PROJECT_ROOT/tests/integration/config/registry.example.json" ]; then
            cp "$PROJECT_ROOT/tests/integration/config/registry.example.json" "$PROJECT_ROOT/tests/integration/config/testnet/registry.json"
        fi
    fi

    # Ensure keys directory exists
    mkdir -p "$PROJECT_ROOT/tests/integration/config/testnet/keys"

    # Verify signer keystore exists (skip if using KMS)
    if [ -z "$USE_KMS" ] || [ "$USE_KMS" != "true" ]; then
        if [ ! -f "$PROJECT_ROOT/tests/integration/config/testnet/keys/local-signer.json" ]; then
            log_error "Testnet signer keystore not found!"
            log_info "Please create the keystore by running:"
            echo ""
            echo "mkdir -p tests/integration/config/testnet/keys"
            echo ""
            echo "cargo run --example create_key -- \\"
            echo "  --password \"\${KEYSTORE_PASSPHRASE}\" \\"
            echo "  --output-dir tests/integration/config/testnet/keys \\"
            echo "  --filename local-signer.json"
            echo ""
            echo "Or use an existing keystore file and update the path in config.json"
            echo ""
            echo "Alternatively, set USE_KMS=true to use KMS signers instead"
            echo ""
            exit 1
        fi
    else
        log_info "Using KMS signers (USE_KMS=true), skipping keystore check"
    fi
else
    log_error "Invalid MODE: $MODE (must be 'local' or 'testnet')"
    exit 1
fi

export CONFIG_SOURCE

# Validate required variables
if [ -z "$API_KEY" ] || [ "$API_KEY" = "your-api-key-here" ]; then
    log_error "API_KEY not set in .env.integration"
    exit 1
fi

# For local mode, config files should already exist (committed to git)
# For testnet mode, they are created above if needed

# Parse command line arguments
COMMAND=${1:-up}

# Helper function to wait for container health
wait_for_health() {
    local container_name=$1
    local max_attempts=30
    local attempt=0

    log_info "Waiting for $container_name to be healthy..."
    while [ $attempt -lt $max_attempts ]; do
        if docker inspect --format='{{.State.Health.Status}}' "$container_name" 2>/dev/null | grep -q "healthy"; then
            log_success "$container_name is healthy!"
            return 0
        fi
        attempt=$((attempt + 1))
        sleep 1
    done

    log_error "$container_name did not become healthy after $max_attempts attempts"
    return 1
}

case "$COMMAND" in
    up|run)
        log_info "Starting integration test environment (MODE: $MODE)..."

        cd "$PROJECT_ROOT"

        # Start services based on mode
        if [ "$MODE" = "local" ]; then
            log_info "Starting Anvil and Redis..."
            docker compose $PROFILE -f docker-compose.integration.yml up -d anvil redis
        else
            log_info "Starting Redis..."
            docker compose $PROFILE -f docker-compose.integration.yml up -d redis
        fi

        # If local mode, wait for Anvil and deploy contracts before starting relayer
        if [ "$MODE" = "local" ]; then
            # Wait for Anvil healthy
            if wait_for_health integration-anvil; then
                # Give Anvil a moment to fully initialize RPC endpoint
                sleep 2
                # Deploy contracts
                log_info "Deploying contracts to Anvil..."
                ANVIL_CONTAINER="integration-anvil" \
                RPC_URL="http://localhost:8545" \
                REGISTRY_PATH="tests/integration/config/local/registry.json" \
                NETWORK_NAME="localhost-integration" \
                "$PROJECT_ROOT/scripts/deploy-local-contracts.sh"

                log_info "Using local config files:"
                log_info "  Registry (for tests): $TEST_REGISTRY_PATH"
                log_info "  Config (for relayer): config/config.json"
                log_info "  Tests discover relayers via API (GET /api/v1/relayers)"

                # Verify Anvil RPC is accessible before starting relayer
                log_info "Verifying Anvil RPC connectivity..."
                max_attempts=10
                attempt=0
                while [ $attempt -lt $max_attempts ]; do
                    if docker exec integration-anvil cast client --rpc-url http://localhost:8545 2>/dev/null | grep -q "anvil"; then
                        log_success "Anvil RPC is ready!"
                        break
                    fi
                    attempt=$((attempt + 1))
                    sleep 1
                done

                if [ $attempt -eq $max_attempts ]; then
                    log_error "Anvil RPC did not become ready"
                    docker compose $PROFILE -f docker-compose.integration.yml down
                    exit 1
                fi
            else
                log_error "Failed to start Anvil"
                docker compose $PROFILE -f docker-compose.integration.yml down
                exit 1
            fi
        fi

        # Now start the relayer (after Anvil is ready in local mode)
        log_info "Starting relayer..."
        docker compose $PROFILE -f docker-compose.integration.yml up -d relayer

        # Run tests
        log_info "Running integration tests..."
        docker compose $PROFILE -f docker-compose.integration.yml up --build --abort-on-container-exit integration-tests
        EXIT_CODE=$?

        if [ $EXIT_CODE -eq 0 ]; then
            log_success "Integration tests passed!"
        else
            log_error "Integration tests failed with exit code $EXIT_CODE"
        fi

        # Cleanup
        log_info "Cleaning up containers..."
        docker compose -f docker-compose.integration.yml down

        exit $EXIT_CODE
        ;;

    build)
        log_info "Building integration test images..."
        cd "$PROJECT_ROOT"
        docker compose -f docker-compose.integration.yml build
        log_success "Build complete!"
        ;;

    down|stop)
        log_info "Stopping integration test environment..."
        cd "$PROJECT_ROOT"
        docker compose -f docker-compose.integration.yml down
        log_success "Stopped!"
        ;;

    logs)
        cd "$PROJECT_ROOT"
        docker compose -f docker-compose.integration.yml logs -f
        ;;

    shell)
        log_info "Opening shell in integration-tests container..."
        cd "$PROJECT_ROOT"
        docker compose -f docker-compose.integration.yml run --rm integration-tests bash
        ;;

    clean)
        log_info "Cleaning up Docker resources..."
        cd "$PROJECT_ROOT"
        docker compose -f docker-compose.integration.yml down -v --remove-orphans
        log_success "Cleanup complete!"
        ;;

    help|--help|-h)
        echo "Usage: $0 [COMMAND]"
        echo ""
        echo "Commands:"
        echo "  up, run     Start services and run integration tests (default)"
        echo "  build       Build Docker images without running tests"
        echo "  down, stop  Stop all services"
        echo "  logs        Follow logs from all services"
        echo "  shell       Open a shell in the test container"
        echo "  clean       Remove all containers, networks, and volumes"
        echo "  help        Show this help message"
        echo ""
        echo "Environment variables (set in .env.integration or command line):"
        echo "  MODE                Test mode: 'local' (default) or 'testnet'"
        echo "  API_KEY             API key for relayer service"
        echo "  USE_KMS             Set to 'true' to use KMS signers (skips keystore check)"
        echo ""
        echo "Modes:"
        echo "  local (default)     Use local Anvil node (no testnet funds needed)"
        echo "  testnet            Use live testnet networks"
        echo ""
        echo "Configuration:"
        echo "  Local mode:         tests/integration/config/local/ (committed to git)"
        echo "  Testnet mode:       tests/integration/config/testnet/ (git-ignored)"
        echo ""
        echo "How tests discover relayers:"
        echo "  - Tests query the running relayer API (GET /api/v1/relayers)"
        echo "  - config.json is used to START the relayer, not by tests"
        echo "  - registry.json provides network metadata for tests"
        echo ""
        echo "Network and relayer selection:"
        echo "  - Networks: Enable/disable in registry.json (enabled: true/false)"
        echo "  - Relayers: Discovered via API, filtered by network + !paused"
        echo "  - Tests run on ALL active (unpaused) relayers for each enabled network"
        echo ""
        echo "Examples:"
        echo "  $0 up                    # Local mode (default)"
        echo "  MODE=local $0 up         # Explicit local mode"
        echo "  MODE=testnet $0 up       # Testnet mode"
        echo "  $0 build                 # Build images only"
        echo "  $0 logs                  # View logs"
        ;;

    *)
        log_error "Unknown command: $COMMAND"
        echo "Run '$0 help' for usage information"
        exit 1
        ;;
esac
