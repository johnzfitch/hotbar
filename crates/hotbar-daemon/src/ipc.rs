use std::path::{Path, PathBuf};
use std::sync::Arc;

use hotbar_common::protocol::{decode_command, encode_response, Command, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::{Mutex, RwLock};

use crate::db::Db;
use crate::state::HotState;

/// IPC error types
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Unix socket server for external control (bartender, CLI, scripts).
///
/// The panel does NOT use this socket — it reads daemon state directly via
/// `Arc<RwLock<HotState>>`. This is for external tool integration.
pub struct IpcServer {
    socket_path: PathBuf,
}

impl IpcServer {
    /// Create a new IPC server at the given socket path.
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    /// Create a server at the default path: `$XDG_RUNTIME_DIR/hotbar.sock`
    pub fn default_path() -> PathBuf {
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
            .unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(runtime_dir).join("hotbar.sock")
    }

    /// Run the IPC server, accepting connections and dispatching commands.
    ///
    /// This is intended to run as a long-lived tokio task. It will clean up
    /// the socket file on startup (if leftover from a previous run).
    ///
    /// `cmd_tx` receives commands that need panel action (Toggle, Quit).
    pub async fn run(
        &self,
        state: Arc<RwLock<HotState>>,
        db: Arc<Mutex<Db>>,
        cmd_tx: tokio::sync::mpsc::Sender<Command>,
    ) -> Result<(), IpcError> {
        // Clean up stale socket
        if self.socket_path.exists() {
            tracing::debug!(path = %self.socket_path.display(), "removing stale socket");
            let _ = std::fs::remove_file(&self.socket_path);
        }

        // Ensure parent directory exists
        if let Some(parent) = self.socket_path.parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent)?;
        }

        let listener = UnixListener::bind(&self.socket_path)?;
        tracing::info!(path = %self.socket_path.display(), "IPC server listening");

        loop {
            let (stream, _addr) = listener.accept().await?;
            let state = Arc::clone(&state);
            let db = Arc::clone(&db);
            let cmd_tx = cmd_tx.clone();

            tokio::spawn(async move {
                if let Err(e) =
                    handle_connection(stream, state, db, cmd_tx).await
                {
                    tracing::warn!(error = %e, "IPC connection error");
                }
            });
        }
    }

    /// Get the socket path.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        // Best-effort cleanup of the socket file
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Handle a single IPC connection (one command per line, one response per line).
async fn handle_connection(
    stream: tokio::net::UnixStream,
    state: Arc<RwLock<HotState>>,
    db: Arc<Mutex<Db>>,
    cmd_tx: tokio::sync::mpsc::Sender<Command>,
) -> Result<(), IpcError> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        let response = match decode_command(&line) {
            Ok(cmd) => dispatch_command(cmd, &state, &db, &cmd_tx).await,
            Err(e) => Response::Error {
                message: format!("parse error: {e}"),
                code: Some("PARSE_ERROR".into()),
            },
        };

        let encoded = encode_response(&response)?;
        writer.write_all(encoded.as_bytes()).await?;
        writer.flush().await?;
    }

    Ok(())
}

