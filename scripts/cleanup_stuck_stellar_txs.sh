#!/usr/bin/env bash

###############################################################################
# Redis-based cleanup script for stuck Stellar transactions
#
# Run this ONCE after deploying the status check fixes to clean up existing
# poison transactions.
#
# This script identifies and marks as Expired transactions that:
# 1. Are Stellar network transactions
# 2. Are in Pending or Sent status (unsubmitted states)
# 3. Have missing or empty on-chain hash in network_data
# 4. Were created more than MIN_AGE_HOURS ago
#
# Usage:
#   ./scripts/cleanup_stuck_stellar_txs.sh --dry-run
#   ./scripts/cleanup_stuck_stellar_txs.sh --redis-url redis://localhost:6379 --key-prefix relayer
###############################################################################

set -euo pipefail

# Default values
REDIS_URL="redis://localhost:6379"
KEY_PREFIX="relayer"
DRY_RUN=false
MIN_AGE_HOURS=1

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --redis-url)
            REDIS_URL="$2"
            shift 2
            ;;
        --key-prefix)
            KEY_PREFIX="$2"
            shift 2
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --min-age-hours)
            MIN_AGE_HOURS="$2"
            shift 2
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --redis-url URL       Redis connection URL (default: redis://localhost:6379)"
            echo "  --key-prefix PREFIX   Redis key prefix (default: relayer)"
            echo "  --dry-run             Preview mode - show what would be cleaned without making changes"
            echo "  --min-age-hours N     Minimum age in hours before marking as expired (default: 1)"
            echo "  -h, --help            Show this help message"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

# Check dependencies
if ! command -v redis-cli &> /dev/null; then
    echo "ERROR: redis-cli is not installed"
    exit 1
fi

if ! command -v jq &> /dev/null; then
    echo "ERROR: jq is not installed"
    exit 1
fi

# Extract Redis connection parameters from URL
# Format: redis://[username:password@]host:port[/database]
REDIS_HOST=$(echo "$REDIS_URL" | sed -E 's#redis://([^:@]*:)?([^@]*@)?([^:/]+).*#\3#')
REDIS_PORT=$(echo "$REDIS_URL" | sed -E 's#redis://[^:]*:([0-9]+).*#\1#')
if [[ "$REDIS_PORT" == "$REDIS_URL" ]]; then
    REDIS_PORT=6379
fi

# Redis CLI command with connection parameters
REDIS_CMD="redis-cli -h $REDIS_HOST -p $REDIS_PORT"

# Calculate the threshold timestamp (RFC3339 format)
if [[ "$OSTYPE" == "darwin"* ]]; then
    # macOS
    THRESHOLD_TIMESTAMP=$(date -u -v-${MIN_AGE_HOURS}H +"%Y-%m-%dT%H:%M:%S+00:00")
else
    # Linux
    THRESHOLD_TIMESTAMP=$(date -u -d "${MIN_AGE_HOURS} hours ago" +"%Y-%m-%dT%H:%M:%S+00:00")
fi

# Current timestamp for delete_at (24 hours from now)
if [[ "$OSTYPE" == "darwin"* ]]; then
    DELETE_AT_TIMESTAMP=$(date -u -v+24H +"%Y-%m-%dT%H:%M:%S+00:00")
else
    DELETE_AT_TIMESTAMP=$(date -u -d "24 hours" +"%Y-%m-%dT%H:%M:%S+00:00")
fi

# Statistics
TOTAL_SCANNED=0
STELLAR_TRANSACTIONS=0
STUCK_TRANSACTIONS=0
MARKED_EXPIRED=0

echo "=== Redis Cleanup for Stuck Stellar Transactions ==="
echo "Redis: $REDIS_HOST:$REDIS_PORT"
echo "Key prefix: $KEY_PREFIX"
echo "Dry run: $DRY_RUN"
echo "Min age: $MIN_AGE_HOURS hours"
echo "Threshold: $THRESHOLD_TIMESTAMP"
echo ""

# Get all relayer IDs
RELAYER_LIST_KEY="${KEY_PREFIX}:relayer_list"
RELAYER_IDS=$($REDIS_CMD SMEMBERS "$RELAYER_LIST_KEY")

if [[ -z "$RELAYER_IDS" ]]; then
    echo "No relayers found"
    exit 0
fi

RELAYER_COUNT=$(echo "$RELAYER_IDS" | wc -l | tr -d ' ')
echo "Found $RELAYER_COUNT relayer(s)"
echo ""

# Process each relayer
for RELAYER_ID in $RELAYER_IDS; do
    echo "Processing relayer: $RELAYER_ID"

    # Get all transaction IDs for this relayer (sorted by created_at)
    TX_BY_CREATED_AT_KEY="${KEY_PREFIX}:relayer:${RELAYER_ID}:tx_by_created_at"
    TX_IDS=$($REDIS_CMD ZRANGE "$TX_BY_CREATED_AT_KEY" 0 -1)

    if [[ -z "$TX_IDS" ]]; then
        echo "  No transactions found"
        continue
    fi

    # Process each transaction
    for TX_ID in $TX_IDS; do
        ((TOTAL_SCANNED++))

        # Fetch transaction JSON
        TX_KEY="${KEY_PREFIX}:relayer:${RELAYER_ID}:tx:${TX_ID}"
        TX_JSON=$($REDIS_CMD GET "$TX_KEY")

        if [[ -z "$TX_JSON" ]]; then
            continue
        fi

        # Parse transaction fields
        NETWORK_TYPE=$(echo "$TX_JSON" | jq -r '.network_type')
        STATUS=$(echo "$TX_JSON" | jq -r '.status')
        CREATED_AT=$(echo "$TX_JSON" | jq -r '.created_at')
        HASH=$(echo "$TX_JSON" | jq -r '.network_data.hash // empty')

        # Filter for Stellar transactions
        if [[ "$NETWORK_TYPE" != "Stellar" ]]; then
            continue
        fi
        ((STELLAR_TRANSACTIONS++))

        # Check if in unsubmitted state (Pending or Sent)
        if [[ "$STATUS" != "Pending" && "$STATUS" != "Sent" ]]; then
            continue
        fi

        # Check if hash is missing or empty
        if [[ -n "$HASH" ]]; then
            continue
        fi

        # Check if older than threshold
        if [[ "$CREATED_AT" > "$THRESHOLD_TIMESTAMP" ]]; then
            continue
        fi

        # This transaction is stuck!
        ((STUCK_TRANSACTIONS++))

        # Calculate age in hours
        if [[ "$OSTYPE" == "darwin"* ]]; then
            CREATED_EPOCH=$(date -jf "%Y-%m-%dT%H:%M:%S" "$(echo $CREATED_AT | cut -d+ -f1)" +"%s" 2>/dev/null || echo "0")
            NOW_EPOCH=$(date +"%s")
        else
            CREATED_EPOCH=$(date -d "$CREATED_AT" +"%s" 2>/dev/null || echo "0")
            NOW_EPOCH=$(date +"%s")
        fi
        AGE_HOURS=$(( (NOW_EPOCH - CREATED_EPOCH) / 3600 ))

        echo "  [STUCK] tx_id=$TX_ID status=$STATUS created_at=$CREATED_AT age=${AGE_HOURS}h"

        if [[ "$DRY_RUN" == "true" ]]; then
            ((MARKED_EXPIRED++))
            continue
        fi

        # Update transaction JSON
        UPDATED_TX_JSON=$(echo "$TX_JSON" | jq \
            --arg status "Expired" \
            --arg reason "Transaction stuck in unsubmitted state for too long (cleanup script)" \
            --arg delete_at "$DELETE_AT_TIMESTAMP" \
            '.status = $status | .status_reason = $reason | .delete_at = $delete_at')

        # Save updated transaction
        $REDIS_CMD SET "$TX_KEY" "$UPDATED_TX_JSON" > /dev/null

        # Update status indexes
        # Remove from old status SET
        OLD_STATUS_KEY="${KEY_PREFIX}:relayer:${RELAYER_ID}:status:${STATUS}"
        $REDIS_CMD SREM "$OLD_STATUS_KEY" "$TX_ID" > /dev/null

        # Remove from old status SORTED SET
        OLD_STATUS_SORTED_KEY="${KEY_PREFIX}:relayer:${RELAYER_ID}:status_sorted:${STATUS}"
        $REDIS_CMD ZREM "$OLD_STATUS_SORTED_KEY" "$TX_ID" > /dev/null

        # Add to new status SET
        NEW_STATUS_KEY="${KEY_PREFIX}:relayer:${RELAYER_ID}:status:Expired"
        $REDIS_CMD SADD "$NEW_STATUS_KEY" "$TX_ID" > /dev/null

        # Add to new status SORTED SET (score = created_at timestamp in milliseconds)
        NEW_STATUS_SORTED_KEY="${KEY_PREFIX}:relayer:${RELAYER_ID}:status_sorted:Expired"

        # Convert RFC3339 timestamp to milliseconds since epoch
        if [[ "$OSTYPE" == "darwin"* ]]; then
            CREATED_MS=$(( CREATED_EPOCH * 1000 ))
        else
            CREATED_MS=$(( CREATED_EPOCH * 1000 ))
        fi

        $REDIS_CMD ZADD "$NEW_STATUS_SORTED_KEY" "$CREATED_MS" "$TX_ID" > /dev/null

        ((MARKED_EXPIRED++))
        echo "  [UPDATED] Marked as Expired"
    done
done

echo ""
echo "=== Cleanup Summary ==="
echo "Total transactions scanned: $TOTAL_SCANNED"
echo "Stellar transactions: $STELLAR_TRANSACTIONS"
echo "Stuck transactions found: $STUCK_TRANSACTIONS"
echo "Transactions marked as Expired: $MARKED_EXPIRED"

if [[ "$DRY_RUN" == "true" ]]; then
    echo ""
    echo "DRY RUN MODE - No changes were made"
    echo "Run without --dry-run to actually clean up these transactions"
fi
