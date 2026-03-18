//! SQLite-backed tracing layer for long-term structured trace storage.
//!
//! Writes span lifecycle events and log events into a WAL-mode SQLite database.
//! Both the daemon and panel import this layer so all hotbar trace data flows
//! into a single organized store at `$XDG_DATA_HOME/hotbar/traces.db`.
//!
//! # Schema
//!
//! - `spans`: one row per span close (name, target, level, start/end timestamps, fields)
//! - `events`: one row per tracing event (level, target, message, timestamp, parent span)
//! - `sessions`: one row per process startup (pid, component, start time)
//!
//! # Usage
//!
//! ```ignore
//! let layer = trace_db::SqliteLayer::open("traces.db", "panel")?;
//! tracing_subscriber::registry().with(layer).init();
//! ```

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use rusqlite::{params, Connection};
use tracing::field::{Field, Visit};
use tracing::span;
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

/// Schema DDL for the trace database.
const TRACE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    id          INTEGER PRIMARY KEY,
    pid         INTEGER NOT NULL,
    component   TEXT NOT NULL,
    started_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS spans (
    id          INTEGER PRIMARY KEY,
    session_id  INTEGER NOT NULL REFERENCES sessions(id),
    parent_id   INTEGER,
    name        TEXT NOT NULL,
    target      TEXT NOT NULL,
    level       TEXT NOT NULL,
    start_us    INTEGER NOT NULL,
    end_us      INTEGER NOT NULL,
    fields      TEXT
);

CREATE TABLE IF NOT EXISTS events (
    id          INTEGER PRIMARY KEY,
    session_id  INTEGER NOT NULL REFERENCES sessions(id),
    span_id     INTEGER,
    level       TEXT NOT NULL,
    target      TEXT NOT NULL,
    message     TEXT NOT NULL,
    timestamp_us INTEGER NOT NULL,
    fields      TEXT
);

CREATE INDEX IF NOT EXISTS idx_spans_name ON spans(name);
CREATE INDEX IF NOT EXISTS idx_spans_session ON spans(session_id);
CREATE INDEX IF NOT EXISTS idx_spans_start ON spans(start_us);
CREATE INDEX IF NOT EXISTS idx_events_session ON events(session_id);
CREATE INDEX IF NOT EXISTS idx_events_timestamp ON events(timestamp_us);
CREATE INDEX IF NOT EXISTS idx_events_level ON events(level);
";

/// Initialize the SQLite trace layer at the default path.
///
/// Convenience wrapper around `SqliteLayer::open()` + `default_trace_path()`.
/// Also prunes old data (>30 days) on startup.
///
/// Compose into your subscriber:
/// ```ignore
/// use tracing_subscriber::layer::SubscriberExt;
/// let sqlite_layer = trace_db::init("panel").unwrap();
/// tracing_subscriber::registry()
///     .with(env_filter)
///     .with(fmt_layer)
///     .with(sqlite_layer)
///     .init();
/// ```
pub fn init(component: &str) -> Result<SqliteLayer, rusqlite::Error> {
    let path = default_trace_path();
    // Best-effort prune on startup (ignore errors — DB may not exist yet)
    let _ = prune(&path, 30);
    SqliteLayer::open(&path, component)
}

/// Default trace database path.
pub fn default_trace_path() -> std::path::PathBuf {
    let data_dir = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{home}/.local/share")
    });
    std::path::PathBuf::from(data_dir).join("hotbar/traces.db")
}

/// Per-span state tracked between enter and close.
struct SpanTiming {
    start: Instant,
    parent_id: Option<u64>,
    fields: String,
}

/// Buffered write entry — avoids holding the DB lock per-span.
enum TraceEntry {
    Span {
        parent_id: Option<u64>,
        name: String,
        target: String,
        level: String,
        start_us: i64,
        end_us: i64,
        fields: String,
    },
    Event {
        span_id: Option<u64>,
        level: String,
        target: String,
        message: String,
        timestamp_us: i64,
        fields: String,
    },
}

