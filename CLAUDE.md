
## Project Status

**Current Phase:** 4 (Panel UI) — ✅ **COMPLETE**

| Phase | Status | Tests | Notes |
|-------|--------|-------|-------|
| 1: Foundation | ✅ Complete | 27 | Types, protocol, schema, database |
| 2: Daemon Core | ✅ Complete | 117 | All 4 ingest sources, state, IPC |
| 3: Inference + Search + Plugins | ✅ Complete | - | FTS5, ollama fallback, plugin system, GPU device |
| 4: Panel UI (egui + SCTK) | ✅ Complete | 35 | All widgets implemented, SCTK shell wired |
| 5: Integration + Wiring | ⏳ Pending | - | Wire daemon to panel, inotify watchers |
| 6: Polish + Shipping | ⏳ Pending | - | Performance, bartender integration, v2.0.0 tag |

**GPU Shaders:** Delegated to GPU Specialist (flames, chrome, heat_glow, starburst)

**Total Tests:** 179 passing (27 common + 117 daemon + 35 panel)
**Gates:** `cargo check ✓` `cargo clippy -D warnings ✓` `cargo test ✓`

---

## Multi-Agent Orchestration

### Master Architecture

```
┌───────────────────────────────────────────────────┐
│              YOU — Master Terminal                  │
│              Claude Opus (terminal control)        │
│                                                    │
│  • Launches orchestrator per phase                 │
│  • Reviews phase gates                             │
│  • Writes GPU shaders (flames, chrome, heat, FX)   │
│  • Accepts/rejects milestones                      │
└────────────────────────┬──────────────────────────┘
                         │
┌────────────────────────▼──────────────────────────┐
│              ORCHESTRATOR (Opus)                    │
│                                                    │
│  • Reads THIS plan as source of truth              │
│  • Breaks phases into task tickets                 │
│  • Routes tasks to parallel Sonnet workers         │
│  • Tracks completion in markdown checklist          │
│  • Runs review loop per task                       │
│  • Escalates blockers after 3 retries              │
│  • Never modifies files directly                   │
└────────────────────────┬──────────────────────────┘
                         │
       ┌─────────────────┼───────────────────┐
       │                 │                   │
       ▼                 ▼                   ▼
┌─────────────┐  ┌──────────────┐  ┌──────────────────┐
│  PARALLEL   │  │   REVIEWER   │  │    ESCALATION    │
│  WORKERS    │  │   (Sonnet)   │  │    (Sonnet)      │
│  (Sonnet)   │  │              │  │                  │
│             │  │ • cargo check│  │ • SCTK surface   │
│ 1│2│3│4│5│6 │  │ • clippy     │  │ • Burn↔wgpu     │
│             │  │ • cargo test │  │ • Complex arch   │
│ Each owns   │  │ • API compat │  │ • Shader issues  │
│ one module  │  │ • Doc check  │  │                  │
│             │  │ • Edge cases │  │ → Escalates to   │
│             │  │              │  │   YOU if stuck    │
└──────┬──────┘  └──────┬───────┘  └────────┬─────────┘
       │                │                   │
       └────────────────┼───────────────────┘
                        │
       ┌────────────────▼───────────────────┐
       │          FEEDBACK LOOP              │
       │                                     │
       │  cargo check ──── FAIL ──→ Re-prompt│
       │      │                              │
       │     PASS                            │
       │      │                              │
       │  cargo clippy ── FAIL ──→ Re-prompt │
       │      │                              │
       │     PASS                            │
       │      │                              │
       │  cargo test ──── FAIL ──→ Re-prompt │
       │      │                              │
       │     PASS                            │
       │      │                              │
       │  ✅ ACCEPTED                        │
       │                                     │
       │  Max 3 retries → ESCALATE           │
       └─────────────────────────────────────┘
```

### Orchestrator Rules

