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

# Validate required variables
if [ -z "$RELAYER_API_KEY" ] || [ "$RELAYER_API_KEY" = "your-api-key-here" ]; then
    log_error "RELAYER_API_KEY not set in .env.integration"
    exit 1
fi

# Parse command line arguments
COMMAND=${1:-up}
NETWORKS=${TEST_NETWORKS:-sepolia}

case "$COMMAND" in
    up|run)
        log_info "Starting integration test environment..."
        log_info "Networks: $NETWORKS"

        cd "$PROJECT_ROOT"
        docker-compose -f docker-compose.integration.yml up --build --abort-on-container-exit
        EXIT_CODE=$?

        if [ $EXIT_CODE -eq 0 ]; then
            log_success "Integration tests passed!"
        else
            log_error "Integration tests failed with exit code $EXIT_CODE"
        fi

        # Cleanup
        log_info "Cleaning up containers..."
        docker-compose -f docker-compose.integration.yml down

        exit $EXIT_CODE
        ;;

    build)
        log_info "Building integration test images..."
        cd "$PROJECT_ROOT"
        docker-compose -f docker-compose.integration.yml build
        log_success "Build complete!"
        ;;

    down|stop)
        log_info "Stopping integration test environment..."
        cd "$PROJECT_ROOT"
        docker-compose -f docker-compose.integration.yml down
        log_success "Stopped!"
        ;;

    logs)
        cd "$PROJECT_ROOT"
        docker-compose -f docker-compose.integration.yml logs -f
        ;;

    shell)
        log_info "Opening shell in integration-tests container..."
        cd "$PROJECT_ROOT"
        docker-compose -f docker-compose.integration.yml run --rm integration-tests bash
        ;;

    clean)
        log_info "Cleaning up Docker resources..."
        cd "$PROJECT_ROOT"
        docker-compose -f docker-compose.integration.yml down -v --remove-orphans
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
        echo "Environment variables (set in .env.integration):"
        echo "  RELAYER_API_KEY     API key for relayer service"
        echo "  TEST_NETWORKS       Networks to test (comma-separated)"
        echo "  TEST_MODE           Test mode: quick, ci, or full"
        echo ""
        echo "Examples:"
        echo "  $0 up                           # Run tests with default config"
        echo "  TEST_NETWORKS=sepolia $0 up     # Test only sepolia"
        echo "  $0 build                        # Build images only"
        echo "  $0 logs                         # View logs"
        ;;

    *)
        log_error "Unknown command: $COMMAND"
        echo "Run '$0 help' for usage information"
        exit 1
        ;;
esac
