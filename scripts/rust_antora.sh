#!/bin/bash

# Exit immediately if a command exits with a non-zero status
set -euo pipefail

# Base directories
REPO_ROOT="$PWD"
DOCS_DIR="$REPO_ROOT/docs"
NAME=$(grep '^name:' "$DOCS_DIR/antora.yml" | awk '{print $2}')
VERSION=$(grep '^version:' "$DOCS_DIR/antora.yml" | awk '{print $2}')
BUILD_DIR="$DOCS_DIR/build/site"
RUST_DOCS_DIR="$DOCS_DIR/rust_docs"

# Check if the target directory exists
if [ ! -d "$BUILD_DIR" ]; then
  echo "Error: Build directory '$BUILD_DIR' not found."
  exit 1
fi

# Copy the Rust docs to the target directory
if [ -d "$RUST_DOCS_DIR" ] && [ "$(ls -A "$RUST_DOCS_DIR")" ]; then
  echo "Copying '$RUST_DOCS_DIR' to '$BUILD_DIR'..."
  cp -r "$RUST_DOCS_DIR/doc/"* "$BUILD_DIR/"
  echo "Rust docs successfully copied to '$BUILD_DIR'."
  # Remove the original Rust docs directory
  echo "Removing original Rust docs directory '$RUST_DOCS_DIR'..."
  rm -rf "$RUST_DOCS_DIR"
  echo "Original Rust docs directory '$RUST_DOCS_DIR' removed."
else
  echo "Source directory '$RUST_DOCS_DIR' does not exist or is empty."
fi
