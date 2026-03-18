#!/usr/bin/env bash
# Hotbar Trace Viewer — DeltaGraph Edition
# Usage: ./tools/trace-viewer.sh [--port 8777] [--db path/to/traces.db]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec python3 "$SCRIPT_DIR/trace-viewer.py" "$@"
