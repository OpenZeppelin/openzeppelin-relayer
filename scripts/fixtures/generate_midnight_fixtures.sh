#!/bin/bash

# Script to generate Midnight test fixtures using the unified generator

set -e

echo "🌙 Midnight Test Fixture Generator Script"
echo "========================================"
echo

# Check if WALLET_SEED is provided
if [ -z "$WALLET_SEED" ]; then
    echo "⚠️  No WALLET_SEED environment variable found."
    echo "Please provide a funded wallet seed to generate meaningful fixtures."
    echo
    echo "Example:"
    echo "  WALLET_SEED=0e0cc7db98c60a39a6b0888795ba3f1bb1d61298cce264d4beca1529650e9041 $0"
    echo
    exit 1
fi

# Create fixture directory if it doesn't exist
mkdir -p tests/fixtures/midnight

echo "📋 Current configuration:"
echo "  WALLET_SEED: $WALLET_SEED"
echo "  START_HEIGHT: ${START_HEIGHT:-0}"
echo "  SAVE_INTERVAL: ${SAVE_INTERVAL:-not set}"
echo "  RUST_LOG: ${RUST_LOG:-info}"
echo

# Generate complete context fixture
echo "🏗️  Generating complete context fixture (wallet + ledger state)..."
echo
cargo run --example generate_midnight_fixtures

echo
echo "✅ Fixture generation complete!"
echo
echo "📁 Generated fixtures:"
ls -la tests/fixtures/midnight/ 2>/dev/null || echo "No fixtures found"

echo
echo "📖 To use these fixtures in tests:"
echo "1. Ensure the fixtures are in the tests/fixtures/midnight/ directory"
echo "2. Use create_funded_test_context() with the same wallet seed"
echo "3. The test utilities will automatically load the fixtures"
