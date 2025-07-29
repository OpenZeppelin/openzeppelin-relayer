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
REMOTE=$(git remote get-url origin)
REMOTE=${REMOTE%.git}
REPO_FULL=${REMOTE#*github.com[:/]}
# For netlify, we need to use $HEAD to determine the branch
# If HEAD is not set, we default to the current branch
LOCAL_BRANCH="${HEAD:-$(git rev-parse --abbrev-ref HEAD)}"

SPEC_URL="https://raw.githubusercontent.com/${REPO_FULL}/${LOCAL_BRANCH}/docs/openapi.json"

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

# Copy the API docs file to the target directory
cat > "$BUILD_DIR/api_docs.html" <<EOF
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8"/>
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>OpenZeppelin Relayer API</title>
  <style>body{margin:0;padding:0}</style>
</head>
<body>
  <redoc spec-url="${SPEC_URL}" required-props-first="true"></redoc>
  <script src="https://cdn.redocly.com/redoc/latest/bundles/redoc.standalone.js"></script>
</body>
</html>
EOF

echo "✅ Generated api_docs.html in '$BUILD_DIR'."
