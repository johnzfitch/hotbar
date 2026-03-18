use std::collections::HashMap;
use std::path::{Path, PathBuf};

use hotbar_common::types::{Action, Confidence, FileEvent, Source};

use super::{
    home_dir, include_system_events, is_under_home, parse_iso8601, should_skip_path,
    source_for_path, unix_now, IngestError, SKIP_FILES,
};

/// Parser for Codex session JSONL files.
///
/// Codex stores one file per session in `~/.codex/sessions/YYYY/MM/DD/*.jsonl`.
/// Each line is a JSON object with an ISO 8601 `timestamp` field.
/// Write events use `apply_patch` with patch headers containing file paths.
pub struct CodexWatcher {
    sessions_dir: PathBuf,
    /// Per-file cursor: path -> last offset read
    file_offsets: HashMap<PathBuf, u64>,
    home: String,
    include_system: bool,
}

impl Default for CodexWatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexWatcher {
    /// Create a new watcher for the default Codex sessions directory.
    pub fn new() -> Self {
        let home = home_dir();
        let sessions_dir = PathBuf::from(format!("{home}/.codex/sessions"));
        Self {
            sessions_dir,
            file_offsets: HashMap::new(),
            home,
            include_system: include_system_events(),
        }
    }

    /// Create a watcher with an explicit sessions directory (for testing).
    pub fn with_dir(sessions_dir: PathBuf, home: String) -> Self {
        Self {
            sessions_dir,
            file_offsets: HashMap::new(),
            home,
            include_system: false,
        }
    }

    /// Read new events from recent Codex session files.
    ///
    /// Scans today + yesterday's session directories, sorted by mtime (most recent first),
    /// capped at 20 files. Returns events within the 24h window.
    pub fn read_new(&mut self) -> Result<Vec<FileEvent>, IngestError> {
        let _span = tracing::debug_span!("codex_ingest").entered();
        let session_files = self.find_session_files();
        if session_files.is_empty() {
            tracing::debug!("no recent Codex session files found");
            return Ok(vec![]);
        }

        let now = unix_now();
        let cutoff = now - 86400;
        let mut all_events = Vec::new();

        for path in &session_files {
            match self.parse_session_file(path, now, cutoff) {
                Ok(events) => all_events.extend(events),
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "failed to parse Codex session");
                }
            }
        }

        tracing::debug!(
            files = session_files.len(),
            events = all_events.len(),
            "codex watcher: read complete"
        );

        Ok(all_events)
    }

    /// Find recent session JSONL files (today + yesterday, mtime-sorted, capped at 20).
    fn find_session_files(&self) -> Vec<PathBuf> {
        let now = unix_now();
        let mut candidates: Vec<(PathBuf, i64)> = Vec::new();

        for day_offset in 0..=1 {
            let day_ts = now - day_offset * 86400;
            let dir = date_dir(&self.sessions_dir, day_ts);

            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let mtime = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                candidates.push((path, mtime));
            }
        }

        // Sort by mtime descending (most recent first), cap at 20
        candidates.sort_by(|a, b| b.1.cmp(&a.1));
        candidates.truncate(20);
        candidates.into_iter().map(|(p, _)| p).collect()
    }

    /// Parse a single session JSONL file for apply_patch events.
    fn parse_session_file(
        &mut self,
        path: &Path,
        now: i64,
        cutoff: i64,
    ) -> Result<Vec<FileEvent>, IngestError> {
        let metadata = std::fs::metadata(path)?;
        let file_size = metadata.len();

        // Check if we have a stored offset for this file
        let stored_offset = self.file_offsets.get(path).copied().unwrap_or(0);

        // If file hasn't grown, skip processing
        if stored_offset > 0 && file_size == stored_offset {
            return Ok(vec![]);
        }

        // If file is smaller than stored offset, reset (file was truncated)
        let start_offset = if file_size < stored_offset {
            0
        } else {
            stored_offset
        };

        let mut file = std::fs::File::open(path)?;
        let content = if start_offset == 0 {
            std::fs::read_to_string(path)?
        } else {
            use std::io::{Seek, SeekFrom, Read};
            file.seek(SeekFrom::Start(start_offset))?;
            let mut buf = String::new();
            file.read_to_string(&mut buf)?;
            buf
        };

        let mut events: Vec<FileEvent> = Vec::new();
        // Track most recent event per path within this session
        let mut seen: HashMap<String, usize> = HashMap::new();

        for line in content.lines() {
            let parsed: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Only response_item type
            if parsed.get("type").and_then(|v| v.as_str()) != Some("response_item") {
                continue;
            }

            let payload = match parsed.get("payload") {
                Some(p) => p,
                None => continue,
            };

            // Check for apply_patch (custom_tool_call or function_call)
            let payload_type = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("");

            let is_apply_patch = (payload_type == "custom_tool_call"
                || payload_type == "function_call")
                && name == "apply_patch";
            if !is_apply_patch {
                continue;
            }

            // Extract patch text
            let patch_text = self.extract_patch_text(payload);
            if patch_text.is_empty() {
                continue;
            }

            // Parse ISO timestamp
            let row_ts = parsed
                .get("timestamp")
                .and_then(|v| v.as_str())
                .and_then(parse_iso8601);
            let row_ts = match row_ts {
                Some(ts) if ts >= cutoff && ts <= now + 60 => ts,
                _ => continue,
            };

            // Extract file paths from patch headers
            for (operation, file_path) in extract_patch_paths(&patch_text) {
                if !file_path.starts_with('/') {
                    tracing::debug!(path = file_path, "skipping relative path in Codex patch");
                    continue;
                }

                if !is_under_home(&file_path, &self.home) {
                    continue;
                }

                let source = match source_for_path(
                    &file_path,
                    &self.home,
                    Source::Codex,
                    self.include_system,
                ) {
                    Some(s) => s,
                    None => continue,
                };

                if should_skip_path(&file_path) {
                    continue;
                }

                let basename = file_path.rsplit('/').next().unwrap_or("");
                if SKIP_FILES.contains(&basename) {
                    continue;
                }

                let action = match operation {
                    PatchOp::Add => Action::Created,
                    PatchOp::Delete => Action::Deleted,
                    PatchOp::Update => Action::Modified,
                };

                let event = FileEvent {
                    path: file_path.clone(),
                    action,
                    source,
                    timestamp: row_ts,
                    confidence: Confidence::High,
                    session_id: None,
                };

                // Dedup: keep most recent per path
                if let Some(&idx) = seen.get(&file_path) {
                    if row_ts > events[idx].timestamp {
                        events[idx] = event;
                    }
                } else {
                    seen.insert(file_path, events.len());
                    events.push(event);
                }
            }
        }

        // Update cursor offset for this file
        self.file_offsets
            .insert(path.to_path_buf(), file_size);

        Ok(events)
    }

    /// Extract patch text from a payload object.
    fn extract_patch_text(&self, payload: &serde_json::Value) -> String {
        // custom_tool_call: patch text in 'input'
        if let Some(input) = payload.get("input").and_then(|v| v.as_str()) {
            return input.to_string();
        }

        // function_call: patch text in 'arguments' (may be JSON-stringified)
        if let Some(args) = payload.get("arguments")
            && let Some(args_str) = args.as_str()
        {
            // Try parsing as JSON to extract patch/input field
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(args_str)
                && let Some(patch) = parsed
                    .get("patch")
                    .or_else(|| parsed.get("input"))
                    .and_then(|v| v.as_str())
            {
                return patch.to_string();
            }
            // Fall back to raw string
            return args_str.to_string();
        }

        String::new()
    }
}

