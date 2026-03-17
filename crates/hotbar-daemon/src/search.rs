use hotbar_common::types::HotFile;
use rusqlite::{params, OptionalExtension};

use crate::db::{row_to_hotfile, Db, DbError};

/// Index a file in the FTS5 search index.
///
/// Upserts by path: deletes any existing entry first, then inserts.
/// `summary` is optional cached LLM summary text.
pub fn index_file(db: &Db, path: &str, filename: &str, summary: Option<&str>) -> Result<(), DbError> {
    let conn = db.conn();
    conn.execute("DELETE FROM search_index WHERE path = ?1", [path])?;
    conn.execute(
        "INSERT INTO search_index (path, filename, summary_content) VALUES (?1, ?2, ?3)",
        params![path, filename, summary.unwrap_or("")],
    )?;
    Ok(())
}

/// Remove a file from the search index.
pub fn remove_from_index(db: &Db, path: &str) -> Result<bool, DbError> {
    let affected = db
        .conn()
        .execute("DELETE FROM search_index WHERE path = ?1", [path])?;
    Ok(affected > 0)
}

/// Full-text search across tracked files using FTS5.
///
/// Returns HotFiles ranked by BM25 relevance. Empty query returns recent files
/// (delegates to `Db::get_events`).
pub fn search(db: &Db, query: &str, limit: usize) -> Result<Vec<HotFile>, DbError> {
    let query = query.trim();
    if query.is_empty() {
        return db.get_events(None, limit);
    }

    let escaped = escape_fts5_query(query);

    // Phase 1: Get matching paths from FTS5, ranked by BM25
    // bm25() returns negative values; lower (more negative) = better match
    let mut stmt = db.conn().prepare_cached(
        "SELECT path FROM search_index WHERE search_index MATCH ?1
         ORDER BY bm25(search_index) LIMIT ?2",
    )?;

    let paths: Vec<String> = stmt
        .query_map(params![escaped, limit], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    if paths.is_empty() {
        return Ok(vec![]);
    }

    // Phase 2: Get latest event for each matching path, preserving FTS5 rank order
    let mut event_stmt = db.conn().prepare_cached(
        "SELECT path, event_type, source, timestamp, confidence, metadata
         FROM file_events WHERE path = ?1 ORDER BY timestamp DESC LIMIT 1",
    )?;

    let mut results = Vec::with_capacity(paths.len());
    for path in &paths {
        let hotfile = event_stmt
            .query_row(params![path], row_to_hotfile)
            .optional()?;
        if let Some(hf) = hotfile {
            results.push(hf);
        }
    }

    Ok(results)
}

/// Bulk-index all recent events into the FTS5 search index.
///
/// Reads recent HotFiles from the DB and indexes each one, including any
/// cached summary text. Intended for startup or manual re-index.
pub fn rebuild_index(db: &Db, limit: usize) -> Result<usize, DbError> {
    // Clear existing index
    db.conn().execute("DELETE FROM search_index", [])?;

    let files = db.get_events(None, limit)?;
    let mut count = 0;

    for file in &files {
        let summary = db.get_summary(&file.path)?;
        let summary_text = summary.as_ref().map(|s| s.content.as_str());
        index_file(db, &file.path, &file.filename, summary_text)?;
        count += 1;
    }

    tracing::info!(indexed = count, "search index rebuilt");
    Ok(count)
}

/// Escape a user query for safe FTS5 MATCH usage.
///
/// Wraps each whitespace-delimited token in double quotes to prevent
/// FTS5 syntax characters (`*`, `(`, `)`, `:`, `^`, `{`, `}`) from
/// being interpreted as operators.
fn escape_fts5_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|token| {
            let escaped = token.replace('"', "\"\"");
            format!("\"{escaped}\"")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use hotbar_common::types::{Action, Confidence, FileEvent, Source};

    fn test_db() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn insert_test_events(db: &Db) {
        // Insert a session for FK
        db.conn()
            .execute(
                "INSERT INTO sessions (session_id, agent, started_at) VALUES (?1, ?2, ?3)",
                params!["s1", "claude", 1710500000],
            )
            .unwrap();

        let events = vec![
            FileEvent {
                path: "/home/test/dev/main.rs".into(),
                action: Action::Created,
                source: Source::Claude,
                timestamp: 1710500000,
                confidence: Confidence::High,
                session_id: Some("s1".into()),
            },
            FileEvent {
                path: "/home/test/dev/lib.rs".into(),
                action: Action::Modified,
                source: Source::User,
                timestamp: 1710500010,
                confidence: Confidence::High,
                session_id: Some("s1".into()),
            },
            FileEvent {
                path: "/home/test/dev/utils/helpers.py".into(),
                action: Action::Modified,
                source: Source::Codex,
                timestamp: 1710500020,
                confidence: Confidence::Low,
                session_id: None,
            },
        ];

        db.insert_events(&events).unwrap();
    }

    #[test]
    fn index_and_search_basic() {
        let db = test_db();
        insert_test_events(&db);

        index_file(&db, "/home/test/dev/main.rs", "main.rs", None).unwrap();
        index_file(&db, "/home/test/dev/lib.rs", "lib.rs", None).unwrap();
        index_file(
            &db,
            "/home/test/dev/utils/helpers.py",
            "helpers.py",
            None,
        )
        .unwrap();

        let results = search(&db, "main", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, "/home/test/dev/main.rs");
    }

    #[test]
    fn search_by_filename() {
        let db = test_db();
        insert_test_events(&db);

        index_file(&db, "/home/test/dev/main.rs", "main.rs", None).unwrap();
        index_file(
            &db,
            "/home/test/dev/utils/helpers.py",
            "helpers.py",
            Some("Helper functions for data processing"),
        )
        .unwrap();

        let results = search(&db, "helpers", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, "/home/test/dev/utils/helpers.py");
    }

    #[test]
    fn search_by_summary() {
        let db = test_db();
        insert_test_events(&db);

        index_file(
            &db,
            "/home/test/dev/main.rs",
            "main.rs",
            Some("Entry point for the hotbar daemon"),
        )
        .unwrap();
        index_file(&db, "/home/test/dev/lib.rs", "lib.rs", None).unwrap();

        let results = search(&db, "daemon", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, "/home/test/dev/main.rs");
    }

    #[test]
    fn search_empty_query_returns_all() {
        let db = test_db();
        insert_test_events(&db);

        let results = search(&db, "", 10).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn search_no_match() {
        let db = test_db();
        insert_test_events(&db);

        index_file(&db, "/home/test/dev/main.rs", "main.rs", None).unwrap();

        let results = search(&db, "nonexistent", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_special_characters() {
        let db = test_db();
        insert_test_events(&db);

        index_file(&db, "/home/test/dev/main.rs", "main.rs", None).unwrap();

        // FTS5 special chars should be escaped safely
        let results = search(&db, "main* (test)", 10).unwrap();
        // Should not panic; may or may not find results depending on escaping
        assert!(results.len() <= 1);
    }

    #[test]
    fn index_upsert() {
        let db = test_db();
        insert_test_events(&db);

        // Index without summary
        index_file(&db, "/home/test/dev/main.rs", "main.rs", None).unwrap();
        let results = search(&db, "daemon", 10).unwrap();
        assert!(results.is_empty());

        // Re-index with summary
        index_file(
            &db,
            "/home/test/dev/main.rs",
            "main.rs",
            Some("The daemon entry point"),
        )
        .unwrap();
        let results = search(&db, "daemon", 10).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn remove_from_index_basic() {
        let db = test_db();
        insert_test_events(&db);

        index_file(&db, "/home/test/dev/main.rs", "main.rs", None).unwrap();

        assert!(remove_from_index(&db, "/home/test/dev/main.rs").unwrap());
        assert!(!remove_from_index(&db, "/home/test/dev/main.rs").unwrap());

        let results = search(&db, "main", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn rebuild_index_basic() {
        let db = test_db();
        insert_test_events(&db);

        // Add a summary for one file
        db.upsert_summary("/home/test/dev/main.rs", "The daemon entry point", "test")
            .unwrap();

        let count = rebuild_index(&db, 100).unwrap();
        assert_eq!(count, 3);

        // Should find file by summary content
        let results = search(&db, "daemon", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, "/home/test/dev/main.rs");
    }

    #[test]
    fn search_respects_limit() {
        let db = test_db();
        insert_test_events(&db);

        // Index all with common term in path
        index_file(&db, "/home/test/dev/main.rs", "main.rs", Some("dev tool")).unwrap();
        index_file(&db, "/home/test/dev/lib.rs", "lib.rs", Some("dev library")).unwrap();
        index_file(
            &db,
            "/home/test/dev/utils/helpers.py",
            "helpers.py",
            Some("dev helper"),
        )
        .unwrap();

        let results = search(&db, "dev", 2).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn escape_fts5_basic() {
        assert_eq!(escape_fts5_query("hello world"), "\"hello\" \"world\"");
        assert_eq!(escape_fts5_query("main.rs"), "\"main.rs\"");
        assert_eq!(escape_fts5_query("test*"), "\"test*\"");
        assert_eq!(
            escape_fts5_query("has \"quotes\" inside"),
            "\"has\" \"\"\"quotes\"\"\" \"inside\""
        );
    }
}
