#!/bin/bash
# build.sh - Build Hotbar executable
# Creates self-contained ./hotbar bundle

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

if ! command -v ags &>/dev/null; then
    echo "Error: ags not found"
    exit 1
fi

echo "Bundling app.tsx -> ./hotbar"
ags bundle app.tsx ./hotbar -d "SRC='$SCRIPT_DIR'"

chmod +x ./hotbar
echo "Build complete: ./hotbar ($(du -h ./hotbar | cut -f1))"
