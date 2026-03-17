# Hotbar - File history timeline for Hyprland

.PHONY: build run dev debug clean help

help:
	@echo "Hotbar build targets:"
	@echo "  make build   Bundle app.tsx -> ./hotbar"
	@echo "  make run     Execute hotbar"
	@echo "  make dev     Build and run"
	@echo "  make debug   Build and run with debug logging"
	@echo "  make clean   Remove build artifacts"
	@echo ""
	@echo "Debug flags (pass to run.sh or hotbar directly):"
	@echo "  --debug, -d          Print debug logs to console"
	@echo "  --file, -f           Write debug logs to ~/.cache/hotbar/debug.log"
	@echo "  --file=PATH, -f=PATH Write debug logs to custom path"
	@echo "  --system             Include system events (~/.codex, ~/.claude) via wrapper"
	@echo "  --no-system          Force-hide system events via wrapper"
	@echo ""
	@echo "Examples:"
	@echo "  ./run.sh --debug"
	@echo "  ./run.sh --debug --file=/tmp/hotbar.log"
	@echo "  ./run.sh --system --debug"

build:
	./build.sh

run:
	./run.sh

dev: build run

debug: build
	./run.sh --debug

clean:
	rm -f ./hotbar
	@echo "Cleaned build artifacts"
