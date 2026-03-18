use std::collections::HashSet;
use std::io::{BufRead, Seek, SeekFrom};
use std::path::PathBuf;

use hotbar_common::types::{Action, Confidence, FileEvent, Source};

use super::{
    home_dir, include_system_events, is_under_home, should_skip_path, source_for_path, unix_now,
    IngestError,
};

/// Tool names we extract file paths from
const TOOL_ACTIONS: &[&str] = &["Read", "Write", "Edit", "NotebookEdit"];

/// Cursor-based parser for Claude Code's `events.jsonl`.
///
/// The file is append-only and accumulates events across multiple sessions.
/// Timestamps are **relative** to each session's start (seconds since session began).
/// Session boundaries are detected where a timestamp decreases by >60s from the
/// previous line.
///
/// `read_new()` reads from the last offset, only returning new events.
/// On inode change (file rotation), the cursor resets and does a full re-read.
pub struct ClaudeCursor {
    path: PathBuf,
    last_offset: u64,
    last_inode: u64,
    /// Base time (Unix seconds) for the current (most recent) session
    current_session_base_time: i64,
    /// Maximum relative timestamp seen in the current session
    current_session_max_ts: f64,
    /// Previous line's relative timestamp (for boundary detection in incremental reads)
    prev_relative_ts: f64,
    /// Paths whose first write-type event was Write (→ "created")
    created_paths: HashSet<String>,
    /// All paths seen so far (for created vs modified detection)
    seen_paths: HashSet<String>,
    home: String,
    include_system: bool,
}

