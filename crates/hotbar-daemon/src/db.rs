use hotbar_common::schema::{MIGRATIONS, SCHEMA_VERSION};
use hotbar_common::types::{Action, Confidence, FileEvent, HotFile, Pin, Source, Summary};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

/// Database error types
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("database path is not valid UTF-8")]
    InvalidPath,

    #[error("failed to create database directory: {0}")]
    CreateDir(std::io::Error),
}

/// Wrapper around rusqlite providing typed access to hotbar's schema.
///
/// One writer (daemon core), panel reads in-memory state via Arc<RwLock>.
/// The DB is the persistence layer, not the hot path.
pub struct Db {
    conn: Connection,
}

/// Run schema migrations on a connection.
fn run_migrations(conn: &Connection) -> Result<(), rusqlite::Error> {
    // journal_mode returns a result row — use pragma() which accepts a callback.
    // Other PRAGMAs work with pragma_update (no result row).
    // All PRAGMAs use pragma() with callback — bundled-full SQLite returns result
    // rows for several PRAGMAs that bundled does not.
    conn.pragma(None, "journal_mode", "WAL", |_| Ok(()))?;
    conn.pragma(None, "synchronous", "NORMAL", |_| Ok(()))?;
    conn.pragma(None, "foreign_keys", "ON", |_| Ok(()))?;
    conn.pragma(None, "busy_timeout", 1000, |_| Ok(()))?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )?;

    let current_version: i32 = conn
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'schema_version'",
            [],
            |row: &rusqlite::Row| row.get(0),
        )
        .unwrap_or(0);

    for (i, ddl) in MIGRATIONS.iter().enumerate() {
        let migration_version = (i as i32) + 1;
        if migration_version > current_version {
            tracing::info!(
                from = current_version,
                to = migration_version,
                "applying database migration"
            );
            conn.execute_batch(ddl)?;
        }
    }

    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?1)",
        [SCHEMA_VERSION.to_string()],
    )?;

    Ok(())
}

