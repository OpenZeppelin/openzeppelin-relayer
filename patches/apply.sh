#!/usr/bin/env bash
#
# Apply the midnight-ledger patch to a local crate copy.
#
# Usage:
#   ./patches/apply.sh                        # auto-detect source from cargo cache
#   ./patches/apply.sh /path/to/source         # explicit source directory
#
# The script copies the upstream midnight-ledger crate source to
# /tmp/midnight-ledger-patched, applies the patch, and prints the
# Cargo.toml [patch] section you need.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PATCH_FILE="$SCRIPT_DIR/midnight-ledger-8.0.2.patch"
TARGET="/tmp/midnight-ledger-patched"

# Resolve source directory
if [[ $# -ge 1 ]]; then
    SOURCE="$1"
else
    # Auto-detect from cargo git checkout (midnight-ledger 8.0.2)
    CHECKOUT_BASE="$HOME/.cargo/registry/src/index.crates.io-*/midnight-ledger-8.0.2"
    # shellcheck disable=SC2086
    SOURCE=$(ls -d $CHECKOUT_BASE 2>/dev/null | head -1 || true)

    if [[ -z "$SOURCE" ]]; then
        # Fall back to git checkout
        CHECKOUT_BASE="$HOME/.cargo/git/checkouts/midnight-ledger-*/*/ledger"
        # shellcheck disable=SC2086
        SOURCE=$(ls -d $CHECKOUT_BASE 2>/dev/null | head -1 || true)
    fi

    if [[ -z "$SOURCE" ]]; then
        echo "ERROR: Could not find midnight-ledger source in cargo cache."
        echo "Run 'cargo fetch' first, or provide the source path as an argument."
        exit 1
    fi
fi

echo "Source:  $SOURCE"
echo "Target:  $TARGET"
echo "Patch:   $PATCH_FILE"
echo

# Clean and copy
rm -rf "$TARGET"
cp -R "$SOURCE" "$TARGET"

# Apply patch (strip leading a/b prefix)
cd "$TARGET"
patch -p1 < "$PATCH_FILE"

echo
echo "Patch applied successfully."
echo
echo "Add this to your Cargo.toml:"
echo
echo '  [patch.crates-io]'
echo '  midnight-ledger = { path = "/tmp/midnight-ledger-patched" }'
echo
