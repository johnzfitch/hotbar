//! Hotbar — GPU-accelerated file history panel for Hyprland.
//!
//! Single binary that runs the daemon (ingest, state, IPC) and panel (egui + SCTK)
//! in the same process. Daemon tasks run on tokio; the panel runs on the main
//! thread via SCTK's calloop (Wayland requires main-thread rendering).

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
use tokio::sync::{mpsc, Mutex, Notify, RwLock};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use hotbar_common::protocol::Command;
use hotbar_common::HotFile;
use hotbar_daemon::db::Db;
use hotbar_daemon::inference::Summarizer;
use hotbar_daemon::ingest::claude::ClaudeCursor;
use hotbar_daemon::ingest::codex::CodexWatcher;
use hotbar_daemon::ingest::dirscan::DirScanner;
use hotbar_daemon::ingest::xbel::XbelParser;
use hotbar_daemon::ipc::IpcServer;
use hotbar_daemon::plugin::PluginManager;
use hotbar_daemon::search;
use hotbar_daemon::state::HotState;
use hotbar_daemon::watcher::{self, IngestWatcher};
use hotbar_daemon::write_behind::WriteBehindBuffer;
use hotbar_panel::app::{AppAction, HotbarApp};
use hotbar_panel::config::HotbarConfig;
use hotbar_panel::dispatch::{DispatchResult, Dispatcher};
use hotbar_panel::sctk_shell::{HotbarShell, PanelConfig};
use hotbar_panel::widgets::toast::ToastKind;