/// A tracing [`Layer`] that writes span and event data to SQLite.
///
/// Thread-safe via internal `Mutex<Connection>`. Batches writes to minimize
/// lock contention — flushes every `flush_threshold` entries or on drop.
pub struct SqliteLayer {
    db: Mutex<Connection>,
    session_id: i64,
    epoch: Instant,
    /// In-flight spans: span Id -> timing data
    open_spans: Mutex<HashMap<span::Id, SpanTiming>>,
    /// Write buffer
    buffer: Mutex<Vec<TraceEntry>>,
    /// Flush when buffer reaches this size
    flush_threshold: usize,
}

/// Minimum level to record (to avoid flooding with trace-level frame spans
/// during normal operation). Defaults to DEBUG.
const MIN_LEVEL: tracing::Level = tracing::Level::DEBUG;

/// Flush threshold — number of buffered entries before a DB write.
const DEFAULT_FLUSH_THRESHOLD: usize = 64;

impl SqliteLayer {
    /// Open (or create) a trace database and register a new session.
    ///
    /// `component` identifies the source process ("daemon" or "panel").
    pub fn open(path: &Path, component: &str) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            let _ = std::fs::create_dir_all(parent);
        }

        let conn = Connection::open(path)?;

        // WAL mode for concurrent reads (panel can query while daemon writes)
        conn.pragma(None, "journal_mode", "WAL", |_| Ok(()))?;
        conn.pragma(None, "synchronous", "NORMAL", |_| Ok(()))?;
        conn.pragma(None, "busy_timeout", 1000, |_| Ok(()))?;

        conn.execute_batch(TRACE_SCHEMA)?;

        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        conn.execute(
            "INSERT INTO sessions (pid, component, started_at) VALUES (?1, ?2, ?3)",
            params![std::process::id() as i64, component, now_unix],
        )?;
        let session_id = conn.last_insert_rowid();

        Ok(Self {
            db: Mutex::new(conn),
            session_id,
            epoch: Instant::now(),
            open_spans: Mutex::new(HashMap::new()),
            buffer: Mutex::new(Vec::with_capacity(DEFAULT_FLUSH_THRESHOLD)),
            flush_threshold: DEFAULT_FLUSH_THRESHOLD,
        })
    }

    /// Microseconds since this layer was created (monotonic).
    fn elapsed_us(&self) -> i64 {
        self.epoch.elapsed().as_micros() as i64
    }

    /// Flush buffered entries to the database.
    fn flush(&self) {
        let entries: Vec<TraceEntry> = {
            let mut buf = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *buf)
        };

        if entries.is_empty() {
            return;
        }

        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        // Batch in a transaction for performance
        let _ = db.execute_batch("BEGIN");

        for entry in &entries {
            match entry {
                TraceEntry::Span {
                    parent_id,
                    name,
                    target,
                    level,
                    start_us,
                    end_us,
                    fields,
                } => {
                    let _ = db.execute(
                        "INSERT INTO spans (session_id, parent_id, name, target, level, start_us, end_us, fields)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![
                            self.session_id,
                            parent_id.map(|id| id as i64),
                            name,
                            target,
                            level,
                            start_us,
                            end_us,
                            if fields.is_empty() { None } else { Some(fields.as_str()) },
                        ],
                    );
                }
                TraceEntry::Event {
                    span_id,
                    level,
                    target,
                    message,
                    timestamp_us,
                    fields,
                } => {
                    let _ = db.execute(
                        "INSERT INTO events (session_id, span_id, level, target, message, timestamp_us, fields)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![
                            self.session_id,
                            span_id.map(|id| id as i64),
                            level,
                            target,
                            message,
                            timestamp_us,
                            if fields.is_empty() { None } else { Some(fields.as_str()) },
                        ],
                    );
                }
            }
        }

        let _ = db.execute_batch("COMMIT");
    }

    /// Push an entry to the buffer, flushing if threshold reached.
    fn push_entry(&self, entry: TraceEntry) {
        let should_flush = {
            let mut buf = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
            buf.push(entry);
            buf.len() >= self.flush_threshold
        };
        if should_flush {
            self.flush();
        }
    }
}