#[derive(Debug, Clone, Copy)]
enum PatchOp {
    Update,
    Add,
    Delete,
}

/// Extract file paths from patch header lines.
/// Format: `*** Update File: /path/to/file`
///         `*** Add File: /path/to/file`
///         `*** Delete File: /path/to/file`
fn extract_patch_paths(patch_text: &str) -> Vec<(PatchOp, String)> {
    let mut results = Vec::new();
    for line in patch_text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("*** Update File: ") {
            results.push((PatchOp::Update, rest.trim().to_string()));
        } else if let Some(rest) = trimmed.strip_prefix("*** Add File: ") {
            results.push((PatchOp::Add, rest.trim().to_string()));
        } else if let Some(rest) = trimmed.strip_prefix("*** Delete File: ") {
            results.push((PatchOp::Delete, rest.trim().to_string()));
        }
    }
    results
}

/// Build the session directory path for a given Unix timestamp.
/// Format: `sessions_dir/YYYY/MM/DD`
fn date_dir(sessions_dir: &Path, timestamp: i64) -> PathBuf {
    // Convert Unix timestamp to (year, month, day)
    // Using Howard Hinnant's days_to_civil algorithm (inverse of civil_from_days)
    let z = timestamp / 86400 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    sessions_dir.join(format!("{y:04}/{m:02}/{d:02}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn setup_session_dir(base: &Path, date: &str) -> PathBuf {
        let dir = base.join(date);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn extract_patch_paths_basic() {
        let patch = "*** Update File: /home/test/src/main.rs\n@@ -1,3 +1,3 @@\n-old\n+new\n";
        let paths = extract_patch_paths(patch);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].1, "/home/test/src/main.rs");
        assert!(matches!(paths[0].0, PatchOp::Update));
    }

    #[test]
    fn extract_patch_paths_multiple() {
        let patch = "*** Add File: /home/test/new.rs\nsome content\n*** Delete File: /home/test/old.rs\n*** Update File: /home/test/mod.rs\n";
        let paths = extract_patch_paths(patch);
        assert_eq!(paths.len(), 3);
        assert!(matches!(paths[0].0, PatchOp::Add));
        assert_eq!(paths[0].1, "/home/test/new.rs");
        assert!(matches!(paths[1].0, PatchOp::Delete));
        assert!(matches!(paths[2].0, PatchOp::Update));
    }

    #[test]
    fn date_dir_computes_correct_path() {
        let base = PathBuf::from("/sessions");
        // 2026-03-15 = some known timestamp
        // Let's use a known date: 2024-01-01T00:00:00Z = 1704067200
        let dir = date_dir(&base, 1704067200);
        assert_eq!(dir, PathBuf::from("/sessions/2024/01/01"));
    }

    #[test]
    fn parse_session_file_apply_patch() {
        let tmp = tempfile::tempdir().unwrap();
        let home = "/home/test";
        let now = unix_now();
        // Create a timestamp string close to now
        let ts_str = format_iso8601(now - 60);

        let session_dir = setup_session_dir(tmp.path(), "2026/03/15");
        let session_file = session_dir.join("session1.jsonl");
        {
            let mut f = std::fs::File::create(&session_file).unwrap();
            writeln!(
                f,
                r#"{{"type":"response_item","payload":{{"type":"custom_tool_call","name":"apply_patch","input":"*** Update File: {home}/src/main.rs\n@@ -1 +1 @@\n-old\n+new"}},"timestamp":"{ts_str}"}}"#,
            )
            .unwrap();
        }

        let mut watcher = CodexWatcher::with_dir(tmp.path().to_path_buf(), home.into());
        // Override sessions_dir to point to our temp dir
        watcher.sessions_dir = tmp.path().to_path_buf();

        let events = watcher
            .parse_session_file(&session_file, now, now - 86400)
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].path, format!("{home}/src/main.rs"));
        assert_eq!(events[0].action, Action::Modified);
        assert_eq!(events[0].source, Source::Codex);
    }

    #[test]
    fn parse_session_file_add_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let home = "/home/test";
        let now = unix_now();
        let ts_str = format_iso8601(now - 30);

        let session_file = tmp.path().join("session.jsonl");
        {
            let mut f = std::fs::File::create(&session_file).unwrap();
            writeln!(
                f,
                r#"{{"type":"response_item","payload":{{"type":"custom_tool_call","name":"apply_patch","input":"*** Add File: {home}/new.rs\ncontent\n*** Delete File: {home}/old.rs"}},"timestamp":"{ts_str}"}}"#,
            )
            .unwrap();
        }

        let mut watcher = CodexWatcher::with_dir(tmp.path().to_path_buf(), home.into());
        let events = watcher
            .parse_session_file(&session_file, now, now - 86400)
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].action, Action::Created);
        assert_eq!(events[1].action, Action::Deleted);
    }

    #[test]
    fn skips_non_apply_patch() {
        let tmp = tempfile::tempdir().unwrap();
        let home = "/home/test";
        let now = unix_now();
        let ts_str = format_iso8601(now - 10);

        let session_file = tmp.path().join("session.jsonl");
        {
            let mut f = std::fs::File::create(&session_file).unwrap();
            // Not apply_patch
            writeln!(
                f,
                r#"{{"type":"response_item","payload":{{"type":"custom_tool_call","name":"exec_command","input":"ls"}},"timestamp":"{ts_str}"}}"#,
            )
            .unwrap();
            // Not response_item
            writeln!(
                f,
                r#"{{"type":"session_start","timestamp":"{ts_str}"}}"#,
            )
            .unwrap();
        }

        let mut watcher = CodexWatcher::with_dir(tmp.path().to_path_buf(), home.into());
        let events = watcher
            .parse_session_file(&session_file, now, now - 86400)
            .unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn read_new_no_sessions_dir() {
        let mut watcher =
            CodexWatcher::with_dir("/nonexistent/sessions".into(), "/home/test".into());
        let events = watcher.read_new().unwrap();
        assert!(events.is_empty());
    }

    /// Helper to format a Unix timestamp as ISO 8601
    fn format_iso8601(ts: i64) -> String {
        // Inverse of parse_iso8601: convert Unix timestamp to ISO string
        let total_days = ts / 86400;
        let day_secs = ts % 86400;
        let h = day_secs / 3600;
        let m = (day_secs % 3600) / 60;
        let s = day_secs % 60;

        // days_to_civil (Howard Hinnant)
        let z = total_days + 719468;
        let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let mo = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if mo <= 2 { y + 1 } else { y };

        format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}.000Z")
    }
}
