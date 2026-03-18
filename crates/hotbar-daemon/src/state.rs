use std::collections::{HashMap, VecDeque};

use hotbar_common::protocol::Delta;
use hotbar_common::types::{
    Action, ActionFilter, ActivityLevel, FileEvent, Filter, HotFile, Pin, Source,
};

use crate::db::{self, Db, DbError};

/// Central in-memory state for the daemon.
///
/// Tokio tasks write events via `apply_events()`. The panel reads state via
/// `files()` and `apply_filter()`. Shared as `Arc<RwLock<HotState>>`.
pub struct HotState {
    files: Vec<HotFile>,
    by_path: HashMap<String, usize>,
    /// Pinned files in the Pit Stop shelf
    pub pins: Vec<Pin>,
    /// Rolling event rate tracker
    pub activity: ActivityTracker,
    /// Maximum number of files to retain
    max_files: usize,
}

impl HotState {
    /// Create empty state with default capacity.
    pub fn new() -> Self {
        Self {
            files: Vec::new(),
            by_path: HashMap::new(),
            pins: Vec::new(),
            activity: ActivityTracker::new(10),
            max_files: 200,
        }
    }

    /// Create state with a custom file limit (for testing).
    pub fn with_max_files(max_files: usize) -> Self {
        Self {
            max_files,
            ..Self::new()
        }
    }

    /// All files, sorted by timestamp DESC.
    pub fn files(&self) -> &[HotFile] {
        &self.files
    }

    /// Number of tracked files.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether the state has no files.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Apply new events and return a delta describing what changed.
    ///
    /// Events are merged by path (most recent wins). The returned Delta
    /// lists added, updated, and removed files.
    pub fn apply_events(&mut self, events: Vec<FileEvent>) -> Delta {
        let _span =
            tracing::debug_span!("state_apply_events", event_count = events.len()).entered();
        if events.is_empty() {
            return Delta {
                activity_level: ActivityLevel(self.activity.events_per_second()),
                ..Default::default()
            };
        }

        let mut delta = Delta::default();

        for event in &events {
            let hotfile = file_event_to_hotfile(event);

            if let Some(&idx) = self.by_path.get(&event.path) {
                // Existing path — update if newer
                if event.timestamp > self.files[idx].timestamp {
                    self.files[idx] = hotfile.clone();
                    delta.updated.push(hotfile);
                }
            } else {
                // New path
                let idx = self.files.len();
                self.by_path.insert(event.path.clone(), idx);
                self.files.push(hotfile.clone());
                delta.added.push(hotfile);
            }
        }

        // Record activity
        self.activity.record_events(events.len() as u32);
        delta.activity_level = ActivityLevel(self.activity.events_per_second());

        // Re-sort by timestamp DESC
        self.files.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        // Rebuild index after sort
        self.rebuild_index();

        // Trim to max_files
        if self.files.len() > self.max_files {
            let removed: Vec<String> = self.files[self.max_files..]
                .iter()
                .map(|f| f.path.clone())
                .collect();
            self.files.truncate(self.max_files);
            self.rebuild_index();
            delta.removed = removed;
        }

        delta
    }

    /// Filter files by source and action.
    pub fn apply_filter(&self, source: Filter, action: ActionFilter) -> Vec<&HotFile> {
        self.files
            .iter()
            .filter(|f| {
                let source_ok = match source {
                    Filter::All => true,
                    Filter::Claude => f.source == Source::Claude,
                    Filter::Codex => f.source == Source::Codex,
                    Filter::User => f.source == Source::User,
                    Filter::System => f.source == Source::System,
                };
                let action_ok = match action {
                    ActionFilter::All => true,
                    ActionFilter::Opened => f.action == Action::Opened,
                    ActionFilter::Modified => f.action == Action::Modified,
                    ActionFilter::Created => f.action == Action::Created,
                    ActionFilter::Deleted => f.action == Action::Deleted,
                };
                source_ok && action_ok
            })
            .collect()
    }

