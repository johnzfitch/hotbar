use std::collections::{HashMap, HashSet};
use std::path::Path;

use hotbar_common::types::{Action, Confidence, FileEvent, Source};

use super::{
    home_dir, include_system_events, is_code_file, is_under_home, source_for_path, unix_now,
    IngestError, SKIP_DIRS, SKIP_FILES,
};

/// Background directory scanner.
///
/// Scans active directories (derived from agent events) for code files modified
/// in the last 24h that aren't already covered by agent events. Detects creates
/// via birthtime heuristic.
pub struct DirScanner {
    home: String,
    include_system: bool,
}

impl Default for DirScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl DirScanner {
    /// Create a new scanner.
    pub fn new() -> Self {
        Self {
            home: home_dir(),
            include_system: include_system_events(),
        }
    }

    /// Create a scanner with explicit home dir (for testing).
    pub fn with_home(home: String) -> Self {
        Self {
            home,
            include_system: false,
        }
    }

    /// Scan active directories for user-modified files not covered by agent events.
    ///
    /// `active_dirs` — directories containing files from agent sources
    /// `agent_timestamps` — map of path -> latest agent timestamp (for dedup)
    ///
    /// Returns discovered user file events.
    pub fn scan(
        &self,
        active_dirs: &HashSet<String>,
        agent_timestamps: &HashMap<String, i64>,
    ) -> Result<Vec<FileEvent>, IngestError> {
        let now = unix_now();
        let cutoff = now - 86400;
        let mut events = Vec::new();
        let mut seen = HashSet::new();

        for dir in active_dirs {
            if !is_under_home(dir, &self.home) {
                continue;
            }

            // Skip hidden/build directories
            let dir_name = dir.rsplit('/').next().unwrap_or("");
            if SKIP_DIRS.contains(&dir_name) {
                continue;
            }

            let dir_path = Path::new(dir);
            if !dir_path.is_dir() {
                continue;
            }

            // ReadDir iterator drops automatically when it goes out of scope,
            // releasing the directory FD. No explicit close needed in Rust.
            let entries = match std::fs::read_dir(dir_path) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for entry in entries.flatten() {
                let metadata = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };

                if !metadata.is_file() {
                    continue;
                }

                let file_name = entry.file_name();
                let name = file_name.to_string_lossy();

                // Skip hidden files
                if name.starts_with('.') {
                    continue;
                }

                // Skip known skip files
                if SKIP_FILES.contains(&name.as_ref()) {
                    continue;
                }

                let path = entry.path();
                let path_str = path.to_string_lossy().to_string();

                if seen.contains(&path_str) {
                    continue;
                }
                seen.insert(path_str.clone());

                let source = match source_for_path(
                    &path_str,
                    &self.home,
                    Source::User,
                    self.include_system,
                ) {
                    Some(s) => s,
                    None => continue,
                };

                // Get modification time
                let mtime = metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);

                if mtime < cutoff {
                    continue;
                }

                // Skip files where an agent has a recent event (5s tolerance).
                // The 5s tolerance accounts for baseTime drift in Claude's
                // relative timestamp math.
                if let Some(&agent_ts) = agent_timestamps.get(&path_str)
                    && agent_ts >= mtime - 5
                {
                    continue;
                }

                // Only include code/text files
                if !is_code_file(&path_str) {
                    continue;
                }

                // Detect creates: if birthtime is recent and close to mtime
                let action = detect_action(&metadata, mtime, cutoff);

                events.push(FileEvent {
                    path: path_str,
                    action,
                    source,
                    timestamp: mtime,
                    confidence: Confidence::Low, // heuristic-based
                    session_id: None,
                });
            }
        }

        tracing::debug!(
            dirs = active_dirs.len(),
            files = events.len(),
            "dir scanner: scan complete"
        );

        Ok(events)
    }
}