impl ClaudeCursor {
    /// Create a new cursor for the given events.jsonl path.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            last_offset: 0,
            last_inode: 0,
            current_session_base_time: 0,
            current_session_max_ts: 0.0,
            prev_relative_ts: 0.0,
            created_paths: HashSet::new(),
            seen_paths: HashSet::new(),
            home: home_dir(),
            include_system: include_system_events(),
        }
    }

    /// Create a cursor with explicit home dir (for testing).
    pub fn with_home(path: PathBuf, home: String) -> Self {
        Self {
            home,
            ..Self::new(path)
        }
    }

    /// Read new events since the last call.
    ///
    /// On first call, processes the entire file. On subsequent calls, reads only
    /// new bytes appended since the last offset. Returns events within the 24h window.
    pub fn read_new(&mut self) -> Result<Vec<FileEvent>, IngestError> {
        let _span = tracing::debug_span!("claude_ingest").entered();
        let metadata = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(path = %self.path.display(), "events.jsonl not found");
                return Ok(vec![]);
            }
            Err(e) => return Err(e.into()),
        };

        let file_size = metadata.len();
        if file_size == 0 {
            return Ok(vec![]);
        }

        // Detect inode change (file rotation)
        #[cfg(unix)]
        let current_inode = {
            use std::os::unix::fs::MetadataExt;
            metadata.ino()
        };
        #[cfg(not(unix))]
        let current_inode = 0u64;

        if current_inode != self.last_inode && self.last_inode != 0 {
            tracing::info!("events.jsonl inode changed, performing full re-read");
            self.reset();
        }
        self.last_inode = current_inode;

        // Nothing new to read
        if file_size <= self.last_offset {
            return Ok(vec![]);
        }

        // Get file mtime for baseTime calculation
        let file_mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or_else(unix_now);

        let is_first_read = self.last_offset == 0;

        if is_first_read {
            self.read_full(file_mtime)
        } else {
            self.read_incremental(file_mtime)
        }
    }

    /// Full re-read of the entire file (first call or after rotation).
    fn read_full(&mut self, file_mtime: i64) -> Result<Vec<FileEvent>, IngestError> {
        let content = std::fs::read_to_string(&self.path)?;
        let lines: Vec<&str> = content.lines().collect();

        if lines.is_empty() {
            self.last_offset = content.len() as u64;
            return Ok(vec![]);
        }

        // Detect session boundaries
        let mut boundaries: Vec<usize> = vec![0];
        let mut prev_ts: f64 = 0.0;

        for (i, line) in lines.iter().enumerate() {
            if let Some(ts) = extract_timestamp(line) {
                if ts < prev_ts - 60.0 {
                    boundaries.push(i);
                }
                prev_ts = ts;
            }
        }

        // Build session segments
        let mut segments: Vec<SessionSegment> = Vec::new();
        for (s, &start) in boundaries.iter().enumerate() {
            let end = boundaries.get(s + 1).copied().unwrap_or(lines.len());
            let max_ts = find_max_relative_ts(&lines[start..end]);
            segments.push(SessionSegment {
                start,
                end,
                max_ts,
            });
        }

        // Compute baseTimes — last segment anchors to fileMtime
        let mut base_times: Vec<i64> = vec![0; segments.len()];
        if let Some(last) = segments.last() {
            let last_idx = segments.len() - 1;
            base_times[last_idx] = file_mtime - last.max_ts as i64;

            // Walk backwards to derive earlier segments' baseTimes
            for s in (0..last_idx).rev() {
                let next_first_ts =
                    find_first_relative_ts(&lines[segments[s + 1].start..segments[s + 1].end]);
                let next_abs_start = base_times[s + 1] + next_first_ts as i64;
                base_times[s] = next_abs_start - segments[s].max_ts as i64;
            }
        }

        // Store current session state for future incremental reads
        if let Some(last) = segments.last() {
            self.current_session_base_time = base_times[segments.len() - 1];
            self.current_session_max_ts = last.max_ts;
        }
        self.prev_relative_ts = prev_ts;

        // Process events from all sessions within 24h window
        let now = unix_now();
        let cutoff = now - 86400;
        let mut events = Vec::new();

        for (s, seg) in segments.iter().enumerate() {
            let base_time = base_times[s];
            let seg_abs_end = base_time + seg.max_ts as i64;

            // Skip sessions that ended more than 24h ago
            if seg_abs_end < cutoff {
                continue;
            }

            for line in &lines[seg.start..seg.end] {
                if let Some(evt) = self.parse_event_line(line, base_time, now, cutoff) {
                    events.push(evt);
                }
            }
        }

        // Preserve "created" lifecycle
        self.restore_created_actions(&mut events);

        self.last_offset = content.len() as u64;

        tracing::debug!(
            sessions = segments.len(),
            events = events.len(),
            "claude cursor: full read complete"
        );

        Ok(events)
    }

    /// Incremental read — only new bytes since last offset.
    fn read_incremental(&mut self, file_mtime: i64) -> Result<Vec<FileEvent>, IngestError> {
        let mut file = std::fs::File::open(&self.path)?;
        file.seek(SeekFrom::Start(self.last_offset))?;

        let reader = std::io::BufReader::new(&file);
        let now = unix_now();
        let cutoff = now - 86400;
        let mut events = Vec::new();
        let mut bytes_read = 0u64;

        for line_result in reader.lines() {
            let line = line_result?;
            bytes_read += line.len() as u64 + 1; // +1 for newline

            // Check for session boundary
            if let Some(ts) = extract_timestamp(&line) {
                if ts < self.prev_relative_ts - 60.0 {
                    // New session started — recompute baseTime for the new session
                    // The previous session's data is already committed.
                    // New session's baseTime will be derived from fileMtime
                    // once we know its maxRelativeTs.
                    self.current_session_max_ts = 0.0;
                    tracing::debug!("claude cursor: new session boundary detected");
                }
                if ts > self.current_session_max_ts {
                    self.current_session_max_ts = ts;
                }
                self.prev_relative_ts = ts;
            }

            // Recompute baseTime for current session
            self.current_session_base_time =
                file_mtime - self.current_session_max_ts as i64;

            if let Some(evt) =
                self.parse_event_line(&line, self.current_session_base_time, now, cutoff)
            {
                events.push(evt);
            }
        }

        self.restore_created_actions(&mut events);
        self.last_offset += bytes_read;

        tracing::debug!(
            new_events = events.len(),
            offset = self.last_offset,
            "claude cursor: incremental read complete"
        );

        Ok(events)
    }

    /// Parse a single JSON line into a FileEvent, applying all filters.
    fn parse_event_line(
        &mut self,
        line: &str,
        base_time: i64,
        now: i64,
        cutoff: i64,
    ) -> Option<FileEvent> {
        let parsed: serde_json::Value = serde_json::from_str(line).ok()?;

        let tool = parsed.get("tool")?.as_str()?;
        if !TOOL_ACTIONS.contains(&tool) {
            return None;
        }

        let path = parsed.get("original_cmd")?.as_str()?;
        if !is_under_home(path, &self.home) {
            return None;
        }

        let source = source_for_path(path, &self.home, Source::Claude, self.include_system)?;

        if should_skip_path(path) {
            return None;
        }

        let relative_ts = parsed.get("timestamp")?.as_f64()?;
        let absolute_ts = base_time + relative_ts as i64;

        // Skip events with obviously wrong timestamps
        if absolute_ts > now + 60 || absolute_ts < cutoff {
            return None;
        }

        // Determine action
        let action = match tool {
            "Read" => Action::Opened,
            "Write" => {
                if !self.seen_paths.contains(path) {
                    self.created_paths.insert(path.to_string());
                    Action::Created
                } else {
                    Action::Modified
                }
            }
            _ => Action::Modified, // Edit, NotebookEdit
        };
        self.seen_paths.insert(path.to_string());

        Some(FileEvent {
            path: path.to_string(),
            action,
            source,
            timestamp: absolute_ts,
            confidence: Confidence::High,
            session_id: None,
        })
    }

    /// Post-process: if a path's first write was "created", restore that action
    /// even if later Edit events changed it to "modified".
    fn restore_created_actions(&self, events: &mut [FileEvent]) {
        for event in events.iter_mut() {
            if self.created_paths.contains(&event.path) && event.action == Action::Modified {
                event.action = Action::Created;
            }
        }
    }

    /// Reset cursor state for full re-read
    fn reset(&mut self) {
        self.last_offset = 0;
        self.current_session_base_time = 0;
        self.current_session_max_ts = 0.0;
        self.prev_relative_ts = 0.0;
        self.created_paths.clear();
        self.seen_paths.clear();
    }
}