    /// Load initial state from the database.
    pub fn hydrate_from_db(&mut self, db: &Db) -> Result<(), DbError> {
        self.files = db.get_events(None, self.max_files)?;
        self.rebuild_index();
        self.pins = db.get_pins()?;

        tracing::info!(
            files = self.files.len(),
            pins = self.pins.len(),
            "state hydrated from database"
        );
        Ok(())
    }

    /// Rebuild the by_path index from the current files vec.
    fn rebuild_index(&mut self) {
        self.by_path.clear();
        for (i, file) in self.files.iter().enumerate() {
            self.by_path.insert(file.path.clone(), i);
        }
    }
}

impl Default for HotState {
    fn default() -> Self {
        Self::new()
    }
}

/// Rolling event rate tracker over a time window.
///
/// Stores (timestamp, count) tuples in a ring buffer and computes
/// events-per-second over the window.
pub struct ActivityTracker {
    events: VecDeque<(i64, u32)>,
    window_secs: i64,
}

impl ActivityTracker {
    /// Create a tracker with the given window size in seconds.
    pub fn new(window_secs: i64) -> Self {
        Self {
            events: VecDeque::new(),
            window_secs,
        }
    }

    /// Record a batch of events at the current time.
    pub fn record_events(&mut self, count: u32) {
        if count == 0 {
            return;
        }
        let now = crate::ingest::unix_now();
        self.events.push_back((now, count));
        self.prune(now);
    }

    /// Record events at a specific timestamp (for testing).
    pub fn record_events_at(&mut self, count: u32, timestamp: i64) {
        if count == 0 {
            return;
        }
        self.events.push_back((timestamp, count));
        self.prune(timestamp);
    }

    /// Compute events per second over the rolling window.
    pub fn events_per_second(&self) -> f32 {
        if self.events.is_empty() {
            return 0.0;
        }
        let now = crate::ingest::unix_now();
        let cutoff = now - self.window_secs;
        let total: u32 = self
            .events
            .iter()
            .filter(|(ts, _)| *ts >= cutoff)
            .map(|(_, count)| count)
            .sum();
        total as f32 / self.window_secs as f32
    }

    /// Compute events per second relative to a given timestamp (for testing).
    pub fn events_per_second_at(&self, now: i64) -> f32 {
        if self.events.is_empty() {
            return 0.0;
        }
        let cutoff = now - self.window_secs;
        let total: u32 = self
            .events
            .iter()
            .filter(|(ts, _)| *ts >= cutoff)
            .map(|(_, count)| count)
            .sum();
        total as f32 / self.window_secs as f32
    }

    /// Remove entries older than the window.
    fn prune(&mut self, now: i64) {
        let cutoff = now - self.window_secs;
        while let Some(&(ts, _)) = self.events.front() {
            if ts < cutoff {
                self.events.pop_front();
            } else {
                break;
            }
        }
    }
}

impl Default for ActivityTracker {
    fn default() -> Self {
        Self::new(10)
    }
}

