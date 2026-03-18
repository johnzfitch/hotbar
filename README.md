# 🔥 HOTBAR

> A GPU-accelerated file history timeline panel for Hyprland, inspired by Hot Wheels Stunt Track Driver (1998)

**Status:** Phase 4 Complete — 179 tests passing | GPU shaders pending

[![Build Status](https://img.shields.io/badge/build-passing-brightgreen)]()
[![Tests](https://img.shields.io/badge/tests-179%20passing-brightgreen)]()
[![License](https://img.shields.io/badge/license-MIT-blue)]()

---

## Vision

This is a **HOT**bar. Not a sidebar. Not a panel. Not a widget.

It's a file history timeline that runs hot — inspired by Hot Wheels Stunt Track Driver (THQ/Mattel, 1998). Files load into a vertical spinner like cars on a selection wheel. Heavy I/O makes the screen edges glow red-hot. Opening the panel triggers a flame burst. The whole thing screams speed because the code behind it IS fast — single-binary Rust, GPU-accelerated particles, zero-copy state management, sub-16ms frame budget.

### What Makes It Different

- **Sub-10ms latency** — File changes appear in the spinner within 10ms of disk write
- **GPU-accelerated effects** — Flame particles, chrome surfaces, heat glow, starburst selection (wgpu + custom shaders)
- **Four data sources** — Claude Code events.jsonl, Codex session logs, XBEL recently-used, directory scanning
- **Momentum physics** — Flick the spinner and watch it decelerate naturally
- **Zero allocations per frame** — State is zero-copy, updates are deltas
- **Hot Wheels aesthetic** — Flame red, chrome silver, brushed metal, gradient text

---

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│                 HOTBAR (single Rust binary)              │
│                                                          │
│  ┌────────────────────────────────────────────────────┐  │
│  │              SHARED GPU RUNTIME                    │  │
│  │                                                    │  │
│  │  wgpu::Device ← initialized once, shared everywhere│  │
│  │       ├── Burn wgpu backend (tensor inference)     │  │
│  │       ├── egui-wgpu renderer (UI widgets)          │  │
│  │       └── Custom shaders (flames, chrome, glow)    │  │
│  └────────────────────────────────────────────────────┘  │
│                                                          │
│  ┌──────────────────┐  ┌────────────────────────────┐   │
│  │  DAEMON CORE     │  │  PANEL RENDERER            │   │
│  │                  │  │                            │   │
│  │  • 4 ingesters   │  │  • SCTK layer-shell        │   │
│  │  • FTS5 search   │  │  • egui widgets            │   │
│  │  │  • Plugin sys  │  │  • Keyboard nav            │   │
│  │  • State delta   │  │  • Hot Wheels theme        │   │
│  │  • SQLite store  │  │  • Momentum spinner        │   │
│  │  • IPC server    │  │  • Filter/search/pin       │   │
│  └────────┬─────────┘  └────────────┬───────────────┘   │
│           │                         │                   │
│           └─────────────┬───────────┘                   │
│                         │                               │
│                 Arc<RwLock<HotState>>                   │
└──────────────────────────────────────────────────────────┘
```

### Data Flow

1. **Ingestion** — Four parallel cursors read from:
   - `~/.claude-code/events.jsonl` (Claude Code file events)
   - `~/.codex/sessions/YYYY/MM/DD/*.jsonl` (Codex apply_patch events)
   - `~/.local/share/recently-used.xbel` (GNOME recently-used)
   - Directory scanner (inotify fallback for user-initiated changes)

2. **State Management** — Events → `HotState::apply_events()` → Delta → SQLite + FTS5 index

3. **Rendering** — Panel polls `Arc<RwLock<HotState>>` each frame (16ms budget) → egui widgets → wgpu render passes

4. **GPU Effects** — Custom wgpu pipelines composite over egui output:
   - `flames.wgsl` — Particle system (Burn tensor compute)
   - `chrome.wgsl` — Brushed metal background
   - `heat_glow.wgsl` — Edge glow (activity-driven hue)
   - `starburst.wgsl` — Selection explosion effect

---

## Current Status

| Phase | Status | Tests | Description |
|-------|--------|-------|-------------|
| **1: Foundation** | ✅ Complete | 27 | Types, protocol, schema, database |
| **2: Daemon Core** | ✅ Complete | 117 | 4 ingest sources, state delta, IPC server |
| **3: Inference + Search** | ✅ Complete | - | FTS5 search, ollama inference, plugin system, GPU device |
| **4: Panel UI** | ✅ Complete | 35 | SCTK shell, all egui widgets, keyboard nav |
| **5: Integration** | ⏳ Next | - | Wire daemon to panel, inotify watchers |
| **6: Polish** | ⏳ Pending | - | Performance tuning, bartender integration |

**GPU Shaders:** Delegated to GPU specialist (flames, chrome, heat_glow, starburst)

**Total:** 179 tests passing | 0 warnings | 0 errors

---

## Technology Stack

| Layer | Technology | Version | Purpose |
|-------|-----------|---------|---------|
| **Language** | Rust | Edition 2024 | Zero-cost abstractions, memory safety |
| **GUI Framework** | egui | 0.31 | Immediate-mode UI widgets |
| **Wayland** | smithay-client-toolkit | 0.19 | Layer-shell surface management |
| **Graphics** | wgpu | 24 | Vulkan/GL backend for GPU shaders |
| **Database** | rusqlite | 0.32 | SQLite with FTS5 full-text search |
| **Async Runtime** | tokio | 1.x | Multi-source ingestion, IPC server |
| **ML Inference** | Burn | - | ONNX model loader (Qwen2.5-Coder) |
| **Serialization** | serde + serde_json | 1.x | IPC protocol, config files |
| **Error Handling** | thiserror | 2.x | Typed error variants |

---

## Building

### Prerequisites

```bash
# Arch Linux / Hyprland
sudo pacman -S rustup sqlite wayland vulkan-icd-loader

# Initialize Rust toolchain
rustup default stable
rustup component add clippy

# For GPU shader development
sudo pacman -S shaderc
```

### Build

```bash
# Clone
git clone https://github.com/yourusername/hotbar.git
cd hotbar

# Build (release)
cargo build --release

# Run tests
cargo test --workspace

# Run clippy
cargo clippy --all-targets -- -D warnings

# Binary output
./target/release/hotbar
```

### Development Build

```bash
# Fast incremental builds
cargo check

# Watch mode (requires cargo-watch)
cargo install cargo-watch
cargo watch -x check -x test
```

---

## Project Structure

```
hotbar/
├── crates/
│   ├── hotbar-common/       # Shared types & protocol
│   │   ├── types.rs         # HotFile, Source, Action, Filter
│   │   ├── protocol.rs      # Command/Response IPC
│   │   ├── schema.rs        # SQL DDL, migrations
│   │   └── trace_db.rs      # SQLite tracing layer (spans + events)
│   │
│   ├── hotbar-daemon/       # Background daemon
│   │   ├── db.rs            # SQLite + FTS5
│   │   ├── state.rs         # HotState + ActivityTracker
│   │   ├── ipc.rs           # Unix socket IPC server
│   │   ├── search.rs        # FTS5 full-text search
│   │   ├── inference.rs     # LLM summarization (Burn/ollama)
│   │   ├── plugin.rs        # Plugin discovery + invocation
│   │   └── ingest/
│   │       ├── claude.rs    # Claude Code events.jsonl parser
│   │       ├── codex.rs     # Codex session JSONL parser
│   │       ├── xbel.rs      # XBEL recently-used parser
│   │       └── dirscan.rs   # Directory scanner
│   │
│   └── hotbar-panel/        # Wayland panel renderer
│       ├── gpu.rs           # Shared wgpu device
│       ├── sctk_shell.rs    # Layer-shell + event loop
│       ├── theme.rs         # Hot Wheels color tokens
│       ├── keybinds.rs      # Keyboard navigation
│       ├── app.rs           # Main UI coordinator
│       └── widgets/
│           ├── spinner.rs   # Momentum file spinner
│           ├── pit_stop.rs  # Pinned files shelf
│           ├── search_bar.rs # Debounced search
│           ├── context_menu.rs # Right-click menu
│           ├── toast.rs     # Toast notifications
│           ├── summary.rs   # Summary popover
│           ├── filter_bar.rs # Filter chips
│           └── logo.rs      # HOTBAR wordmark
│
├── tools/
│   ├── trace-viewer.sh      # Launcher script
│   └── trace-viewer.py      # DeltaGraph-inspired trace viewer (htmx)
│
├── CLAUDE.md                # Multi-agent development plan
├── README.md                # This file
└── Cargo.toml               # Workspace manifest
```

---

## Development Guide

### Running Tests

```bash
# All tests
cargo test --workspace

# Specific crate
cargo test -p hotbar-daemon
cargo test -p hotbar-panel

# Specific test
cargo test --test integration_test

# With output
cargo test -- --nocapture
```

### Coding Standards

1. **No `.unwrap()` outside `#[cfg(test)]`** — Use `thiserror` for error types
2. **All public items need `///` doc comments**
3. **Use `tracing::debug!` / `tracing::info!`** — Never `println!`
4. **Edition 2024 features** — Use let-chains for nested conditions
5. **Zero-copy where possible** — Pass `&[T]`, return `Vec<&T>` for filters
6. **Test edge cases** — Multi-session timestamps, FD leaks, cursor persistence

### Phase Gates (Pre-Merge Checklist)

```bash
# All three must pass
cargo check --workspace
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

### Critical Edge Cases (Ported from v1 TypeScript)

1. **Multi-session timestamp bug** — events.jsonl accumulates across Claude Code sessions. Detect session boundaries (60s timestamp decrease), compute per-session baseTime anchored to file mtime.

2. **Created action preservation** — First `Write()` = "created", subsequent `Edit()` = "modified". Use `createdPaths` HashSet to preserve "created" across session boundaries.

3. **FD exhaustion** — Close all directory handles. v1 leaked 166 FDs. Rust `ReadDir` auto-drops, but verify in integration tests.

4. **Sandbox path filtering** — events.jsonl contains `/test/`, `/home/user/` sandbox paths. Only accept paths under `$HOME`.

5. **Agent timestamp tolerance** — Dir scan uses 5s tolerance (`agent_ts >= mtime - 5s`) to account for baseTime drift in relative timestamps.

---

## Trace Viewer

Hotbar includes a built-in trace viewer for profiling and debugging both the daemon and panel. All `tracing` spans and events are written to a shared SQLite database at `~/.local/share/hotbar/traces.db` via a custom `tracing_subscriber::Layer`.

### Usage

```bash
# Launch the viewer (opens browser automatically)
./tools/trace-viewer.sh

# Or with options
python tools/trace-viewer.py --port 8777 --db path/to/traces.db
```

Then open `http://localhost:8777` in your browser.

### Views

| View | Style | What It Shows |
|------|-------|---------------|
| **Timeline** | DeltaGraph | Hierarchical span tree with colored duration bars |
| **Events** | DeltaGraph Notebook | Filterable log with level badges (DEBUG/INFO/WARN/ERROR) |
| **Performance** | DeltaGraph Bar Chart | Latency percentiles (P50/P90/P95/P99) + duration histogram |
| **Top Spans** | DeltaGraph Notebook | Slowest 100 spans ranked by duration |
| **Heatmap** | Lotus 1-2-3 | Spreadsheet grid (A-Z columns, numbered rows) with heat-colored cells |
| **Trend** | Lotus Line Chart | Frame-by-frame sparkline with 16ms budget threshold line |
| **Pie** | Harvard Graphics | Conic-gradient pie chart with 3D shadow + proportional stacked bar |
| **Waterfall** | Harvard Graphics Cascade | Gantt-like timeline showing span positions and nesting depth |

### How Tracing Works

Both `hotbar` (panel) and `hotbar-daemon` register a `trace_db::SqliteLayer` on startup:

```rust
let sqlite_layer = trace_db::init("panel")?;
tracing_subscriber::registry()
    .with(env_filter)
    .with(fmt_layer)
    .with(sqlite_layer)
    .init();
```

The layer captures every `tracing::debug_span!`, `tracing::info!`, etc. into three tables:

- **`sessions`** — one row per process startup (pid, component, start time)
- **`spans`** — one row per span close (name, target, level, start/end timestamps, fields)
- **`events`** — one row per tracing event (level, target, message, timestamp)

Data is batched (64 entries per flush), WAL-mode for concurrent access, and auto-pruned (>30 days) on startup.

---

## Configuration

Config file: `$XDG_CONFIG_HOME/hotbar/config.toml`

```toml
[theme]
corner_radius = 6
panel_width = 420
panel_margin = 8

[keybinds]
rotate_up = "k"
rotate_down = "j"
open = "Enter"
search = "/"
pin = "p"

[inference]
backend = "ollama"  # "burn" | "ollama" | "none"
model = "qwen2.5-coder:1.5b"
ollama_url = "http://localhost:11434"
timeout_secs = 30

[plugins]
dir = "$XDG_CONFIG_HOME/hotbar/plugins"
timeout_ms = 5000
```

---

## License

MIT License — see [LICENSE](LICENSE) for details.

---

## Credits

Inspired by:
- **Hot Wheels Stunt Track Driver** (THQ/Mattel, 1998) — Visual aesthetic, spinner UI concept
- **Raycast** — Command palette UX patterns
- **Linear** — Polish, attention to detail
- **egui** — Immediate-mode GUI paradigm

Built with ❤️ and 🔥 in Rust.