/// Dispatch a parsed command and return a response.
async fn dispatch_command(
    cmd: Command,
    state: &Arc<RwLock<HotState>>,
    db: &Arc<Mutex<Db>>,
    cmd_tx: &tokio::sync::mpsc::Sender<Command>,
) -> Response {
    match cmd {
        Command::Toggle | Command::Quit => {
            // Forward to panel via channel
            if cmd_tx.send(cmd).await.is_ok() {
                Response::Ok {
                    message: "forwarded".into(),
                }
            } else {
                Response::Error {
                    message: "panel channel closed".into(),
                    code: Some("CHANNEL_CLOSED".into()),
                }
            }
        }

        Command::GetState => {
            let st = state.read().await;
            Response::State {
                files: st.files().to_vec(),
                pins: st.pins.clone(),
                activity_level: hotbar_common::types::ActivityLevel(
                    st.activity.events_per_second(),
                ),
            }
        }

        Command::SetFilter { source } => {
            // The filter is a panel-side concept; forward to panel
            if cmd_tx
                .send(Command::SetFilter { source })
                .await
                .is_ok()
            {
                Response::Ok {
                    message: format!("filter set to {source:?}"),
                }
            } else {
                Response::Error {
                    message: "panel channel closed".into(),
                    code: Some("CHANNEL_CLOSED".into()),
                }
            }
        }

        Command::SetActionFilter { action } => {
            if cmd_tx
                .send(Command::SetActionFilter { action })
                .await
                .is_ok()
            {
                Response::Ok {
                    message: format!("action filter set to {action:?}"),
                }
            } else {
                Response::Error {
                    message: "panel channel closed".into(),
                    code: Some("CHANNEL_CLOSED".into()),
                }
            }
        }

        Command::Pin { path, label } => {
            let now = crate::ingest::unix_now();
            let pin_count = {
                let st = state.read().await;
                st.pins.len() as i32
            };
            let pin = hotbar_common::types::Pin {
                path: path.clone(),
                label,
                pin_group: "default".into(),
                position: pin_count,
                pinned_at: now,
            };

            match db.lock().await.upsert_pin(&pin) {
                Ok(()) => {
                    let mut st = state.write().await;
                    // Remove existing pin for this path if any
                    st.pins.retain(|p| p.path != path);
                    st.pins.push(pin);
                    Response::Ok {
                        message: format!("pinned {path}"),
                    }
                }
                Err(e) => Response::Error {
                    message: format!("db error: {e}"),
                    code: Some("DB_ERROR".into()),
                },
            }
        }

        Command::Unpin { path } => match db.lock().await.remove_pin(&path) {
            Ok(removed) => {
                if removed {
                    let mut st = state.write().await;
                    st.pins.retain(|p| p.path != path);
                    Response::Ok {
                        message: format!("unpinned {path}"),
                    }
                } else {
                    Response::Error {
                        message: format!("{path} was not pinned"),
                        code: Some("NOT_FOUND".into()),
                    }
                }
            }
            Err(e) => Response::Error {
                message: format!("db error: {e}"),
                code: Some("DB_ERROR".into()),
            },
        },

        Command::Search { query, limit } => {
            // FTS5 full-text search with BM25 ranking
            match crate::search::search(&*db.lock().await, &query, limit) {
                Ok(results) => Response::SearchResults { query, results },
                Err(e) => {
                    tracing::warn!(error = %e, "search failed, falling back to substring");
                    // Fallback to in-memory substring match
                    let st = state.read().await;
                    let results = st
                        .files()
                        .iter()
                        .filter(|f| f.path.contains(&query) || f.filename.contains(&query))
                        .take(limit)
                        .cloned()
                        .collect();
                    Response::SearchResults { query, results }
                }
            }
        }

        Command::Summarize { path } => {
            // Check for cached summary
            match db.lock().await.get_summary(&path) {
                Ok(Some(summary)) => Response::SummaryResult {
                    path,
                    summary: summary.content,
                    model: summary.model,
                },
                Ok(None) => Response::Error {
                    message: "no cached summary; inference not yet implemented".into(),
                    code: Some("NOT_CACHED".into()),
                },
                Err(e) => Response::Error {
                    message: format!("db error: {e}"),
                    code: Some("DB_ERROR".into()),
                },
            }
        }

        Command::Refresh => {
            if cmd_tx.send(Command::Refresh).await.is_ok() {
                Response::Ok {
                    message: "refresh triggered".into(),
                }
            } else {
                Response::Error {
                    message: "panel channel closed".into(),
                    code: Some("CHANNEL_CLOSED".into()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hotbar_common::protocol::encode_command;
    use hotbar_common::types::ActivityLevel;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn setup_server() -> (
        PathBuf,
        Arc<RwLock<HotState>>,
        Arc<Mutex<Db>>,
        tokio::sync::mpsc::Receiver<Command>,
        tokio::task::JoinHandle<()>,
        tempfile::TempDir,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let sock_path = tmp.path().join("test.sock");
        let state = Arc::new(RwLock::new(HotState::new()));
        let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(32);

        let server = IpcServer::new(sock_path.clone());
        let state_c = Arc::clone(&state);
        let db_c = Arc::clone(&db);

        let handle = tokio::spawn(async move {
            let _ = server.run(state_c, db_c, cmd_tx).await;
        });

        // Give server a moment to bind
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        (sock_path, state, db, cmd_rx, handle, tmp)
    }

    #[tokio::test]
    async fn get_state_returns_empty() {
        let (sock_path, _state, _db, _rx, handle, _tmp) = setup_server().await;

        let stream = tokio::net::UnixStream::connect(&sock_path).await.unwrap();
        let (mut reader, mut writer) = stream.into_split();

        let cmd = encode_command(&Command::GetState).unwrap();
        writer.write_all(cmd.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        let mut buf = vec![0u8; 4096];
        let n = reader.read(&mut buf).await.unwrap();
        let response_str = std::str::from_utf8(&buf[..n]).unwrap();
        let resp: Response = serde_json::from_str(response_str.trim()).unwrap();

        match resp {
            Response::State {
                files,
                pins,
                activity_level,
            } => {
                assert!(files.is_empty());
                assert!(pins.is_empty());
                assert_eq!(activity_level, ActivityLevel(0.0));
            }
            other => panic!("expected State, got {other:?}"),
        }

        handle.abort();
    }

    #[tokio::test]
    async fn toggle_forwarded_to_channel() {
        let (sock_path, _state, _db, mut rx, handle, _tmp) = setup_server().await;

        let stream = tokio::net::UnixStream::connect(&sock_path).await.unwrap();
        let (mut reader, mut writer) = stream.into_split();

        let cmd = encode_command(&Command::Toggle).unwrap();
        writer.write_all(cmd.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        // Read response
        let mut buf = vec![0u8; 4096];
        let n = reader.read(&mut buf).await.unwrap();
        let response_str = std::str::from_utf8(&buf[..n]).unwrap();
        let resp: Response = serde_json::from_str(response_str.trim()).unwrap();
        assert!(matches!(resp, Response::Ok { .. }));

        // Verify command was forwarded
        let received = rx.recv().await.unwrap();
        assert!(matches!(received, Command::Toggle));

        handle.abort();
    }

    #[tokio::test]
    async fn search_fts5() {
        let (sock_path, state, db, _rx, handle, _tmp) = setup_server().await;

        // Add files to state and DB, then index in FTS5
        {
            use hotbar_common::types::{Action, Confidence, FileEvent, Source};
            let events = vec![
                FileEvent {
                    path: "/home/test/dev/main.rs".into(),
                    action: Action::Modified,
                    source: Source::Claude,
                    timestamp: 100,
                    confidence: Confidence::High,
                    session_id: None,
                },
                FileEvent {
                    path: "/home/test/dev/lib.rs".into(),
                    action: Action::Modified,
                    source: Source::User,
                    timestamp: 200,
                    confidence: Confidence::High,
                    session_id: None,
                },
            ];
            let mut st = state.write().await;
            st.apply_events(events.clone());

            let db = db.lock().await;
            db.insert_events(&events).unwrap();
            crate::search::index_file(&db, "/home/test/dev/main.rs", "main.rs", None).unwrap();
            crate::search::index_file(&db, "/home/test/dev/lib.rs", "lib.rs", None).unwrap();
        }

        let stream = tokio::net::UnixStream::connect(&sock_path).await.unwrap();
        let (mut reader, mut writer) = stream.into_split();

        let cmd = encode_command(&Command::Search {
            query: "main".into(),
            limit: 10,
        })
        .unwrap();
        writer.write_all(cmd.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        let mut buf = vec![0u8; 8192];
        let n = reader.read(&mut buf).await.unwrap();
        let response_str = std::str::from_utf8(&buf[..n]).unwrap();
        let resp: Response = serde_json::from_str(response_str.trim()).unwrap();

        match resp {
            Response::SearchResults { results, .. } => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].path, "/home/test/dev/main.rs");
            }
            other => panic!("expected SearchResults, got {other:?}"),
        }

        handle.abort();
    }

    #[tokio::test]
    async fn pin_and_unpin() {
        let (sock_path, state, _db, _rx, handle, _tmp) = setup_server().await;

        let stream = tokio::net::UnixStream::connect(&sock_path).await.unwrap();
        let (mut reader, mut writer) = stream.into_split();

        // Pin
        let cmd = encode_command(&Command::Pin {
            path: "/home/test/file.rs".into(),
            label: Some("test pin".into()),
        })
        .unwrap();
        writer.write_all(cmd.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        let mut buf = vec![0u8; 4096];
        let n = reader.read(&mut buf).await.unwrap();
        let resp: Response =
            serde_json::from_str(std::str::from_utf8(&buf[..n]).unwrap().trim()).unwrap();
        assert!(matches!(resp, Response::Ok { .. }));

        // Verify pin exists in state
        {
            let st = state.read().await;
            assert_eq!(st.pins.len(), 1);
            assert_eq!(st.pins[0].path, "/home/test/file.rs");
        }

        // Unpin
        let cmd = encode_command(&Command::Unpin {
            path: "/home/test/file.rs".into(),
        })
        .unwrap();
        writer.write_all(cmd.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        buf.fill(0);
        let n = reader.read(&mut buf).await.unwrap();
        let resp: Response =
            serde_json::from_str(std::str::from_utf8(&buf[..n]).unwrap().trim()).unwrap();
        assert!(matches!(resp, Response::Ok { .. }));

        {
            let st = state.read().await;
            assert!(st.pins.is_empty());
        }

        handle.abort();
    }

    #[tokio::test]
    async fn invalid_json_returns_error() {
        let (sock_path, _state, _db, _rx, handle, _tmp) = setup_server().await;

        let stream = tokio::net::UnixStream::connect(&sock_path).await.unwrap();
        let (mut reader, mut writer) = stream.into_split();

        writer.write_all(b"not json\n").await.unwrap();
        writer.flush().await.unwrap();

        let mut buf = vec![0u8; 4096];
        let n = reader.read(&mut buf).await.unwrap();
        let resp: Response =
            serde_json::from_str(std::str::from_utf8(&buf[..n]).unwrap().trim()).unwrap();

        assert!(matches!(resp, Response::Error { .. }));

        handle.abort();
    }
}
