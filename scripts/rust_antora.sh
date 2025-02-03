#!/bin/bash

# Exit immediately if a command exits with a non-zero status
set -e

# Base directories
NAME=$(grep '^name:' antora.yml | awk '{print $2}')
VERSION=$(grep '^version:' antora.yml | awk '{print $2}')
BUILD_DIR="build/site"
RUST_DOCS_DIR="modules/ROOT/pages/rust_docs"

if [ "$(basename "$PWD")" != "docs" ]; then
  echo "Error: You must run this script from the 'docs' directory."
  exit 1
fi
# Check if the target directory exists
TARGET_DIR="$BUILD_DIR/$NAME/$VERSION"
if [ ! -d "$TARGET_DIR" ]; then
  echo "Error: Target directory '$TARGET_DIR' not found."
  exit 1
fi

# Check if the Rust docs directory exists
DEST_DIR="$TARGET_DIR/rust_docs"
mkdir -p "$DEST_DIR"

# Copy the Rust docs to the target directory
if [ -d "$RUST_DOCS_DIR" ] && [ "$(ls -A "$RUST_DOCS_DIR")" ]; then
  echo "Copying '$RUST_DOCS_DIR' to '$DEST_DIR'..."
  cp -r "$RUST_DOCS_DIR/"* "$DEST_DIR/"
  echo "Rust docs successfully copied to '$DEST_DIR'."
else
  echo "Source directory '$RUST_DOCS_DIR' does not exist or is empty."
fi