1. Execute phases sequentially: 1 → 2 → 3 → 4 → 5 → 6. Parallelize tasks within each phase.
2. Every task passes the reviewer BEFORE the phase gate unlocks.
3. Max 3 retry loops per task. After 3 failures: escalate to YOU with the specific error, what was tried, and a suggested approach.
4. Workers MUST NOT modify files outside their assigned module. If they need a type from `hotbar-common`, they REQUEST it — they don't add it.
5. Reviewer runs all three gates: `cargo check --workspace`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`.
6. Track state in a markdown checklist. Update after each task.
7. The CLAUDE.md file in the repo root is the agent's secondary source of truth for edge cases and gotchas discovered during development.

### Worker Rules

1. Only modify files in your assigned module.
2. `thiserror` for error types. `anyhow` only in `main.rs`.
3. No `.unwrap()` outside `#[cfg(test)]` blocks.
4. Every public function gets a `///` doc comment.
5. Write tests for all non-trivial logic. Test edge cases from CLAUDE.md.
6. Use `tracing::debug!` / `tracing::info!` for logging, never `println!`.
7. Reference the existing TypeScript in `services/hotfiles.ts` for parsing edge cases — rewrite idiomatically in Rust, don't transliterate.

### Reviewer Checklist

```
□ cargo check --workspace
□ cargo clippy --all-targets -- -D warnings  
□ cargo test --workspace
□ No .unwrap() outside #[cfg(test)]
□ All public items have /// doc comments
□ Error types use thiserror
□ No hardcoded paths (use $HOME, XDG, or config)
□ No secrets/env vars in logs
□ Types from hotbar-common used correctly (no redefinitions)
□ Edge cases from CLAUDE.md are tested
```

On failure: report specific check, file, line, suggested fix. Return to worker. Do NOT fix it yourself.

---

## Phase 1: Foundation

**Goal:** Shared types compile. Schema initializes. IPC protocol round-trips.

**Parallel Tasks:**

| Task | Worker | Module | Description |
|------|--------|--------|-------------|
| 1A | W1 | `hotbar-common/types.rs` | `HotFile`, `Source`, `Action`, `Filter`, `ActionFilter`, `Pin`, `Summary`, `Preference`. All with `Serialize, Deserialize, Clone, Debug, PartialEq`. `Source` and `Action` are `#[serde(rename_all = "lowercase")]` enums. Include `ActivityLevel` as `f32` wrapper. |
| 1B | W2 | `hotbar-common/protocol.rs` | `Command` enum (Toggle, Quit, SetFilter, Pin, Unpin, Summarize, Search, GetState). `Response` enum (State, Ok, Error). `Delta` struct: `added: Vec<HotFile>`, `updated: Vec<HotFile>`, `removed: Vec<String>`, `activity_level: f32`. JSON-lines ser/de with `serde_json::to_string` + `\n` delimiter. |
| 1C | W3 | `hotbar-common/schema.rs` | All `CREATE TABLE/INDEX` SQL as `const &str`. `fn init_db(conn: &Connection) -> Result<()>` that creates tables idempotently (`IF NOT EXISTS`). `meta` table with `schema_version INTEGER`. Migration runner checks version and applies incremental DDL. |
| 1D | W4 | `hotbar-daemon/db.rs` | `struct Db { conn: Connection }`. Methods: `open(path) -> Result<Self>`, `insert_events(events: &[FileEvent])`, `upsert_pin(pin: &Pin)`, `remove_pin(path: &str)`, `get_pins() -> Vec<Pin>`, `get_events(filter: Filter, limit: usize) -> Vec<HotFile>`, `upsert_summary(path, content, model)`, `get_summary(path) -> Option<Summary>`, `set_preference(key, value_json)`, `get_preference(key) -> Option<String>`. All via `thiserror` error type `DbError`. |

**Reviewer Checks:**
- Protocol round-trips: every `Command` variant → `serde_json::to_string` → `serde_json::from_str` → assert_eq
- DB methods tested with `:memory:` SQLite
- Schema migration applies cleanly on fresh DB
- Types derive all required traits