struct SessionSegment {
    start: usize,
    end: usize,
    max_ts: f64,
}

/// Extract the relative timestamp from a JSON line
fn extract_timestamp(line: &str) -> Option<f64> {
    let parsed: serde_json::Value = serde_json::from_str(line).ok()?;
    parsed.get("timestamp")?.as_f64()
}

/// Find the maximum relative timestamp in a slice of lines
fn find_max_relative_ts(lines: &[&str]) -> f64 {
    let mut max = 0.0f64;
    for line in lines {
        if let Some(ts) = extract_timestamp(line) && ts > max {
            max = ts;
        }
    }
    max
}

/// Find the first relative timestamp in a slice of lines
fn find_first_relative_ts(lines: &[&str]) -> f64 {
    for line in lines.iter().take(10) {
        if let Some(ts) = extract_timestamp(line)
            && ts > 0.0
        {
            return ts;
        }
    }
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_fixture(dir: &std::path::Path, content: &str) -> PathBuf {
        let path = dir.join("events.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    /// Fixture: single session with 3 events
    const SINGLE_SESSION: &str = r#"{"tool":"Write","original_cmd":"/home/test/dev/main.rs","timestamp":5.0}
{"tool":"Edit","original_cmd":"/home/test/dev/main.rs","timestamp":12.5}
{"tool":"Read","original_cmd":"/home/test/dev/lib.rs","timestamp":20.0}
"#;

    /// Fixture: two sessions (second starts with lower timestamp)
    const TWO_SESSIONS: &str = r#"{"tool":"Write","original_cmd":"/home/test/dev/old.rs","timestamp":100.0}
{"tool":"Edit","original_cmd":"/home/test/dev/old.rs","timestamp":200.0}
{"tool":"Write","original_cmd":"/home/test/dev/new.rs","timestamp":5.0}
{"tool":"Edit","original_cmd":"/home/test/dev/new.rs","timestamp":30.0}
"#;

    /// Fixture: events with non-tool entries and sandbox paths
    const MIXED_EVENTS: &str = r#"{"tool":"Bash","original_cmd":"ls","timestamp":1.0}
{"tool":"Write","original_cmd":"/home/test/dev/file.rs","timestamp":5.0}
{"tool":"Read","original_cmd":"/test/sandbox/bad.rs","timestamp":10.0}
{"tool":"Edit","original_cmd":"/home/test/node_modules/pkg/index.js","timestamp":15.0}
{"tool":"Write","original_cmd":"/home/test/dev/file2.rs","timestamp":20.0}
"#;

    #[test]
    fn single_session_parse() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_fixture(tmp.path(), SINGLE_SESSION);
        let mut cursor = ClaudeCursor::with_home(path, "/home/test".into());
        let events = cursor.read_new().unwrap();

        assert_eq!(events.len(), 3);

        // Write to new path → created
        assert_eq!(events[0].path, "/home/test/dev/main.rs");
        assert_eq!(events[0].action, Action::Created);
        assert_eq!(events[0].source, Source::Claude);

        // Edit to existing path → created (preserved from first Write)
        assert_eq!(events[1].path, "/home/test/dev/main.rs");
        assert_eq!(events[1].action, Action::Created); // preserved!

        // Read → opened
        assert_eq!(events[2].path, "/home/test/dev/lib.rs");
        assert_eq!(events[2].action, Action::Opened);
    }

    #[test]
    fn two_sessions_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_fixture(tmp.path(), TWO_SESSIONS);
        let mut cursor = ClaudeCursor::with_home(path, "/home/test".into());
        let events = cursor.read_new().unwrap();

        // Should have events from both sessions
        let paths: HashSet<&str> = events.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains("/home/test/dev/new.rs"));
        // old.rs may or may not be in 24h window depending on computed baseTime
    }

    #[test]
    fn filters_non_tools_and_bad_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_fixture(tmp.path(), MIXED_EVENTS);
        let mut cursor = ClaudeCursor::with_home(path, "/home/test".into());
        let events = cursor.read_new().unwrap();

        let paths: Vec<&str> = events.iter().map(|e| e.path.as_str()).collect();
        // Bash tool → filtered
        // /test/sandbox/bad.rs → not under $HOME → filtered
        // node_modules path → filtered by should_skip_path
        assert!(paths.contains(&"/home/test/dev/file.rs"));
        assert!(paths.contains(&"/home/test/dev/file2.rs"));
        assert!(!paths.contains(&"/test/sandbox/bad.rs"));
    }

    #[test]
    fn incremental_read_returns_only_new() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("events.jsonl");

        // Write initial events
        {
            let mut f = std::fs::File::create(&path).unwrap();
            writeln!(
                f,
                r#"{{"tool":"Write","original_cmd":"/home/test/dev/a.rs","timestamp":5.0}}"#
            )
            .unwrap();
        }

        let mut cursor = ClaudeCursor::with_home(path.clone(), "/home/test".into());
        let events1 = cursor.read_new().unwrap();
        assert_eq!(events1.len(), 1);
        assert_eq!(events1[0].path, "/home/test/dev/a.rs");

        // Append more events
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(
                f,
                r#"{{"tool":"Write","original_cmd":"/home/test/dev/b.rs","timestamp":30.0}}"#
            )
            .unwrap();
        }

        let events2 = cursor.read_new().unwrap();
        assert_eq!(events2.len(), 1);
        assert_eq!(events2[0].path, "/home/test/dev/b.rs");
    }

    #[test]
    fn empty_file_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_fixture(tmp.path(), "");
        let mut cursor = ClaudeCursor::with_home(path, "/home/test".into());
        let events = cursor.read_new().unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn missing_file_returns_empty() {
        let mut cursor =
            ClaudeCursor::with_home("/nonexistent/events.jsonl".into(), "/home/test".into());
        let events = cursor.read_new().unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn created_preserved_across_edits() {
        let tmp = tempfile::tempdir().unwrap();
        // Write then Edit the same file — should preserve "created"
        let content = r#"{"tool":"Write","original_cmd":"/home/test/dev/new.rs","timestamp":5.0}
{"tool":"Edit","original_cmd":"/home/test/dev/new.rs","timestamp":10.0}
{"tool":"Edit","original_cmd":"/home/test/dev/new.rs","timestamp":20.0}
"#;
        let path = write_fixture(tmp.path(), content);
        let mut cursor = ClaudeCursor::with_home(path, "/home/test".into());
        let events = cursor.read_new().unwrap();

        // All events for new.rs should be "created" (preserved from first Write)
        for event in &events {
            assert_eq!(event.action, Action::Created, "path: {}", event.path);
        }
    }

    #[test]
    fn timestamps_are_absolute() {
        let tmp = tempfile::tempdir().unwrap();
        let content =
            r#"{"tool":"Write","original_cmd":"/home/test/dev/a.rs","timestamp":10.0}
"#;
        let path = write_fixture(tmp.path(), content);
        let mut cursor = ClaudeCursor::with_home(path, "/home/test".into());
        let events = cursor.read_new().unwrap();

        assert_eq!(events.len(), 1);
        // Timestamp should be absolute (baseTime + 10)
        // baseTime = fileMtime - 10, so absoluteTs = fileMtime
        let now = unix_now();
        // Should be within the last few seconds (file was just written)
        assert!(
            events[0].timestamp > now - 60 && events[0].timestamp <= now + 5,
            "timestamp {} not near now {}",
            events[0].timestamp,
            now
        );
    }
}