fn main() -> Result<()> {
    // ── 1. Config ──────────────────────────────────────────
    let config = HotbarConfig::load();

    // ── 2. Tracing ─────────────────────────────────────────
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let fmt_layer = tracing_subscriber::fmt::layer().compact();

    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer);

    // Optionally add SQLite trace layer
    match hotbar_common::trace_db::init("hotbar") {
        Ok(sqlite_layer) => {
            registry.with(sqlite_layer).init();
        }
        Err(e) => {
            registry.init();
            tracing::warn!(error = %e, "trace DB unavailable, continuing without it");
        }
    }

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "hotbar starting"
    );

    // ── 3. Database + State ────────────────────────────────
    let db = Db::open(&config.db_path)
        .context("failed to open database")?;
    let db = Arc::new(Mutex::new(db));

    let mut state = HotState::new();
    {
        let db_guard = db.blocking_lock();
        state
            .hydrate_from_db(&db_guard)
            .context("failed to hydrate state")?;
    }
    let state = Arc::new(RwLock::new(state));

    // ── 4. Shared resources ────────────────────────────────
    let summarizer = Arc::new(Summarizer::new(config.inference.clone()));
    let search_results: Arc<RwLock<Option<Vec<HotFile>>>> = Arc::new(RwLock::new(None));
    let dispatcher = Arc::new(Dispatcher::new(
        Arc::clone(&state),
        Arc::clone(&db),
        Arc::clone(&summarizer),
        Arc::clone(&search_results),
    ));

    // Channel: IPC commands → panel (Toggle, Quit)
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(32);
    // Channel: AppActions → tokio dispatcher
    let (action_tx, action_rx) = mpsc::channel::<Vec<AppAction>>(64);
    // Channel: DispatchResults → panel (toasts, summaries)
    let (result_tx, result_rx) = mpsc::channel::<Vec<DispatchResult>>(64);

    // ── 5. Tokio runtime ───────────────────────────────────
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("hotbar-daemon")
        .build()
        .context("failed to build tokio runtime")?;

    // inotify signals for each ingest source
    let claude_signal = Arc::new(Notify::new());
    let codex_signal = Arc::new(Notify::new());
    let xbel_signal = Arc::new(Notify::new());

    // Discover Claude events.jsonl
    let claude_path = watcher::find_claude_events()
        .unwrap_or(config.claude_events_path.clone());
    let codex_dir = watcher::codex_sessions_dir();
    let xbel_file = watcher::xbel_path();

    // Set up filesystem watchers (best-effort — falls back to polling)
    let _watcher = IngestWatcher::new(
        &claude_path,
        &codex_dir,
        &xbel_file,
        Arc::clone(&claude_signal),
        Arc::clone(&codex_signal),
        Arc::clone(&xbel_signal),
    )
    .ok();

    // ── 6. Spawn daemon tasks ──────────────────────────────
    let _guard = rt.enter();

    // 6a. Claude ingest
    {
        let state = Arc::clone(&state);
        let db = Arc::clone(&db);
        let signal = Arc::clone(&claude_signal);
        let path = claude_path.clone();
        rt.spawn(async move {
            let mut cursor = ClaudeCursor::new(path);
            let mut wb = WriteBehindBuffer::new();
            loop {
                // Wait for inotify signal OR poll timeout
                tokio::select! {
                    _ = signal.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                }

                match cursor.read_new() {
                    Ok(events) if !events.is_empty() => {
                        tracing::debug!(count = events.len(), "claude events ingested");
                        wb.push(&events);
                        let mut st = state.write().await;
                        st.apply_events(events);
                        drop(st);

                        if wb.should_flush() {
                            let db = db.lock().await;
                            if let Err(e) = wb.flush(&db) {
                                tracing::warn!(error = %e, "claude write-behind flush failed");
                            }
                        }
                    }
                    Ok(_) => {} // no new events
                    Err(e) => {
                        tracing::warn!(error = %e, "claude ingest error");
                    }
                }
            }
        });
    }

    // 6b. Codex ingest
    {
        let state = Arc::clone(&state);
        let db = Arc::clone(&db);
        let signal = Arc::clone(&codex_signal);
        rt.spawn(async move {
            let mut watcher = CodexWatcher::new();
            let mut wb = WriteBehindBuffer::new();
            loop {
                tokio::select! {
                    _ = signal.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                }

                match watcher.read_new() {
                    Ok(events) if !events.is_empty() => {
                        tracing::debug!(count = events.len(), "codex events ingested");
                        wb.push(&events);
                        let mut st = state.write().await;
                        st.apply_events(events);
                        drop(st);

                        if wb.should_flush() {
                            let db = db.lock().await;
                            if let Err(e) = wb.flush(&db) {
                                tracing::warn!(error = %e, "codex write-behind flush failed");
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "codex ingest error");
                    }
                }
            }
        });
    }

    // 6c. XBEL ingest
    {
        let state = Arc::clone(&state);
        let db = Arc::clone(&db);
        let signal = Arc::clone(&xbel_signal);
        rt.spawn(async move {
            let parser = XbelParser::new();
            let mut wb = WriteBehindBuffer::new();
            loop {
                tokio::select! {
                    _ = signal.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                }

                match parser.read_new() {
                    Ok(events) if !events.is_empty() => {
                        tracing::debug!(count = events.len(), "xbel events ingested");
                        wb.push(&events);
                        let mut st = state.write().await;
                        st.apply_events(events);
                        drop(st);

                        if wb.should_flush() {
                            let db = db.lock().await;
                            if let Err(e) = wb.flush(&db) {
                                tracing::warn!(error = %e, "xbel write-behind flush failed");
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "xbel ingest error");
                    }
                }
            }
        });
    }

    // 6d. Directory scanner (periodic, no inotify)
    {
        let state = Arc::clone(&state);
        let db = Arc::clone(&db);
        rt.spawn(async move {
            let scanner = DirScanner::new();
            let mut wb = WriteBehindBuffer::new();
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;

                let st = state.read().await;
                let active_dirs = st.active_directories();
                let agent_timestamps = st.agent_timestamps();
                drop(st);

                match scanner.scan(&active_dirs, &agent_timestamps) {
                    Ok(events) if !events.is_empty() => {
                        tracing::debug!(count = events.len(), "dirscan events");
                        wb.push(&events);
                        let mut st = state.write().await;
                        st.apply_events(events);
                        drop(st);

                        if wb.should_flush() {
                            let db = db.lock().await;
                            if let Err(e) = wb.flush(&db) {
                                tracing::warn!(error = %e, "dirscan write-behind flush failed");
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "dirscan error");
                    }
                }
            }
        });
    }

    // 6e. IPC server
    {
        let state = Arc::clone(&state);
        let db = Arc::clone(&db);
        let cmd_tx = cmd_tx.clone();
        let socket_path = config.socket_path.clone();
        rt.spawn(async move {
            let server = IpcServer::new(socket_path);
            if let Err(e) = server.run(state, db, cmd_tx).await {
                tracing::error!(error = %e, "IPC server failed");
            }
        });
    }

    // 6f. Plugin discovery
    {
        let plugin_dir = config.plugin_dir.clone();
        rt.spawn(async move {
            let mut pm = PluginManager::new(plugin_dir);
            match pm.discover() {
                Ok(count) => {
                    if count > 0 {
                        tracing::info!(count, "plugins discovered");
                    }
                }
                Err(e) => tracing::warn!(error = %e, "plugin discovery failed"),
            }
        });
    }

    // 6g. FTS5 index rebuild (once on startup)
    {
        let db = Arc::clone(&db);
        rt.spawn(async move {
            let db = db.lock().await;
            if let Err(e) = search::rebuild_index(&db, 500) {
                tracing::warn!(error = %e, "FTS5 index rebuild failed");
            } else {
                tracing::info!("FTS5 search index rebuilt");
            }
        });
    }

    // 6h. Action dispatcher (processes AppActions from panel)
    {
        let dispatcher = Arc::clone(&dispatcher);
        let result_tx = result_tx.clone();
        let mut action_rx = action_rx;
        rt.spawn(async move {
            while let Some(actions) = action_rx.recv().await {
                let results = dispatcher.dispatch(actions).await;
                if !results.is_empty() {
                    let _ = result_tx.send(results).await;
                }
            }
        });
    }

    // ── 7. SCTK Panel (main thread) ───────────────────────
    let panel_config = PanelConfig {
        width: config.panel_width,
        margin: config.panel_margin,
        ..Default::default()
    };

    let (mut shell, mut event_loop, qh) =
        HotbarShell::new(panel_config).context("failed to create SCTK shell")?;
    shell.create_surface(&qh);

    // App state — lives on the main thread, accessed in the ui_callback
    let mut app = HotbarApp::default();
    let mut source_filter = hotbar_common::Filter::All;
    let mut action_filter = hotbar_common::ActionFilter::All;

    // Wrap channels for main-thread access
    let mut cmd_rx = cmd_rx;
    let mut result_rx = result_rx;

    // Set the UI callback — called every frame by SCTK
    let state_for_ui = Arc::clone(&state);
    let search_for_ui = Arc::clone(&search_results);
    let action_tx_for_ui = action_tx.clone();

    shell.set_ui_callback(move |ctx| {
        // Poll for IPC commands (non-blocking)
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                Command::Toggle => {
                    // Handled via DispatchResult::TogglePanel
                }
                Command::Quit => {
                    std::process::exit(0);
                }
                _ => {}
            }
        }

        // Poll for dispatch results (summaries, toasts)
        while let Ok(results) = result_rx.try_recv() {
            for result in results {
                match result {
                    DispatchResult::Toast(msg) => {
                        app.toasts.push(msg, ToastKind::Info);
                    }
                    DispatchResult::Summary {
                        path: _,
                        content,
                        model,
                    } => {
                        app.summary.set_summary(content, model);
                    }
                    DispatchResult::TogglePanel => {
                        // Can't toggle shell from inside callback — handled externally
                    }
                }
            }
        }

        // Read state snapshot (try_read to avoid blocking the frame)
        let files: Vec<HotFile>;
        let pins;
        let heat;
        if let Ok(st) = state_for_ui.try_read() {
            let filtered = st.apply_filter(source_filter, action_filter);
            files = filtered.into_iter().cloned().collect();
            pins = st.pins.clone();
            heat = st.activity.events_per_second();
        } else {
            // State locked — use empty data this frame, next frame will catch up
            files = Vec::new();
            pins = Vec::new();
            heat = 0.0;
        }

        // Check for search results override
        let display_files;
        if let Ok(sr) = search_for_ui.try_read() {
            if let Some(ref results) = *sr {
                display_files = results.clone();
            } else {
                display_files = files;
            }
        } else {
            display_files = files;
        }

        // Draw UI
        let actions = app.draw(ctx, &display_files, &pins);

        // Update filters from app state
        source_filter = app.source_filter;
        action_filter = app.action_filter;

        // Send actions to dispatcher (non-blocking)
        if !actions.is_empty() {
            let _ = action_tx_for_ui.try_send(actions);
        }

        // Request repaint for smooth animation
        ctx.request_repaint();

        let _ = heat; // will be used by shell.set_heat_intensity once wired
    });

    // ── 8. Calloop timer for heat intensity updates ────────
    let state_for_heat = Arc::clone(&state);
    let timer = Timer::from_duration(Duration::from_millis(100));
    event_loop
        .handle()
        .insert_source(timer, move |_, _, shell| {
            if let Ok(st) = state_for_heat.try_read() {
                let heat = st.activity.events_per_second();
                let intensity = (heat / 10.0).clamp(0.0, 1.0);
                shell.set_heat_intensity(intensity);
            }
            TimeoutAction::ToDuration(Duration::from_millis(100))
        })
        .map_err(|e| anyhow::anyhow!("calloop timer: {e}"))?;

    // ── 9. Run event loop (blocks) ─────────────────────────
    tracing::info!("entering event loop");
    event_loop
        .run(None, &mut shell, |_| {})
        .map_err(|e| anyhow::anyhow!("event loop error: {e}"))?;

    tracing::info!("hotbar shutting down");
    Ok(())
}