/// Convert a FileEvent to a HotFile for display.
fn file_event_to_hotfile(event: &FileEvent) -> HotFile {
    let (filename, dir, full_dir) = db::split_path(&event.path);
    let mime_type = db::guess_mime(&filename);
    HotFile {
        path: event.path.clone(),
        filename,
        dir,
        full_dir,
        timestamp: event.timestamp,
        source: event.source,
        mime_type,
        action: event.action,
        confidence: event.confidence,
        metadata: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hotbar_common::types::Confidence;

    fn make_event(path: &str, action: Action, source: Source, ts: i64) -> FileEvent {
        FileEvent {
            path: path.into(),
            action,
            source,
            timestamp: ts,
            confidence: Confidence::High,
            session_id: None,
        }
    }

    #[test]
    fn empty_state() {
        let state = HotState::new();
        assert!(state.is_empty());
        assert_eq!(state.len(), 0);
    }

    #[test]
    fn apply_events_adds_files() {
        let mut state = HotState::new();
        let events = vec![
            make_event("/a.rs", Action::Created, Source::Claude, 100),
            make_event("/b.rs", Action::Modified, Source::User, 200),
        ];
        let delta = state.apply_events(events);

        assert_eq!(state.len(), 2);
        assert_eq!(delta.added.len(), 2);
        assert!(delta.updated.is_empty());
        assert!(delta.removed.is_empty());

        // Sorted by timestamp DESC
        assert_eq!(state.files()[0].path, "/b.rs");
        assert_eq!(state.files()[1].path, "/a.rs");
    }

    #[test]
    fn apply_events_updates_existing() {
        let mut state = HotState::new();

        // First batch
        state.apply_events(vec![make_event(
            "/a.rs",
            Action::Created,
            Source::Claude,
            100,
        )]);
        assert_eq!(state.len(), 1);

        // Second batch with newer timestamp
        let delta = state.apply_events(vec![make_event(
            "/a.rs",
            Action::Modified,
            Source::Claude,
            200,
        )]);

        assert_eq!(state.len(), 1); // Still 1 file
        assert_eq!(delta.updated.len(), 1);
        assert!(delta.added.is_empty());
        assert_eq!(state.files()[0].action, Action::Modified);
        assert_eq!(state.files()[0].timestamp, 200);
    }

    #[test]
    fn apply_events_ignores_older() {
        let mut state = HotState::new();

        state.apply_events(vec![make_event(
            "/a.rs",
            Action::Modified,
            Source::Claude,
            200,
        )]);

        // Older event should be ignored
        let delta = state.apply_events(vec![make_event(
            "/a.rs",
            Action::Created,
            Source::Claude,
            100,
        )]);

        assert_eq!(state.len(), 1);
        assert!(delta.updated.is_empty());
        assert!(delta.added.is_empty());
        assert_eq!(state.files()[0].timestamp, 200);
    }

    #[test]
    fn apply_events_trims_to_max() {
        let mut state = HotState::with_max_files(3);

        let events: Vec<FileEvent> = (0..5)
            .map(|i| make_event(&format!("/file{i}.rs"), Action::Modified, Source::User, i))
            .collect();

        let delta = state.apply_events(events);

        assert_eq!(state.len(), 3);
        assert_eq!(delta.removed.len(), 2);
        // Files with highest timestamps kept (sorted DESC)
        assert_eq!(state.files()[0].path, "/file4.rs");
        assert_eq!(state.files()[1].path, "/file3.rs");
        assert_eq!(state.files()[2].path, "/file2.rs");
    }

    #[test]
    fn apply_filter_by_source() {
        let mut state = HotState::new();
        state.apply_events(vec![
            make_event("/a.rs", Action::Modified, Source::Claude, 100),
            make_event("/b.rs", Action::Modified, Source::User, 200),
            make_event("/c.rs", Action::Modified, Source::Codex, 300),
        ]);

        let claude = state.apply_filter(Filter::Claude, ActionFilter::All);
        assert_eq!(claude.len(), 1);
        assert_eq!(claude[0].path, "/a.rs");

        let all = state.apply_filter(Filter::All, ActionFilter::All);
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn apply_filter_by_action() {
        let mut state = HotState::new();
        state.apply_events(vec![
            make_event("/a.rs", Action::Created, Source::Claude, 100),
            make_event("/b.rs", Action::Modified, Source::Claude, 200),
            make_event("/c.rs", Action::Opened, Source::Claude, 300),
        ]);

        let created = state.apply_filter(Filter::All, ActionFilter::Created);
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].path, "/a.rs");

        let opened = state.apply_filter(Filter::All, ActionFilter::Opened);
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].path, "/c.rs");
    }

    #[test]
    fn apply_filter_combined() {
        let mut state = HotState::new();
        state.apply_events(vec![
            make_event("/a.rs", Action::Created, Source::Claude, 100),
            make_event("/b.rs", Action::Created, Source::User, 200),
            make_event("/c.rs", Action::Modified, Source::Claude, 300),
        ]);

        let claude_created = state.apply_filter(Filter::Claude, ActionFilter::Created);
        assert_eq!(claude_created.len(), 1);
        assert_eq!(claude_created[0].path, "/a.rs");
    }

    #[test]
    fn empty_events_returns_empty_delta() {
        let mut state = HotState::new();
        let delta = state.apply_events(vec![]);
        assert!(delta.is_empty());
    }

    #[test]
    fn activity_tracker_empty() {
        let tracker = ActivityTracker::new(10);
        assert_eq!(tracker.events_per_second_at(1000), 0.0);
    }

    #[test]
    fn activity_tracker_single_batch() {
        let mut tracker = ActivityTracker::new(10);
        tracker.record_events_at(10, 1000);

        // 10 events in a 10-second window = 1.0 eps
        assert!((tracker.events_per_second_at(1000) - 1.0).abs() < 0.01);
    }

    #[test]
    fn activity_tracker_multiple_batches() {
        let mut tracker = ActivityTracker::new(10);
        tracker.record_events_at(5, 1000);
        tracker.record_events_at(5, 1003);

        // 10 events in 10-second window = 1.0 eps
        assert!((tracker.events_per_second_at(1005) - 1.0).abs() < 0.01);
    }

    #[test]
    fn activity_tracker_prunes_old() {
        let mut tracker = ActivityTracker::new(10);
        tracker.record_events_at(100, 1000);
        tracker.record_events_at(5, 1015);

        // At t=1015, the t=1000 batch is 15s old (>10s window), should be pruned
        assert!((tracker.events_per_second_at(1015) - 0.5).abs() < 0.01);
    }

    #[test]
    fn activity_tracker_thermal_states() {
        let mut tracker = ActivityTracker::new(10);

        // Cold: 0 events
        let level = ActivityLevel(tracker.events_per_second_at(1000));
        assert_eq!(level.thermal_state(), "cold");

        // Warm: 30 events in 10s = 3.0 eps
        tracker.record_events_at(30, 1000);
        let level = ActivityLevel(tracker.events_per_second_at(1005));
        assert_eq!(level.thermal_state(), "warm");

        // Hot: 100 events in 10s = 10.0 eps
        tracker.record_events_at(70, 1005);
        let level = ActivityLevel(tracker.events_per_second_at(1005));
        assert_eq!(level.thermal_state(), "hot");

        // On fire: 200 events total in 10s = 20.0 eps
        tracker.record_events_at(100, 1005);
        let level = ActivityLevel(tracker.events_per_second_at(1005));
        assert_eq!(level.thermal_state(), "on_fire");
    }

    #[test]
    fn hydrate_from_db() {
        let db = Db::open_in_memory().unwrap();

        // Insert a session for FK
        db.conn()
            .execute(
                "INSERT INTO sessions (session_id, agent, started_at) VALUES (?1, ?2, ?3)",
                rusqlite::params!["s1", "claude", 1710500000],
            )
            .unwrap();

        db.insert_events(&[FileEvent {
            path: "/home/zack/dev/main.rs".into(),
            action: Action::Modified,
            source: Source::Claude,
            timestamp: 1710500000,
            confidence: Confidence::High,
            session_id: Some("s1".into()),
        }])
        .unwrap();

        db.upsert_pin(&Pin {
            path: "/home/zack/dev/main.rs".into(),
            label: Some("entry".into()),
            pin_group: "default".into(),
            position: 0,
            pinned_at: 1710500000,
        })
        .unwrap();

        let mut state = HotState::new();
        state.hydrate_from_db(&db).unwrap();

        assert_eq!(state.len(), 1);
        assert_eq!(state.files()[0].path, "/home/zack/dev/main.rs");
        assert_eq!(state.pins.len(), 1);
    }

    #[test]
    fn file_event_to_hotfile_conversion() {
        let event = FileEvent {
            path: "/home/zack/dev/hotbar/main.rs".into(),
            action: Action::Created,
            source: Source::Claude,
            timestamp: 1710500000,
            confidence: Confidence::High,
            session_id: None,
        };

        let hotfile = file_event_to_hotfile(&event);
        assert_eq!(hotfile.filename, "main.rs");
        assert_eq!(hotfile.full_dir, "/home/zack/dev/hotbar");
        assert_eq!(hotfile.mime_type, "text/x-rust");
        assert_eq!(hotfile.action, Action::Created);
    }
}