**Phase Gate:** `cargo test --workspace` passes. All 4 tasks accepted.

---

## Phase 2: Daemon Core

**Goal:** Daemon ingests from all 4 sources. State computes deltas. Background dir scan runs.

**Parallel Tasks:**

| Task | Worker | Module | Description |
|------|--------|--------|-------------|
| 2A | W1 | `ingest/claude.rs` | **Cursor-based** events.jsonl parser. `struct ClaudeCursor { path: PathBuf, last_offset: u64, last_inode: u64, session_base_time: i64 }`. `fn read_new(&mut self) -> Result<Vec<FileEvent>>` — seeks to `last_offset`, reads new bytes, parses lines, detects session boundaries (timestamp decrease >60s), computes per-session baseTime anchored to fileMtime. Returns ONLY new events. On inode change (file rotation): full re-read, reset cursor. Port logic from `hotfiles.ts` lines 672-912 — preserve session boundary detection, `createdPaths` set, sandbox path filtering. |
| 2B | W2 | `ingest/codex.rs` | Codex session JSONL parser. Scans `$HOME/.codex/sessions/{YYYY}/{MM}/{DD}/*.jsonl` for today + yesterday. Parses `apply_patch` events: `custom_tool_call` or `function_call` with `name: "apply_patch"`. Extracts paths from `*** Update/Add/Delete File:` regex in patch text. Absolute ISO 8601 timestamps. Cursor-based per session file. Cap at 20 session files, mtime-sorted. |
| 2C | W3 | `ingest/xbel.rs` | XBEL parser. Reads `~/.local/share/recently-used.xbel`. XML-lite parsing (regex on `<bookmark` blocks — don't pull a full XML parser). Extract `href` (file URI), `visited` (ISO timestamp), `mime:mime-type`. Filter: 24h window, code/text MIME types, under `$HOME`, skip dist/build/node_modules/minified. Fallback to extension-based MIME detection for entries with missing/incorrect MIME. |
| 2D | W4 | `ingest/dirscan.rs` | Background directory scanner. Input: `HashSet<PathBuf>` of active dirs from other sources. Scans for code files modified in last 24h not covered by agent events (5s timestamp tolerance). Birthtime-based create detection: `birthtime > cutoff && (mtime - birthtime) < 120s`. Skip `SKIP_DIRS` (node_modules, .git, .venv, __pycache__, target, dist, build). Skip `SKIP_FILES` (lockfiles, minified). **Close all directory handles** (this was an FD leak bug in v1). |
| 2E | W5 | `state.rs` | `struct HotState { files: Vec<HotFile>, by_path: HashMap<String, usize>, pins: Vec<Pin>, activity: ActivityTracker }`. `ActivityTracker`: ring buffer of `(timestamp, event_count)` tuples, 10-second window, computes `events_per_second() -> f32`. `fn apply_events(&mut self, events: Vec<FileEvent>) -> Delta` — merges into files vec, deduplicates by path (keep most recent), computes diff (added/updated/removed). `fn apply_filter(&self, source: Filter, action: ActionFilter) -> Vec<&HotFile>`. `fn hydrate_from_db(&mut self, db: &Db)` — loads on startup. |
| 2F | W6 | `ipc.rs` | Tokio Unix socket server at `$XDG_RUNTIME_DIR/hotbar.sock`. Accept connections, read JSON-lines, dispatch `Command` variants. `Toggle` → sends `crossbeam` channel message to panel. `GetState` → serializes current `HotState` snapshot. `SetFilter/Pin/Unpin/Search` → modify state + DB. Graceful shutdown on drop. |

**Reviewer Checks:**
- Each ingest module has integration test with fixture data in `tests/fixtures/`
- Claude cursor: call `read_new()` twice, second returns only new events
- Codex: fixture JSONL with `apply_patch` events, verify path extraction
- XBEL: fixture with various MIME types, verify filtering
- Dirscan: create temp dir with files, verify detection + create vs modified
- State delta: insert 5 events → delta has 5 added. Update 2 → delta has 2 updated. Remove 1 → delta has 1 removed.
- ActivityTracker: push 10 events in 1 second → `events_per_second()` ≈ 10.0
- IPC: start server, connect with tokio `UnixStream`, send `GetState`, verify response
- **No FD leaks** — all directory handles dropped/closed

**Phase Gate:** Daemon logic fully functional as a library crate. Integration test: write line to fixture events.jsonl → `ClaudeCursor::read_new()` returns correct event → `HotState::apply_events()` returns correct delta.

---

## Phase 3: Inference + Search + Plugins

**Parallel Tasks:**

| Task | Worker | Module | Description |
|------|--------|--------|-------------|
| 3A | W1 | `inference.rs` | Burn ONNX model loader. Load quantized summarization model (Qwen2.5-Coder 1.5B or Phi-3-mini — configurable). System prompt: "Summarize this source file in 2-3 sentences. Focus on purpose, key abstractions, and dependencies." Read file content, run inference, cache in `summaries` table. **Fallback:** if Burn model fails or is unconfigured, fall back to `reqwest` call to ollama `localhost:11434`. Timeout: 30s. Config key: `inference.backend = "burn" | "ollama" | "none"`. |
| 3B | W2 | `search.rs` | FTS5 integration. `fn index_file(db: &Db, path: &str, filename: &str, summary: Option<&str>)` — upsert into `search_index`. `fn search(db: &Db, query: &str, limit: usize) -> Vec<HotFile>` — FTS5 `MATCH` + `bm25()` ranking, join back to `file_events` for full metadata. Handle empty query (return all recent). Handle special chars in query (escape FTS5 syntax). |
| 3C | W3 | `plugin.rs` | Plugin hooks. Plugins are executables in `$XDG_CONFIG_HOME/hotbar/plugins/`. Discovery: scan dir on startup, read optional `plugin.toml` manifest (name, triggers, description). Invocation: `tokio::process::Command`, JSON on stdin, JSON response on stdout. Timeout: 5s (configurable). Triggers: `on_file_change`, `on_pin`, `manual`. Stderr logged via `tracing::warn!`, doesn't crash daemon. |
| 3D | W4 | `gpu/device.rs` | **Shared wgpu device initialization.** `fn create_shared_device() -> (wgpu::Device, wgpu::Queue, wgpu::Instance)`. Initialize wgpu with Vulkan backend (primary) + GL fallback. This device is passed to: Burn `WgpuDevice::from(device)`, `egui_wgpu::Renderer::new(device)`, and all custom shader pipelines. Single init, shared everywhere. **This is the keystone module.** |

**Reviewer Checks:**
- Inference: test with ollama fallback (mock HTTP server or skip if ollama unavailable)
- Search: index 10 files, query returns ranked results, empty query returns all
- Plugins: fixture script that echoes input, verify invocation + timeout kill
- GPU device: initializes without panic on headless (test with `WGPU_BACKEND=gl`)
- All modules handle errors gracefully (no panics on missing resources)

**Phase Gate:** Search works end-to-end. Plugin hook invokes test script. GPU device initializes.

---

## Phase 4: Panel UI (egui + SCTK) — ✅ COMPLETE

**Status:** All egui widgets implemented and tested (35 tests). SCTK layer-shell integrated. GPU shaders delegated to GPU Specialist.

```
┌───────────────────────────────────────────┐
│         GPU SPECIALIST (parallel)          │
│                                            │
│  Writes GPU shaders + Rust pipelines:      │
│  • flames.wgsl + flames.rs                 │
│  • chrome.wgsl + chrome.rs                 │
│  • heat_glow.wgsl + heat_glow.rs           │
│  • starburst.wgsl + starburst.rs           │
└─────────────────────┬─────────────────────┘
                      │
┌─────────────────────▼─────────────────────┐
│           ORCHESTRATOR (Opus)              │
│         Completed all tasks ✓              │
└─────────────────────┬─────────────────────┘
                      │
      ┌───────────────┴───────────────────┐
      │               │                   │
      ▼               ▼                   ▼
┌───────────┐  ┌────────────┐  ┌──────────────────┐
│  WORKERS  │  │  REVIEWER  │  │  INTEGRATION     │
│  (Sonnet) │  │  (Sonnet)  │  │                  │
│           │  │            │  │  All tasks ✓     │
│ 4A-4F ✓   │  │ All gates  │  │  179 tests pass  │
│           │  │ passed ✓   │  │  0 warnings      │
└───────────┘  └────────────┘  └──────────────────┘
```

**Parallel Tasks (Sonnet workers):**

| Task | Worker | Module | Description |
|------|--------|--------|-------------|
| 4A | W1 | `sctk_shell.rs` + `main.rs` | SCTK layer-shell surface creation. `namespace="hotbar"`, anchor `TOP|RIGHT|BOTTOM`, margin 8px. Bridge SCTK input events (pointer, keyboard, scroll) to egui `RawInput`. Create wgpu surface on the SCTK `wl_surface`. Main loop: poll SCTK events → update egui input → run egui frame → submit wgpu render. |
| 4B | W2 | `widgets/spinner.rs` | Custom `egui::Widget`. Files arranged radially. `allocate_painter()` for custom drawing. Scroll → rotation. Drag → rotation. Momentum: `angular_velocity *= 0.95` per frame. Selected file at top (0°). Each slot: colored circle (source tint) + file icon + truncated name via `painter.text()`. Selected slot: larger, highlight ring. Showcase area: full metadata for selected file (name, full path, time, source badge, action). |
| 4C | W3 | `widgets/pit_stop.rs` + `widgets/search_bar.rs` | Pit stop: horizontal `egui::ScrollArea` of pinned files. Each pin is an `egui::Frame` with chrome styling. Drag handle for reorder (egui drag-and-drop). Click to open file. Search: `egui::TextEdit::singleline` with placeholder "Search files...". Debounce 200ms. On input: call `search::search()`, replace spinner contents with results. On clear/escape: restore normal file list. |
| 4D | W4 | `widgets/context_menu.rs` + `widgets/toast.rs` + `widgets/summary.rs` | Context menu: `egui::menu::context_menu` on right-click of any file entry. Items: Open, Open Folder, Copy Path, Pin/Unpin, Summarize. Toast: `egui::Window` anchored bottom-right, auto-dismiss after 3s, queue of messages. Summary: `egui::Window` popover near selected spinner slot, shows loading spinner then summary text. |
| 4E | W5 | `theme.rs` + `widgets/logo.rs` | Hot Wheels color tokens as `egui::Color32` constants. Font loading: Berkeley Mono (body), Barlow Condensed (badges), Racing Sans One (logo). `egui::Style` overrides: dark visuals, custom button rounding, chrome-colored focus rings. Logo: render "HOTBAR" as styled `egui::RichText` or custom paint with flame accent shapes. |
| 4F | W6 | `keybinds.rs` + `widgets/filter_bar.rs` | Keyboard: `j/k` rotate spinner, `Enter` open, `Shift+Enter` open folder, `/` focus search, `p` pin/unpin, `Escape` close panel or clear search, `1-5` switch source filters. Filter bar: row of `egui::Button` styled as chips. Active chip gets custom underline paint (flame color). |

**GPU Specialist Tasks (delegated):**

| Module | Description | Status |
|--------|-------------|--------|
| `gpu/flames.rs` + `shaders/flames.wgsl` | Particle system. Burn tensor `[N, 6]` for particle state. Compute pass updates positions/velocities/heat. Render pass draws textured quads at particle positions with heat-based color (yellow → orange → red → fade). | ⏳ Pending |
| `gpu/chrome.rs` + `shaders/chrome.wgsl` | Brushed metal background. Fragment shader with noise-based anisotropic highlights simulating directional brushing. Subtle. | ⏳ Pending |
| `gpu/heat_glow.rs` + `shaders/heat_glow.wgsl` | Edge glow. Radial gradient from panel edges, hue interpolated by `activity_level` uniform (0.0 → blue, 0.5 → orange, 1.0 → red). Border width increases with intensity. | ⏳ Pending |
| `gpu/starburst.rs` + `shaders/starburst.wgsl` | Selection explosion. Emanating lines + additive glow at the selected spinner slot position. Triggered on selection change, decays over ~0.3s. | ⏳ Pending |

> **Note:** GPU shaders use the shared wgpu device from `gpu.rs` (Phase 3, task 3D). All render passes composite over the egui output.

**Reviewer Checks:** ✅ All passed
- ✓ Panel compiles and launches (headless compatible)
- ✓ Spinner rotation: scroll events change angle, momentum decelerates
- ✓ Spinner selection: click on slot selects it, metadata updates
- ✓ Pit stop: pins render, drag reorder works
- ✓ Search: type query → results appear, clear → normal list
- ✓ Context menu: right-click shows menu, items dispatch correct actions
- ✓ Toast: trigger copy → toast appears, auto-dismisses
- ✓ Keyboard: j/k rotates, Enter opens, / focuses search
- ✓ Filter chips: click changes filter, active chip visually distinct
- ✓ All egui widgets use theme colors (no hardcoded colors in widget code)

**Phase Gate:** ✅ **PASSED**
- All 35 panel tests passing
- 0 compiler warnings
- 0 clippy warnings
- SCTK layer-shell integrated
- All widgets functional (pending GPU shaders for visual effects)

**Implementation Notes (Phase 4):**

Key compatibility issues resolved:
1. **egui 0.31 breaking changes**:
   - `Rounding` → `CornerRadius` (with `u8` radius parameter)
   - `WidgetVisuals.rounding` → `.corner_radius`
   - `Visuals.window_rounding` → `.window_corner_radius`
   - New `StrokeKind` parameter on `rect_stroke()`/`rect()` — use `StrokeKind::Outside`

2. **wgpu 24 + egui-wgpu 0.31 lifetime**:
   - `begin_render_pass` returns `RenderPass<'encoder>` but egui-wgpu expects `RenderPass<'static>`
   - Bridge: `render_pass.forget_lifetime()` — wgpu 24 method for this exact use case

3. **SCTK 0.19 calloop version**:
   - SCTK re-exports calloop 0.13 (not 0.14)
   - Always use `smithay_client_toolkit::reexports::calloop`
   - WaylandSource callback returns `Result<usize, DispatchError>`

4. **Module structure**:
   ```
   hotbar-panel/src/
   ├── gpu.rs              # SharedGpu (from Phase 3)
   ├── sctk_shell.rs       # Wayland layer-shell + event loop
   ├── theme.rs            # Hot Wheels color tokens
   ├── keybinds.rs         # Keyboard navigation
   ├── app.rs              # Main app state coordinator
   ├── widgets/
   │   ├── spinner.rs      # File spinner with momentum
   │   ├── pit_stop.rs     # Pinned files shelf
   │   ├── search_bar.rs   # Debounced search input
   │   ├── context_menu.rs # Right-click menu
   │   ├── toast.rs        # Toast notifications
   │   ├── summary.rs      # Summary popover
   │   ├── filter_bar.rs   # Source/action filter chips
   │   └── logo.rs         # HOTBAR wordmark
   └── lib.rs
   ```

---

## Phase 5: Integration + Wiring

**Sequential — Orchestrator drives.**

| Step | Description |
|------|-------------|
| 5A | Wire daemon tasks to panel. `main.rs`: spawn tokio tasks for each ingest source + inotify watchers. Ingest outputs feed into `Arc<RwLock<HotState>>`. Panel reads state each frame. Activity tracker updates per-event. |
| 5B | End-to-end test: start hotbar → touch a file in a Claude Code project → file appears in spinner within 16ms frame budget. |
| 5C | Wire external IPC: bartender sends `toggle` → panel visibility toggles. CLI `hotbar-ctl search "types"` returns results. |
| 5D | Wire inference: alt-click file → read file → Burn inference (or ollama fallback) → cache → display in popover. |
| 5E | Wire search: type in search bar → FTS5 query → results replace spinner. |
| 5F | Config file: `$XDG_CONFIG_HOME/hotbar/config.toml`. Keys: `theme`, `keybinds`, `inference.backend`, `inference.model`, `inference.ollama_url`, `plugin_dir`, `socket_path`. |
| 5G | Systemd: `hotbar.service` (user unit). `ExecStart=hotbar`. `Restart=on-failure`. Hyprland `exec-once = hotbar`. |

---

## Phase 6: Polish + Shipping

| Step | Description |
|------|-------------|
| 6A | Performance profiling. `cargo flamegraph`. Identify any frame drops. Target: 60fps constant with flames active. |
| 6B | Memory profiling. `heaptrack` or `valgrind --tool=massif`. Target: <50MB total. |
| 6C | Bartender integration: add hotbar toggle button to bartender. Match the Hot Wheels aesthetic — the button should have a flame icon. |
| 6D | README.md with screenshots, install instructions, configuration reference, architecture diagram. |
| 6E | `CLAUDE.md` updated with all edge cases, gotchas, and patterns discovered during development. |
| 6F | Tag v2.0.0. Ship it. 🔥 |

---

## Critical Edge Cases to Preserve from v1

These bugs were found and fixed in the TypeScript codebase. Workers MUST port these fixes:

1. **Multi-session timestamp bug**: events.jsonl accumulates across sessions. Timestamps are relative to each session's start. Detect session boundaries where timestamps decrease by >60s. Compute per-session baseTime. Without this: old session events get future timestamps and push out real files.

2. **Created action preservation**: First `Write()` for a path = "created". Subsequent `Edit()` overwrites to "modified". Fix: `createdPaths` HashSet tracks first-write paths. Post-processing restores "created" action.

3. **FD exhaustion**: Directory scanning opens enumerators. MUST close/drop all handles. v1 leaked 166 FDs and broke icon loading. In Rust: `ReadDir` iterators drop automatically, but verify with `ls /proc/self/fd | wc -l` in integration test.

4. **Sandbox path pollution**: events.jsonl contains `/test/`, `/home/user/` sandbox paths. Filter: only accept paths starting with `$HOME`.

5. **Icon theme crash**: Forcing icon theme to Adwaita prevented GTK4 infinite recursion with Yaru/Cosmic. In egui: not applicable (no GTK), but load icons explicitly.

6. **Agent timestamp tolerance**: Dir scan skips files where `agent_timestamp >= mtime - 5s`. The 5s tolerance accounts for baseTime drift in Claude's relative timestamp math.

---

## Success Criteria

The project is complete when:

1. **Single binary** `hotbar` runs on Hyprland as layer-shell panel
2. **< 10ms** from file change to state update (inotify → parse → state delta)  
3. **< 16ms** frame time (60fps with flames active)
4. **Spinner rotates** with momentum physics — flick and decelerate
5. **Heat system visible** — rapid writes glow the edges progressively hotter
6. **Flame edge on panel open** — the slide-in is dramatic
7. **All features work**: filter, search, pin, context menu, alt-click summary, keyboard nav, toast
8. **`cargo test --workspace` passes** with >80% coverage on daemon logic
9. **Binary < 20MB** (single binary with Burn model runtime)
10. **Memory < 50MB** total (daemon + panel + GPU buffers)
11. **Nobody says "oh, another dark mode panel"** — it feels like Hot Wheels made a file manager
12. **"HOTBAR" wordmark has flame energy** — it burns
