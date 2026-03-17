/// Schema version — increment on every breaking DDL change.
/// The migration runner applies DDL incrementally from the stored version.
pub const SCHEMA_VERSION: i32 = 2;

/// Initial DDL for version 1.
/// PRAGMAs are applied separately in db.rs via conn.pragma() — bundled-full
/// SQLite returns result rows for all PRAGMA SET statements.
pub const DDL_V1: &str = "
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    session_id  TEXT PRIMARY KEY,
    agent       TEXT NOT NULL,
    started_at  INTEGER NOT NULL,
    project_root TEXT
);

CREATE TABLE IF NOT EXISTS file_events (
    id          INTEGER PRIMARY KEY,
    session_id  TEXT REFERENCES sessions(session_id),
    path        TEXT NOT NULL,
    event_type  TEXT NOT NULL,
    source      TEXT NOT NULL,
    timestamp   INTEGER NOT NULL,
    confidence  TEXT DEFAULT 'high',
    metadata    TEXT
);

CREATE TABLE IF NOT EXISTS pins (
    path        TEXT PRIMARY KEY,
    label       TEXT,
    pin_group   TEXT DEFAULT 'default',
    position    INTEGER NOT NULL,
    pinned_at   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS summaries (
    path        TEXT PRIMARY KEY,
    content     TEXT NOT NULL,
    model       TEXT NOT NULL,
    cached_at   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS preferences (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS plugin_state (
    plugin TEXT NOT NULL,
    key    TEXT NOT NULL,
    value  TEXT NOT NULL,
    PRIMARY KEY (plugin, key)
);

CREATE INDEX IF NOT EXISTS idx_events_path
    ON file_events(path, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_events_session
    ON file_events(session_id);
CREATE INDEX IF NOT EXISTS idx_events_ts
    ON file_events(timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_events_source
    ON file_events(source, timestamp DESC);

CREATE VIRTUAL TABLE IF NOT EXISTS search_index USING fts5(
    path, filename, summary_content,
    tokenize='unicode61'
);
";

/// v1->v2: Recreate search_index as content-storing.
/// Contentless FTS5 (`content=''`) can't return column values or use bm25().
/// The search index is ephemeral (rebuilt from events+summaries), so data loss
/// from DROP is acceptable.
pub const DDL_V2: &str = "
DROP TABLE IF EXISTS search_index;
CREATE VIRTUAL TABLE IF NOT EXISTS search_index USING fts5(
    path, filename, summary_content,
    tokenize='unicode61'
);
";

/// All migrations in order. Index 0 = v0->v1, index 1 = v1->v2, etc.
pub const MIGRATIONS: &[&str] = &[DDL_V1, DDL_V2];
