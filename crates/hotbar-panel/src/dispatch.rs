//! AppAction dispatcher — executes actions returned by the panel UI.
//!
//! Each `AppAction` variant maps to a side effect: opening files, copying text,
//! modifying pins, requesting inference, or toggling visibility.

use std::path::Path;
use std::sync::Arc;

use hotbar_common::{HotFile, Pin};
use hotbar_daemon::db::Db;
use hotbar_daemon::inference::Summarizer;
use hotbar_daemon::search;
use hotbar_daemon::state::HotState;
use tokio::sync::{Mutex, RwLock};

use crate::app::AppAction;

/// Shared context for dispatching app actions.
pub struct Dispatcher {
    state: Arc<RwLock<HotState>>,
    db: Arc<Mutex<Db>>,
    summarizer: Arc<Summarizer>,
    /// Search results override — when Some, the panel shows these instead of the full file list.
    search_results: Arc<RwLock<Option<Vec<HotFile>>>>,
}

impl Dispatcher {
    /// Create a new dispatcher.
    pub fn new(
        state: Arc<RwLock<HotState>>,
        db: Arc<Mutex<Db>>,
        summarizer: Arc<Summarizer>,
        search_results: Arc<RwLock<Option<Vec<HotFile>>>>,
    ) -> Self {
        Self {
            state,
            db,
            summarizer,
            search_results,
        }
    }

    /// Dispatch a batch of actions.
    pub async fn dispatch(&self, actions: Vec<AppAction>) -> Vec<DispatchResult> {
        let mut results = Vec::new();

        for action in actions {
            match action {
                AppAction::OpenFile(path) => {
                    open_with_xdg(&path);
                }

                AppAction::OpenFolder(path) => {
                    let dir = Path::new(&path)
                        .parent()
                        .unwrap_or(Path::new(&path))
                        .to_string_lossy()
                        .to_string();
                    open_with_xdg(&dir);
                }

                AppAction::CopyToClipboard(text) => {
                    copy_to_clipboard(&text);
                    results.push(DispatchResult::Toast("Copied to clipboard".into()));
                }

                AppAction::PinFile(path) => {
                    let mut st = self.state.write().await;
                    if !st.pins.iter().any(|p| p.path == path) {
                        let pin = Pin {
                            path: path.clone(),
                            label: Some(
                                Path::new(&path)
                                    .file_name()
                                    .map(|f| f.to_string_lossy().to_string())
                                    .unwrap_or_else(|| path.clone()),
                            ),
                            pin_group: "default".into(),
                            position: st.pins.len() as i32,
                            pinned_at: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs() as i64,
                        };
                        st.pins.push(pin.clone());
                        drop(st);
                        let db = self.db.lock().await;
                        if let Err(e) = db.upsert_pin(&pin) {
                            tracing::warn!(error = %e, "failed to persist pin");
                        }
                    }
                    results.push(DispatchResult::Toast("Pinned".into()));
                }

                AppAction::UnpinFile(path) => {
                    let mut st = self.state.write().await;
                    st.pins.retain(|p| p.path != path);
                    drop(st);
                    let db = self.db.lock().await;
                    if let Err(e) = db.remove_pin(&path) {
                        tracing::warn!(error = %e, "failed to remove pin from db");
                    }
                    results.push(DispatchResult::Toast("Unpinned".into()));
                }

                AppAction::Summarize(path) => {
                    // Check DB cache — acquire and release lock before any async work
                    let cached = {
                        let db = self.db.lock().await;
                        db.get_summary(&path).ok().flatten()
                    };

                    if let Some(summary) = cached {
                        results.push(DispatchResult::Summary {
                            path,
                            content: summary.content,
                            model: summary.model,
                        });
                    } else {
                        // Run inference without holding DB lock (Db is !Sync).
                        // We inline the summarizer logic here so lock scoping is explicit.
                        let summarizer = &self.summarizer;
                        match summarizer.infer(&path).await {
                            Ok((summary_text, model_name)) => {
                                // Cache result
                                let db = self.db.lock().await;
                                let _ = db.upsert_summary(
                                    &path,
                                    &summary_text,
                                    &model_name,
                                );
                                drop(db);
                                results.push(DispatchResult::Summary {
                                    path,
                                    content: summary_text,
                                    model: model_name,
                                });
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "summarization failed");
                                results.push(DispatchResult::Toast(
                                    format!("Summary failed: {e}"),
                                ));
                            }
                        }
                    }
                }

                AppAction::Search(query) => {
                    let search_files = {
                        let db = self.db.lock().await;
                        search::search(&db, &query, 50)
                    };
                    match search_files {
                        Ok(files) => {
                            let mut sr = self.search_results.write().await;
                            *sr = Some(files);
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "search failed");
                        }
                    }
                }

                AppAction::ClearSearch => {
                    let mut sr = self.search_results.write().await;
                    *sr = None;
                }

                AppAction::Toggle => {
                    results.push(DispatchResult::TogglePanel);
                }

                AppAction::SetSourceFilter(_) | AppAction::SetActionFilter(_) => {
                    // Filters are applied in the main loop when reading state
                }

                AppAction::ReorderPins { from, to } => {
                    let mut st = self.state.write().await;
                    if from < st.pins.len() && to < st.pins.len() {
                        let pin = st.pins.remove(from);
                        st.pins.insert(to, pin);
                        for (i, p) in st.pins.iter_mut().enumerate() {
                            p.position = i as i32;
                        }
                    }
                }
            }
        }

        results
    }
}

/// Result of dispatching an action that the main loop needs to handle.
#[derive(Debug)]
pub enum DispatchResult {
    /// Show a toast notification
    Toast(String),
    /// Feed a summary result back to the summary widget
    Summary {
        path: String,
        content: String,
        model: String,
    },
    /// Toggle panel visibility
    TogglePanel,
}

/// Open a path with `xdg-open` (non-blocking).
fn open_with_xdg(path: &str) {
    tracing::debug!(path, "opening with xdg-open");
    match std::process::Command::new("xdg-open")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => {}
        Err(e) => tracing::warn!(path, error = %e, "xdg-open failed"),
    }
}

/// Copy text to clipboard via `wl-copy` (non-blocking).
fn copy_to_clipboard(text: &str) {
    tracing::debug!("copying to clipboard");
    match std::process::Command::new("wl-copy")
        .arg(text)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "wl-copy failed"),
    }
}
