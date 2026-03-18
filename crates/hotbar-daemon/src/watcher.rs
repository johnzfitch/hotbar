//! Filesystem watchers for ingest source files.
//!
//! Uses the `notify` crate (inotify on Linux) to watch for changes to
//! Claude Code events.jsonl, Codex session directories, and the XBEL file.
//! On change, signals the corresponding ingest task via `tokio::sync::Notify`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::Notify;

/// Watcher error types.
#[derive(Debug, thiserror::Error)]
pub enum WatcherError {
    #[error("notify error: {0}")]
    Notify(#[from] notify::Error),
}

/// Filesystem watchers that signal ingest tasks on file changes.
///
/// Holds the `RecommendedWatcher` (inotify on Linux). The watcher runs on
/// a background thread managed by `notify`. Dropping this struct stops watching.
pub struct IngestWatcher {
    _watcher: RecommendedWatcher,
}

impl IngestWatcher {
    /// Create watchers for all ingest source paths.
    ///
    /// Each `Notify` is signaled when the corresponding source changes.
    /// If a path doesn't exist, the watcher logs a warning and skips it
    /// (the ingest task will fall back to periodic polling).
    pub fn new(
        claude_path: &Path,
        codex_dir: &Path,
        xbel_path: &Path,
        claude_notify: Arc<Notify>,
        codex_notify: Arc<Notify>,
        xbel_notify: Arc<Notify>,
    ) -> Result<Self, WatcherError> {
        // Paths for the closure (moved in)
        let cl_parent = claude_path.parent().unwrap_or(claude_path).to_path_buf();
        let cl_filename = claude_path.file_name().map(|f| f.to_os_string());
        let cx_dir = codex_dir.to_path_buf();
        let xb_parent = xbel_path.parent().unwrap_or(xbel_path).to_path_buf();
        let xb_filename = xbel_path.file_name().map(|f| f.to_os_string());

        // Paths for the watch registration (cloned before closure takes ownership)
        let watch_claude = cl_parent.clone();
        let watch_codex = cx_dir.clone();
        let watch_xbel = xb_parent.clone();

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                let event = match res {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::warn!(error = %e, "filesystem watcher error");
                        return;
                    }
                };

                for path in &event.paths {
                    if path.starts_with(&cl_parent)
                        && let (Some(name), Some(expected)) = (path.file_name(), &cl_filename)
                        && name == expected.as_os_str()
                    {
                        claude_notify.notify_one();
                        continue;
                    }

                    if path.starts_with(&cx_dir) {
                        codex_notify.notify_one();
                        continue;
                    }

                    if path.starts_with(&xb_parent)
                        && let (Some(name), Some(expected)) = (path.file_name(), &xb_filename)
                        && name == expected.as_os_str()
                    {
                        xbel_notify.notify_one();
                        continue;
                    }
                }
            },
            Config::default(),
        )?;

        // Register watches, log warnings for missing paths
        for (path, mode, label) in [
            (watch_claude.as_path(), RecursiveMode::NonRecursive, "claude events"),
            (watch_codex.as_path(), RecursiveMode::Recursive, "codex sessions"),
            (watch_xbel.as_path(), RecursiveMode::NonRecursive, "xbel"),
        ] {
            if path.exists() {
                if let Err(e) = watcher.watch(path, mode) {
                    tracing::warn!(
                        path = %path.display(),
                        source = label,
                        error = %e,
                        "failed to watch, falling back to polling"
                    );
                } else {
                    tracing::info!(path = %path.display(), source = label, "watching");
                }
            } else {
                tracing::warn!(
                    path = %path.display(),
                    source = label,
                    "path missing, falling back to polling"
                );
            }
        }

        Ok(Self { _watcher: watcher })
    }
}

/// Discover the Claude Code events.jsonl path.
///
/// Scans `~/.claude/projects/` for the most recently modified `events.jsonl`.
pub fn find_claude_events() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let projects_dir = PathBuf::from(&home).join(".claude/projects");
    if !projects_dir.is_dir() {
        return None;
    }

    let mut best: Option<(PathBuf, std::time::SystemTime)> = None;

    if let Ok(entries) = std::fs::read_dir(&projects_dir) {
        for entry in entries.flatten() {
            let events_path = entry.path().join("events.jsonl");
            if events_path.is_file()
                && let Ok(meta) = events_path.metadata()
                && let Ok(mtime) = meta.modified()
                && best.as_ref().is_none_or(|(_, t)| mtime > *t)
            {
                best = Some((events_path, mtime));
            }
        }
    }

    best.map(|(p, _)| p)
}

/// Get the default Codex sessions directory.
pub fn codex_sessions_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".codex/sessions")
}

/// Get the default XBEL path.
pub fn xbel_path() -> PathBuf {
    let data_home = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{home}/.local/share")
    });
    PathBuf::from(data_home).join("recently-used.xbel")
}
