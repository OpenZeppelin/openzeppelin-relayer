#!/bin/bash

set -euo pipefail

ANTORA_FILE="../docs/antora.yml"
ADOC_FILE="../docs/modules/ROOT/pages/structure.adoc"

FULL_VERSION=$(grep '^version:' "$ANTORA_FILE" | awk '{print $2}')
MAJOR_MINOR_VERSION=$(echo "$FULL_VERSION" | cut -d. -f1,2)

sed -i '' "s/{version}/$MAJOR_MINOR_VERSION/g" "$ADOC_FILE"