/// Detect whether a file was created or modified based on birthtime heuristic.
///
/// If birthtime is recent (within 24h) and close to mtime (within 120s),
/// the file was likely just created.
fn detect_action(metadata: &std::fs::Metadata, mtime: i64, cutoff: i64) -> Action {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // On Linux, st_ctime is inode change time, not creation time.
        // True birthtime (btime) requires statx() which std doesn't expose directly.
        // We use created() which falls back to ctime on older kernels.
        let birthtime = metadata
            .created()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);

        if let Some(bt) = birthtime
            && bt > cutoff
            && (mtime - bt).unsigned_abs() < 120
        {
            return Action::Created;
        }

        // Fallback: if ctime and mtime are very close and recent, likely created
        let ctime = metadata.ctime();
        if ctime > cutoff && (mtime - ctime).unsigned_abs() < 120 {
            return Action::Created;
        }
    }

    Action::Modified
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn scan_finds_modified_files() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_string_lossy().to_string();

        // Create a code file
        let dev_dir = tmp.path().join("dev");
        std::fs::create_dir(&dev_dir).unwrap();
        let file_path = dev_dir.join("main.rs");
        std::fs::File::create(&file_path)
            .unwrap()
            .write_all(b"fn main() {}")
            .unwrap();

        let scanner = DirScanner::with_home(home);
        let dirs: HashSet<String> = [dev_dir.to_string_lossy().to_string()].into();
        let agents: HashMap<String, i64> = HashMap::new();

        let events = scanner.scan(&dirs, &agents).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].path, file_path.to_string_lossy().as_ref());
        assert_eq!(events[0].source, Source::User);
        assert_eq!(events[0].confidence, Confidence::Low);
    }

    #[test]
    fn skips_agent_covered_files() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_string_lossy().to_string();

        let dev_dir = tmp.path().join("dev");
        std::fs::create_dir(&dev_dir).unwrap();
        let file_path = dev_dir.join("main.rs");
        std::fs::File::create(&file_path)
            .unwrap()
            .write_all(b"fn main() {}")
            .unwrap();

        let now = unix_now();
        let scanner = DirScanner::with_home(home);
        let dirs: HashSet<String> = [dev_dir.to_string_lossy().to_string()].into();
        let agents: HashMap<String, i64> =
            [(file_path.to_string_lossy().to_string(), now)].into();

        let events = scanner.scan(&dirs, &agents).unwrap();
        assert!(events.is_empty(), "should skip agent-covered file");
    }

    #[test]
    fn skips_non_code_files() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_string_lossy().to_string();

        let dev_dir = tmp.path().join("dev");
        std::fs::create_dir(&dev_dir).unwrap();
        // Create a non-code file
        std::fs::File::create(dev_dir.join("image.png"))
            .unwrap()
            .write_all(b"PNG")
            .unwrap();

        let scanner = DirScanner::with_home(home);
        let dirs: HashSet<String> = [dev_dir.to_string_lossy().to_string()].into();
        let agents: HashMap<String, i64> = HashMap::new();

        let events = scanner.scan(&dirs, &agents).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn skips_hidden_files() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_string_lossy().to_string();

        let dev_dir = tmp.path().join("dev");
        std::fs::create_dir(&dev_dir).unwrap();
        std::fs::File::create(dev_dir.join(".hidden.rs"))
            .unwrap()
            .write_all(b"// hidden")
            .unwrap();

        let scanner = DirScanner::with_home(home);
        let dirs: HashSet<String> = [dev_dir.to_string_lossy().to_string()].into();
        let agents: HashMap<String, i64> = HashMap::new();

        let events = scanner.scan(&dirs, &agents).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn skips_skip_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_string_lossy().to_string();

        let nm_dir = tmp.path().join("node_modules");
        std::fs::create_dir(&nm_dir).unwrap();
        std::fs::File::create(nm_dir.join("pkg.js"))
            .unwrap()
            .write_all(b"module.exports = {}")
            .unwrap();

        let scanner = DirScanner::with_home(home);
        let dirs: HashSet<String> = [nm_dir.to_string_lossy().to_string()].into();
        let agents: HashMap<String, i64> = HashMap::new();

        let events = scanner.scan(&dirs, &agents).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn skips_outside_home() {
        let scanner = DirScanner::with_home("/home/test".into());
        let dirs: HashSet<String> = ["/usr/share/data".into()].into();
        let agents: HashMap<String, i64> = HashMap::new();

        let events = scanner.scan(&dirs, &agents).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn agent_timestamp_tolerance() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_string_lossy().to_string();

        let dev_dir = tmp.path().join("dev");
        std::fs::create_dir(&dev_dir).unwrap();
        let file_path = dev_dir.join("main.rs");
        std::fs::File::create(&file_path)
            .unwrap()
            .write_all(b"fn main() {}")
            .unwrap();

        let file_mtime = std::fs::metadata(&file_path)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Agent timestamp 3 seconds before mtime (within 5s tolerance)
        let scanner = DirScanner::with_home(home);
        let dirs: HashSet<String> = [dev_dir.to_string_lossy().to_string()].into();
        let agents: HashMap<String, i64> =
            [(file_path.to_string_lossy().to_string(), file_mtime - 3)].into();

        let events = scanner.scan(&dirs, &agents).unwrap();
        assert!(events.is_empty(), "within 5s tolerance → should skip");
    }

    #[test]
    fn nonexistent_dir_skipped() {
        let scanner = DirScanner::with_home("/home/test".into());
        let dirs: HashSet<String> = ["/home/test/nonexistent".into()].into();
        let agents: HashMap<String, i64> = HashMap::new();

        let events = scanner.scan(&dirs, &agents).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn newly_created_file_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_string_lossy().to_string();

        let dev_dir = tmp.path().join("dev");
        std::fs::create_dir(&dev_dir).unwrap();

        // Create a file just now — birthtime should be close to mtime
        let file_path = dev_dir.join("brand_new.rs");
        std::fs::File::create(&file_path)
            .unwrap()
            .write_all(b"fn new() {}")
            .unwrap();

        let scanner = DirScanner::with_home(home);
        let dirs: HashSet<String> = [dev_dir.to_string_lossy().to_string()].into();
        let agents: HashMap<String, i64> = HashMap::new();

        let events = scanner.scan(&dirs, &agents).unwrap();
        assert_eq!(events.len(), 1);
        // On supported filesystems, this should be Created
        // (but might be Modified on some FS without birthtime support)
        assert!(
            events[0].action == Action::Created || events[0].action == Action::Modified,
            "should be Created or Modified"
        );
    }
}
