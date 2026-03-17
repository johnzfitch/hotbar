#!/bin/bash
# run.sh - Run Hotbar with environment setup

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Force Adwaita icons - some themes cause GTK4 crashes
export GTK_ICON_THEME=Adwaita

# GObject introspection paths for Astal libraries
export GI_TYPELIB_PATH="/usr/lib/girepository-1.0${GI_TYPELIB_PATH:+:$GI_TYPELIB_PATH}"

# Wrapper flags:
#   --system     Include runtime-managed system events (~/.codex, ~/.claude)
#   --no-system  Force-hide runtime-managed system events
forward_args=()
for arg in "$@"; do
    case "$arg" in
        --system)
            export HOTBAR_INCLUDE_SYSTEM_EVENTS=1
            ;;
        --no-system)
            export HOTBAR_INCLUDE_SYSTEM_EVENTS=0
            ;;
        *)
            forward_args+=("$arg")
            ;;
    esac
done

if [[ -x "$SCRIPT_DIR/hotbar" ]]; then
    exec "$SCRIPT_DIR/hotbar" "${forward_args[@]}"
else
    echo "Error: ./hotbar not found. Run 'make build' first."
    exit 1
fi