impl Db {
    /// Open (or create) the database at the given path.
    /// Runs schema migrations automatically.
    pub fn open(path: &Path) -> Result<Self, DbError> {
        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent).map_err(DbError::CreateDir)?;
        }

        let conn = Connection::open(path)?;
        run_migrations(&conn)?;
        tracing::info!(path = %path.display(), "database opened");
        Ok(Db { conn })
    }

    /// Open an in-memory database (for testing).
    pub fn open_in_memory() -> Result<Self, DbError> {
        let conn = Connection::open_in_memory()?;
        run_migrations(&conn)?;
        Ok(Db { conn })
    }

    // ─── File Events ─────────────────────────────────────

    /// Insert a batch of file events.
    pub fn insert_events(&self, events: &[FileEvent]) -> Result<usize, DbError> {
        let _span = tracing::debug_span!("db_insert_events", batch_size = events.len()).entered();
        let mut count = 0;
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO file_events (session_id, path, event_type, source, timestamp, confidence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;

        for event in events {
            stmt.execute(params![
                event.session_id,
                event.path,
                event.action.to_string(),
                event.source.to_string(),
                event.timestamp,
                match event.confidence {
                    Confidence::High => "high",
                    Confidence::Low => "low",
                },
            ])?;
            count += 1;
        }

        Ok(count)
    }

    /// Get recent file events, optionally filtered by source.
    /// Returns HotFile structs ready for display.
    pub fn get_events(
        &self,
        source_filter: Option<Source>,
        limit: usize,
    ) -> Result<Vec<HotFile>, DbError> {
        let mut files = Vec::new();

        match source_filter {
            Some(source) => {
                let mut stmt = self.conn.prepare_cached(
                    "SELECT path, event_type, source, timestamp, confidence, metadata
                     FROM file_events
                     WHERE source = ?1
                     ORDER BY timestamp DESC
                     LIMIT ?2",
                )?;
                let rows = stmt.query_map(params![source.to_string(), limit], row_to_hotfile)?;
                for row in rows {
                    match row {
                        Ok(file) => files.push(file),
                        Err(e) => tracing::warn!("skipping malformed event row: {e}"),
                    }
                }
            }
            None => {
                let mut stmt = self.conn.prepare_cached(
                    "SELECT path, event_type, source, timestamp, confidence, metadata
                     FROM file_events
                     ORDER BY timestamp DESC
                     LIMIT ?1",
                )?;
                let rows = stmt.query_map(params![limit], row_to_hotfile)?;
                for row in rows {
                    match row {
                        Ok(file) => files.push(file),
                        Err(e) => tracing::warn!("skipping malformed event row: {e}"),
                    }
                }
            }
        }

        Ok(files)
    }

    // ─── Pins ────────────────────────────────────────────

    /// Insert or update a pin.
    pub fn upsert_pin(&self, pin: &Pin) -> Result<(), DbError> {
        tracing::debug!(path = %pin.path, "db upsert pin");
        self.conn.execute(
            "INSERT OR REPLACE INTO pins (path, label, pin_group, position, pinned_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![pin.path, pin.label, pin.pin_group, pin.position, pin.pinned_at],
        )?;
        Ok(())
    }

    /// Remove a pin by path.
    pub fn remove_pin(&self, path: &str) -> Result<bool, DbError> {
        tracing::debug!(path, "db remove pin");
        let affected = self.conn.execute("DELETE FROM pins WHERE path = ?1", [path])?;
        Ok(affected > 0)
    }

    /// Get all pins, ordered by position.
    pub fn get_pins(&self) -> Result<Vec<Pin>, DbError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT path, label, pin_group, position, pinned_at
             FROM pins ORDER BY position ASC",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(Pin {
                path: row.get(0)?,
                label: row.get(1)?,
                pin_group: row.get::<_, Option<String>>(2)?.unwrap_or("default".into()),
                position: row.get(3)?,
                pinned_at: row.get(4)?,
            })
        })?;

        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    // ─── Summaries ───────────────────────────────────────

    /// Insert or update a cached summary.
    pub fn upsert_summary(&self, path: &str, content: &str, model: &str) -> Result<(), DbError> {
        tracing::debug!(path, model, "db upsert summary");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        self.conn.execute(
            "INSERT OR REPLACE INTO summaries (path, content, model, cached_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![path, content, model, now],
        )?;
        Ok(())
    }

    /// Get cached summary for a file, if it exists.
    pub fn get_summary(&self, path: &str) -> Result<Option<Summary>, DbError> {
        let result = self
            .conn
            .query_row(
                "SELECT path, content, model, cached_at FROM summaries WHERE path = ?1",
                [path],
                |row| {
                    Ok(Summary {
                        path: row.get(0)?,
                        content: row.get(1)?,
                        model: row.get(2)?,
                        cached_at: row.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(result)
    }

    // ─── Preferences ─────────────────────────────────────

    /// Set a preference (JSON value).
    pub fn set_preference(&self, key: &str, value_json: &str) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO preferences (key, value) VALUES (?1, ?2)",
            params![key, value_json],
        )?;
        Ok(())
    }

    /// Get a preference value (JSON string).
    pub fn get_preference(&self, key: &str) -> Result<Option<String>, DbError> {
        let result = self
            .conn
            .query_row(
                "SELECT value FROM preferences WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(result)
    }

    // ─── Plugin State ────────────────────────────────────

    /// Set plugin-specific state.
    pub fn set_plugin_state(
        &self,
        plugin: &str,
        key: &str,
        value: &str,
    ) -> Result<(), DbError> {
        tracing::debug!(plugin, key, "db set plugin state");
        self.conn.execute(
            "INSERT OR REPLACE INTO plugin_state (plugin, key, value) VALUES (?1, ?2, ?3)",
            params![plugin, key, value],
        )?;
        Ok(())
    }

    /// Get plugin-specific state.
    pub fn get_plugin_state(&self, plugin: &str, key: &str) -> Result<Option<String>, DbError> {
        let result = self
            .conn
            .query_row(
                "SELECT value FROM plugin_state WHERE plugin = ?1 AND key = ?2",
                params![plugin, key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(result)
    }

    /// Get the underlying connection (for advanced queries / testing)
    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}

/// Parse a file_events row into a HotFile.
///
/// Expected column order: path, event_type, source, timestamp, confidence, metadata
pub(crate) fn row_to_hotfile(row: &rusqlite::Row) -> Result<HotFile, rusqlite::Error> {
    let path: String = row.get(0)?;
    let event_type: String = row.get(1)?;
    let source_str: String = row.get(2)?;
    let timestamp: i64 = row.get(3)?;
    let confidence_str: String = row.get::<_, Option<String>>(4)?.unwrap_or("high".into());
    let metadata: Option<String> = row.get(5)?;

    let (filename, dir, full_dir) = split_path(&path);
    let mime_type = guess_mime(&filename);

    Ok(HotFile {
        path,
        filename,
        dir,
        full_dir,
        timestamp,
        source: parse_source(&source_str),
        mime_type,
        action: parse_action(&event_type),
        confidence: if confidence_str == "low" {
            Confidence::Low
        } else {
            Confidence::High
        },
        metadata,
    })
}

pub(crate) fn split_path(path: &str) -> (String, String, String) {
    let (filename, full_dir) = match path.rfind('/') {
        Some(pos) => (path[pos + 1..].to_string(), path[..pos].to_string()),
        None => (path.to_string(), String::new()),
    };

    // Shorten for display: replace $HOME with ~
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = if full_dir.starts_with(&home) {
        format!("~{}", &full_dir[home.len()..])
    } else {
        full_dir.clone()
    };

    // Truncate middle if too long
    let dir = if dir.len() > 40 {
        let parts: Vec<&str> = dir.split('/').collect();
        if parts.len() > 4 {
            format!(
                "{}/.../{}/{}",
                parts[..2].join("/"),
                parts[parts.len() - 2],
                parts[parts.len() - 1]
            )
        } else {
            dir
        }
    } else {
        dir
    };

    (filename, dir, full_dir)
}

fn parse_source(s: &str) -> Source {
    match s {
        "claude" => Source::Claude,
        "codex" => Source::Codex,
        "user" => Source::User,
        "system" => Source::System,
        _ => Source::User,
    }
}

fn parse_action(s: &str) -> Action {
    match s {
        "opened" => Action::Opened,
        "modified" => Action::Modified,
        "created" => Action::Created,
        "deleted" => Action::Deleted,
        _ => Action::Modified,
    }
}

pub(crate) fn guess_mime(filename: &str) -> String {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "rs" => "text/x-rust",
        "ts" | "tsx" => "text/typescript",
        "js" | "jsx" => "text/javascript",
        "py" => "text/x-python",
        "go" => "text/x-go",
        "rb" => "text/x-ruby",
        "sh" | "bash" | "zsh" => "application/x-shellscript",
        "json" => "application/json",
        "toml" => "application/toml",
        "yaml" | "yml" => "application/yaml",
        "md" => "text/markdown",
        "txt" => "text/plain",
        "html" => "text/html",
        "css" | "scss" => "text/css",
        "sql" => "application/sql",
        "xml" => "application/xml",
        "nix" => "text/x-nix",
        "lua" => "text/x-lua",
        "c" | "h" => "text/x-c",
        "cpp" | "hpp" | "cc" => "text/x-c++",
        "java" => "text/x-java",
        "kt" => "text/x-kotlin",
        "swift" => "text/x-swift",
        "conf" | "cfg" | "ini" | "env" => "text/plain",
        "lock" => "text/plain",
        "php" => "text/x-php",
        "vue" => "text/x-vue",
        "svelte" => "text/x-svelte",
        "wgsl" | "glsl" | "hlsl" => "text/x-shader",
        _ => "text/plain",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hotbar_common::types::Confidence;

    fn test_db() -> Db {
        Db::open_in_memory().unwrap()
    }

    #[test]
    fn open_in_memory() {
        let db = test_db();
        // Should have schema initialized
        let version: i32 = db
            .conn()
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, hotbar_common::schema::SCHEMA_VERSION);
    }

    #[test]
    fn insert_and_get_events() {
        let db = test_db();

        // Insert a session so FK constraint is satisfied
        db.conn()
            .execute(
                "INSERT INTO sessions (session_id, agent, started_at) VALUES (?1, ?2, ?3)",
                rusqlite::params!["sess1", "claude", 1710500000],
            )
            .unwrap();

        let events = vec![
            FileEvent {
                path: "/home/zack/dev/hotbar/main.rs".into(),
                action: Action::Created,
                source: Source::Claude,
                timestamp: 1710500000,
                confidence: Confidence::High,
                session_id: Some("sess1".into()),
            },
            FileEvent {
                path: "/home/zack/dev/hotbar/lib.rs".into(),
                action: Action::Modified,
                source: Source::User,
                timestamp: 1710500010,
                confidence: Confidence::Low,
                session_id: None,
            },
        ];

        let count = db.insert_events(&events).unwrap();
        assert_eq!(count, 2);

        // Get all events
        let files = db.get_events(None, 10).unwrap();
        assert_eq!(files.len(), 2);
        // Sorted by timestamp DESC
        assert_eq!(files[0].filename, "lib.rs");
        assert_eq!(files[1].filename, "main.rs");

        // Filter by source
        let claude_files = db.get_events(Some(Source::Claude), 10).unwrap();
        assert_eq!(claude_files.len(), 1);
        assert_eq!(claude_files[0].filename, "main.rs");
        assert_eq!(claude_files[0].action, Action::Created);
        assert_eq!(claude_files[0].confidence, Confidence::High);
    }

    #[test]
    fn pin_lifecycle() {
        let db = test_db();

        // No pins initially
        assert!(db.get_pins().unwrap().is_empty());

        // Upsert a pin
        let pin = Pin {
            path: "/home/zack/dev/hotbar/main.rs".into(),
            label: Some("entry point".into()),
            pin_group: "default".into(),
            position: 0,
            pinned_at: 1710500000,
        };
        db.upsert_pin(&pin).unwrap();

        let pins = db.get_pins().unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].path, "/home/zack/dev/hotbar/main.rs");
        assert_eq!(pins[0].label, Some("entry point".into()));

        // Update label
        let updated = Pin {
            label: Some("main entry".into()),
            ..pin.clone()
        };
        db.upsert_pin(&updated).unwrap();
        let pins = db.get_pins().unwrap();
        assert_eq!(pins.len(), 1); // still 1, not 2
        assert_eq!(pins[0].label, Some("main entry".into()));

        // Remove
        assert!(db.remove_pin("/home/zack/dev/hotbar/main.rs").unwrap());
        assert!(db.get_pins().unwrap().is_empty());

        // Remove non-existent
        assert!(!db.remove_pin("/nonexistent").unwrap());
    }

    #[test]
    fn pin_ordering() {
        let db = test_db();

        for i in (0..3).rev() {
            db.upsert_pin(&Pin {
                path: format!("/file{i}"),
                label: None,
                pin_group: "default".into(),
                position: i,
                pinned_at: 1710500000,
            })
            .unwrap();
        }

        let pins = db.get_pins().unwrap();
        assert_eq!(pins[0].path, "/file0");
        assert_eq!(pins[1].path, "/file1");
        assert_eq!(pins[2].path, "/file2");
    }

    #[test]
    fn summary_lifecycle() {
        let db = test_db();

        // No summary initially
        assert!(db.get_summary("/test.rs").unwrap().is_none());

        // Upsert
        db.upsert_summary("/test.rs", "A test file.", "qwen2.5")
            .unwrap();

        let summary = db.get_summary("/test.rs").unwrap().unwrap();
        assert_eq!(summary.content, "A test file.");
        assert_eq!(summary.model, "qwen2.5");
        assert!(summary.cached_at > 0);

        // Update
        db.upsert_summary("/test.rs", "Updated summary.", "phi-3")
            .unwrap();
        let summary = db.get_summary("/test.rs").unwrap().unwrap();
        assert_eq!(summary.content, "Updated summary.");
        assert_eq!(summary.model, "phi-3");
    }

    #[test]
    fn preference_lifecycle() {
        let db = test_db();

        // No preference initially
        assert!(db.get_preference("theme").unwrap().is_none());

        // Set
        db.set_preference("theme", r#""lumon""#).unwrap();
        assert_eq!(db.get_preference("theme").unwrap().unwrap(), r#""lumon""#);

        // Update
        db.set_preference("theme", r#""catppuccin""#).unwrap();
        assert_eq!(
            db.get_preference("theme").unwrap().unwrap(),
            r#""catppuccin""#
        );
    }

    #[test]
    fn plugin_state_lifecycle() {
        let db = test_db();

        assert!(db.get_plugin_state("git", "branch").unwrap().is_none());

        db.set_plugin_state("git", "branch", "main").unwrap();
        assert_eq!(
            db.get_plugin_state("git", "branch").unwrap().unwrap(),
            "main"
        );

        // Different plugin, same key
        db.set_plugin_state("lsp", "branch", "other").unwrap();
        assert_eq!(
            db.get_plugin_state("git", "branch").unwrap().unwrap(),
            "main"
        );
        assert_eq!(
            db.get_plugin_state("lsp", "branch").unwrap().unwrap(),
            "other"
        );
    }

    #[test]
    fn split_path_basic() {
        let (filename, _dir, full_dir) = split_path("/home/zack/dev/hotbar/main.rs");
        assert_eq!(filename, "main.rs");
        assert_eq!(full_dir, "/home/zack/dev/hotbar");
    }

    #[test]
    fn guess_mime_known() {
        assert_eq!(guess_mime("main.rs"), "text/x-rust");
        assert_eq!(guess_mime("app.tsx"), "text/typescript");
        assert_eq!(guess_mime("style.scss"), "text/css");
        assert_eq!(guess_mime("flames.wgsl"), "text/x-shader");
    }

    #[test]
    fn guess_mime_unknown() {
        assert_eq!(guess_mime("mystery"), "text/plain");
    }

    #[test]
    fn parse_source_valid() {
        assert_eq!(parse_source("claude"), Source::Claude);
        assert_eq!(parse_source("codex"), Source::Codex);
        assert_eq!(parse_source("user"), Source::User);
        assert_eq!(parse_source("system"), Source::System);
    }

    #[test]
    fn parse_source_unknown_defaults_user() {
        assert_eq!(parse_source("unknown"), Source::User);
    }

    #[test]
    fn parse_action_valid() {
        assert_eq!(parse_action("opened"), Action::Opened);
        assert_eq!(parse_action("created"), Action::Created);
    }

    #[test]
    fn parse_action_unknown_defaults_modified() {
        assert_eq!(parse_action("unknown"), Action::Modified);
    }
}