impl Drop for SqliteLayer {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Visitor that collects span/event fields into a compact JSON-ish string.
struct FieldCollector {
    fields: String,
    message: String,
}

impl FieldCollector {
    fn new() -> Self {
        Self {
            fields: String::new(),
            message: String::new(),
        }
    }
}

impl Visit for FieldCollector {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            if !self.fields.is_empty() {
                self.fields.push_str(", ");
            }
            self.fields
                .push_str(&format!("{}={:?}", field.name(), value));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            if !self.fields.is_empty() {
                self.fields.push_str(", ");
            }
            self.fields
                .push_str(&format!("{}=\"{}\"", field.name(), value));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if !self.fields.is_empty() {
            self.fields.push_str(", ");
        }
        self.fields
            .push_str(&format!("{}={}", field.name(), value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if !self.fields.is_empty() {
            self.fields.push_str(", ");
        }
        self.fields
            .push_str(&format!("{}={}", field.name(), value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        if !self.fields.is_empty() {
            self.fields.push_str(", ");
        }
        self.fields
            .push_str(&format!("{}={:.3}", field.name(), value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if !self.fields.is_empty() {
            self.fields.push_str(", ");
        }
        self.fields
            .push_str(&format!("{}={}", field.name(), value));
    }
}

// Thread-local current-span tracking for parent resolution
thread_local! {
    static CURRENT_SPAN_ID: RefCell<Option<u64>> = const { RefCell::new(None) };
}

impl<S> Layer<S> for SqliteLayer
where
    S: tracing::Subscriber + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>,
{
    fn on_new_span(
        &self,
        attrs: &span::Attributes<'_>,
        id: &span::Id,
        _ctx: Context<'_, S>,
    ) {
        if *attrs.metadata().level() > MIN_LEVEL {
            return;
        }

        let mut collector = FieldCollector::new();
        attrs.record(&mut collector);

        let parent_id = CURRENT_SPAN_ID.with(|c| *c.borrow());

        let timing = SpanTiming {
            start: Instant::now(),
            parent_id,
            fields: collector.fields,
        };

        let mut spans = self.open_spans.lock().unwrap_or_else(|e| e.into_inner());
        spans.insert(id.clone(), timing);
    }

    fn on_enter(&self, id: &span::Id, _ctx: Context<'_, S>) {
        CURRENT_SPAN_ID.with(|c| {
            *c.borrow_mut() = Some(id.into_u64());
        });
    }

    fn on_exit(&self, _id: &span::Id, _ctx: Context<'_, S>) {
        // Restore parent on exit (simplified — no stack, just clear)
        let parent = {
            let spans = self.open_spans.lock().unwrap_or_else(|e| e.into_inner());
            spans.get(_id).and_then(|s| s.parent_id)
        };
        CURRENT_SPAN_ID.with(|c| {
            *c.borrow_mut() = parent;
        });
    }

    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let timing = {
            let mut spans = self.open_spans.lock().unwrap_or_else(|e| e.into_inner());
            spans.remove(&id)
        };

        let Some(timing) = timing else { return };

        let end = Instant::now();
        let start_us = timing.start.duration_since(self.epoch).as_micros() as i64;
        let end_us = end.duration_since(self.epoch).as_micros() as i64;

        // Get metadata from the subscriber registry
        let (name, target, level) = ctx
            .span(&id)
            .map(|span| {
                let meta = span.metadata();
                (
                    meta.name().to_string(),
                    meta.target().to_string(),
                    meta.level().to_string(),
                )
            })
            .unwrap_or_else(|| ("unknown".into(), "unknown".into(), "TRACE".into()));

        self.push_entry(TraceEntry::Span {
            parent_id: timing.parent_id,
            name,
            target,
            level,
            start_us,
            end_us,
            fields: timing.fields,
        });
    }

    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if *event.metadata().level() > MIN_LEVEL {
            return;
        }

        let mut collector = FieldCollector::new();
        event.record(&mut collector);

        let span_id = CURRENT_SPAN_ID.with(|c| *c.borrow());

        self.push_entry(TraceEntry::Event {
            span_id,
            level: event.metadata().level().to_string(),
            target: event.metadata().target().to_string(),
            message: collector.message,
            timestamp_us: self.elapsed_us(),
            fields: collector.fields,
        });
    }
}

/// Prune old trace data. Keeps the last `keep_days` days of data.
///
/// Call periodically (e.g. on startup) to prevent unbounded growth.
pub fn prune(path: &Path, keep_days: u32) -> Result<usize, rusqlite::Error> {
    let conn = Connection::open(path)?;

    let cutoff = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
        - (keep_days as i64 * 86400);

    // Find sessions older than cutoff
    let deleted_spans: usize = conn.execute(
        "DELETE FROM spans WHERE session_id IN (SELECT id FROM sessions WHERE started_at < ?1)",
        params![cutoff],
    )?;
    let deleted_events: usize = conn.execute(
        "DELETE FROM events WHERE session_id IN (SELECT id FROM sessions WHERE started_at < ?1)",
        params![cutoff],
    )?;
    conn.execute(
        "DELETE FROM sessions WHERE started_at < ?1",
        params![cutoff],
    )?;

    // Reclaim space
    conn.execute_batch("PRAGMA incremental_vacuum(256)")?;

    Ok(deleted_spans + deleted_events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn layer_records_spans_and_events() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test_traces.db");

        let layer = SqliteLayer::open(&db_path, "test").unwrap();
        let session_id = layer.session_id;

        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        // Emit a span with an event inside
        {
            let _span = tracing::debug_span!("test_op", count = 42).entered();
            tracing::debug!(result = "ok", "operation complete");
        }

        // The layer flushes on drop — force it by dropping the guard
        drop(_guard);

        // Verify DB contents
        let conn = Connection::open(&db_path).unwrap();

        let session_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(session_count, 1);

        let span_count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM spans WHERE session_id = ?1",
                params![session_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(span_count >= 1, "expected at least 1 span, got {span_count}");

        let span_name: String = conn
            .query_row(
                "SELECT name FROM spans WHERE session_id = ?1 LIMIT 1",
                params![session_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(span_name, "test_op");

        let event_count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id = ?1",
                params![session_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            event_count >= 1,
            "expected at least 1 event, got {event_count}"
        );
    }

    #[test]
    fn prune_removes_old_data() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("prune_test.db");

        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(TRACE_SCHEMA).unwrap();

        // Insert an old session (31 days ago)
        let old_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            - 31 * 86400;

        conn.execute(
            "INSERT INTO sessions (pid, component, started_at) VALUES (1, 'test', ?1)",
            params![old_time],
        )
        .unwrap();
        let old_session_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO spans (session_id, name, target, level, start_us, end_us) VALUES (?1, 'old', 'test', 'DEBUG', 0, 100)",
            params![old_session_id],
        ).unwrap();

        // Insert a recent session
        let recent_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        conn.execute(
            "INSERT INTO sessions (pid, component, started_at) VALUES (2, 'test', ?1)",
            params![recent_time],
        )
        .unwrap();
        let recent_session_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO spans (session_id, name, target, level, start_us, end_us) VALUES (?1, 'new', 'test', 'DEBUG', 0, 100)",
            params![recent_session_id],
        ).unwrap();

        drop(conn);

        let deleted = prune(&db_path, 30).unwrap();
        assert!(deleted >= 1);

        let conn = Connection::open(&db_path).unwrap();
        let remaining: i32 = conn
            .query_row("SELECT COUNT(*) FROM spans", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 1);
    }

    #[test]
    fn default_trace_path_is_under_xdg() {
        let path = default_trace_path();
        let s = path.to_string_lossy();
        assert!(s.contains("hotbar/traces.db"), "path was: {s}");
    }
}
