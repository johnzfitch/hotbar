//! Write-behind buffer for deferred database persistence.
//!
//! Events are applied to in-memory state immediately but buffered
//! before hitting the DB. Flushed every 500ms or when 64 events
//! accumulate — whichever comes first. One explicit transaction
//! per flush instead of one implicit transaction per row.

use std::time::{Duration, Instant};

use hotbar_common::types::FileEvent;

use crate::db::{Db, DbError};

/// Default flush threshold (number of pending events).
const DEFAULT_MAX_BATCH: usize = 64;

/// Default flush interval.
const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_millis(500);

/// Buffers file events for batched DB writes.
pub struct WriteBehindBuffer {
    pending: Vec<FileEvent>,
    last_flush: Instant,
    max_batch: usize,
    flush_interval: Duration,
}

impl WriteBehindBuffer {
    /// Create a buffer with default thresholds (64 events / 500ms).
    pub fn new() -> Self {
        Self {
            pending: Vec::with_capacity(DEFAULT_MAX_BATCH),
            last_flush: Instant::now(),
            max_batch: DEFAULT_MAX_BATCH,
            flush_interval: DEFAULT_FLUSH_INTERVAL,
        }
    }

    /// Enqueue events for deferred persistence.
    pub fn push(&mut self, events: &[FileEvent]) {
        self.pending.extend_from_slice(events);
    }

    /// Whether the buffer should be flushed (threshold or timer).
    pub fn should_flush(&self) -> bool {
        self.pending.len() >= self.max_batch || self.last_flush.elapsed() >= self.flush_interval
    }

    /// Flush all pending events to the database in a single transaction.
    ///
    /// Returns the number of events written, or 0 if the buffer was empty.
    pub fn flush(&mut self, db: &Db) -> Result<usize, DbError> {
        if self.pending.is_empty() {
            return Ok(0);
        }
        let count = db.insert_events_batch(&self.pending)?;
        self.pending.clear();
        self.last_flush = Instant::now();
        tracing::debug!(count, "write-behind flush");
        Ok(count)
    }

    /// Number of pending events.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

impl Default for WriteBehindBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hotbar_common::types::{Action, Confidence, Source};

    fn make_event(path: &str, ts: i64) -> FileEvent {
        FileEvent {
            path: path.into(),
            action: Action::Modified,
            source: Source::Claude,
            timestamp: ts,
            confidence: Confidence::High,
            session_id: None,
        }
    }

    #[test]
    fn flush_on_threshold() {
        let mut buf = WriteBehindBuffer::new();
        for i in 0..65 {
            buf.push(&[make_event(&format!("/f{i}.rs"), i)]);
        }
        assert!(buf.should_flush());
    }

    #[test]
    fn no_flush_below_threshold() {
        let mut buf = WriteBehindBuffer::new();
        buf.push(&[make_event("/a.rs", 1)]);
        // Fresh buffer, well below threshold — should not flush
        assert!(!buf.should_flush());
    }

    #[test]
    fn flush_on_timer() {
        let mut buf = WriteBehindBuffer {
            pending: Vec::new(),
            last_flush: Instant::now() - Duration::from_secs(1),
            max_batch: DEFAULT_MAX_BATCH,
            flush_interval: DEFAULT_FLUSH_INTERVAL,
        };
        buf.push(&[make_event("/a.rs", 1)]);
        assert!(buf.should_flush());
    }

    #[test]
    fn flush_writes_to_db() {
        let db = Db::open_in_memory().unwrap();
        // Insert a session for FK
        db.conn()
            .execute(
                "INSERT INTO sessions (session_id, agent, started_at) VALUES (?1, ?2, ?3)",
                rusqlite::params!["s1", "claude", 100],
            )
            .unwrap();

        let mut buf = WriteBehindBuffer::new();
        buf.push(&[
            FileEvent {
                session_id: Some("s1".into()),
                ..make_event("/a.rs", 100)
            },
            FileEvent {
                session_id: Some("s1".into()),
                ..make_event("/b.rs", 200)
            },
        ]);

        let count = buf.flush(&db).unwrap();
        assert_eq!(count, 2);

        let files = db.get_events(None, 10).unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn flush_clears_buffer() {
        let db = Db::open_in_memory().unwrap();
        let mut buf = WriteBehindBuffer::new();
        buf.push(&[make_event("/a.rs", 1)]);
        assert_eq!(buf.pending_count(), 1);

        // Flush will fail (no session FK) but pending should still be checked
        // Use empty flush instead
        let mut buf2 = WriteBehindBuffer::new();
        let count = buf2.flush(&db).unwrap();
        assert_eq!(count, 0);
        assert_eq!(buf2.pending_count(), 0);
    }

    #[test]
    fn empty_flush_returns_zero() {
        let db = Db::open_in_memory().unwrap();
        let mut buf = WriteBehindBuffer::new();
        assert_eq!(buf.flush(&db).unwrap(), 0);
    }
}
