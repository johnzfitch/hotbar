use std::path::PathBuf;

use hotbar_common::types::{Action, Confidence, FileEvent, Source};

use super::{
    decode_percent, home_dir, include_system_events, is_code_file, is_relevant_mime,
    is_under_home, parse_iso8601, should_skip_path, source_for_path, unix_now, IngestError,
    SKIP_FILES,
};

/// Parser for `~/.local/share/recently-used.xbel`.
///
/// This file is written by GTK apps (file managers, etc.) and contains
/// recently accessed files with MIME types and visit timestamps.
/// We extract code/text file entries within a 24h window.
pub struct XbelParser {
    path: PathBuf,
    home: String,
    include_system: bool,
}

impl Default for XbelParser {
    fn default() -> Self {
        Self::new()
    }
}

impl XbelParser {
    /// Create a parser for the default XBEL path.
    pub fn new() -> Self {
        let home = home_dir();
        let path = PathBuf::from(format!("{home}/.local/share/recently-used.xbel"));
        Self {
            path,
            home,
            include_system: include_system_events(),
        }
    }

    /// Create a parser with explicit path and home (for testing).
    pub fn with_path(path: PathBuf, home: String) -> Self {
        Self {
            path,
            home,
            include_system: false,
        }
    }

    /// Read events from the XBEL file.
    ///
    /// Re-reads the entire file each call, returning all entries within the 24h window.
    /// The state module handles deduplication.
    pub fn read_new(&self) -> Result<Vec<FileEvent>, IngestError> {
        let _span = tracing::debug_span!("xbel_ingest").entered();
        let content = match std::fs::read_to_string(&self.path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(path = %self.path.display(), "xbel file not found");
                return Ok(vec![]);
            }
            Err(e) => return Err(e.into()),
        };

        if content.is_empty() {
            return Ok(vec![]);
        }

        let now = unix_now();
        let cutoff = now - 86400;
        let mut events = Vec::new();

        // Split on <bookmark blocks and parse each
        // The file format has: <bookmark href="..." ... visited="..." ...>
        //   <info><metadata><mime:mime-type type="..."/></metadata></info>
        // </bookmark>
        for block in content.split("<bookmark ").skip(1) {
            if let Some(event) = self.parse_bookmark_block(block, now, cutoff) {
                events.push(event);
            }
        }

        tracing::debug!(entries = events.len(), "xbel parser: read complete");

        Ok(events)
    }

    /// Parse a single `<bookmark ...>...</bookmark>` block.
    fn parse_bookmark_block(
        &self,
        block: &str,
        now: i64,
        cutoff: i64,
    ) -> Option<FileEvent> {
        // Extract href="..."
        let href = extract_attr(block, "href")?;
        if !href.starts_with("file://") {
            return None;
        }

        // Decode URI to filesystem path
        let path = decode_percent(&href[7..]);

        if !is_under_home(&path, &self.home) {
            return None;
        }

        let source = source_for_path(&path, &self.home, Source::User, self.include_system)?;

        // Extract MIME type
        let mime = extract_mime_type(block).unwrap_or_default();
        let mime_relevant = !mime.is_empty() && is_relevant_mime(&mime);
        let path_is_code = is_code_file(&path);

        if !mime_relevant && !path_is_code {
            return None;
        }

        // Skip build artifacts and lockfiles
        if should_skip_path(&path) {
            return None;
        }

        let basename = path.rsplit('/').next().unwrap_or("");
        if SKIP_FILES.contains(&basename) {
            return None;
        }

        // Parse visited timestamp
        let visited_str = extract_attr(block, "visited")?;
        let timestamp = parse_iso8601(&visited_str)?;

        // Skip entries outside 24h window
        if timestamp < cutoff || timestamp > now + 60 {
            return None;
        }

        Some(FileEvent {
            path,
            action: Action::Opened,
            source,
            timestamp,
            confidence: Confidence::Low, // XBEL is heuristic
            session_id: None,
        })
    }
}

/// Extract an XML attribute value: `name="value"` -> `value`
fn extract_attr(block: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = block.find(&needle)? + needle.len();
    let end = start + block[start..].find('"')?;
    Some(block[start..end].to_string())
}

/// Extract MIME type from `<mime:mime-type type="..."/>` within a bookmark block
fn extract_mime_type(block: &str) -> Option<String> {
    // Pattern: <mime:mime-type type="text/plain"/>
    let needle = "mime:mime-type type=\"";
    let start = block.find(needle)? + needle.len();
    let end = start + block[start..].find('"')?;
    Some(block[start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_xbel(entries: &[(&str, &str, &str)]) -> String {
        let mut xbel = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<xbel version="1.0"
      xmlns:bookmark="http://www.freedesktop.org/standards/desktop-bookmarks"
      xmlns:mime="http://www.freedesktop.org/standards/shared-mime-info">
"#,
        );

        for (href, visited, mime) in entries {
            xbel.push_str(&format!(
                r#"<bookmark href="{href}" visited="{visited}">
  <info><metadata>
    <mime:mime-type type="{mime}"/>
  </metadata></info>
</bookmark>
"#
            ));
        }

        xbel.push_str("</xbel>\n");
        xbel
    }

    fn format_iso(ts: i64) -> String {
        // Simple formatter for test timestamps
        let z = ts / 86400 + 719468;
        let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let mo = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if mo <= 2 { y + 1 } else { y };
        let rem = ts % 86400;
        let h = rem / 3600;
        let m = (rem % 3600) / 60;
        let s = rem % 60;
        format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
    }

    #[test]
    fn parse_basic_xbel() {
        let tmp = tempfile::tempdir().unwrap();
        let now = unix_now();
        let recent = format_iso(now - 300); // 5 min ago

        let xbel = make_xbel(&[(
            "file:///home/test/dev/main.rs",
            &recent,
            "text/x-rust",
        )]);
        let path = tmp.path().join("recently-used.xbel");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(xbel.as_bytes())
            .unwrap();

        let parser = XbelParser::with_path(path, "/home/test".into());
        let events = parser.read_new().unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].path, "/home/test/dev/main.rs");
        assert_eq!(events[0].action, Action::Opened);
        assert_eq!(events[0].source, Source::User);
        assert_eq!(events[0].confidence, Confidence::Low);
    }

    #[test]
    fn filters_non_code_mime() {
        let tmp = tempfile::tempdir().unwrap();
        let now = unix_now();
        let recent = format_iso(now - 300);

        let xbel = make_xbel(&[
            ("file:///home/test/image.png", &recent, "image/png"),
            ("file:///home/test/doc.pdf", &recent, "application/pdf"),
        ]);
        let path = tmp.path().join("recently-used.xbel");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(xbel.as_bytes())
            .unwrap();

        let parser = XbelParser::with_path(path, "/home/test".into());
        let events = parser.read_new().unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn filters_old_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let now = unix_now();
        let old = format_iso(now - 100000); // ~27 hours ago

        let xbel = make_xbel(&[(
            "file:///home/test/dev/old.rs",
            &old,
            "text/x-rust",
        )]);
        let path = tmp.path().join("recently-used.xbel");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(xbel.as_bytes())
            .unwrap();

        let parser = XbelParser::with_path(path, "/home/test".into());
        let events = parser.read_new().unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn filters_outside_home() {
        let tmp = tempfile::tempdir().unwrap();
        let now = unix_now();
        let recent = format_iso(now - 300);

        let xbel = make_xbel(&[(
            "file:///usr/share/doc/readme.txt",
            &recent,
            "text/plain",
        )]);
        let path = tmp.path().join("recently-used.xbel");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(xbel.as_bytes())
            .unwrap();

        let parser = XbelParser::with_path(path, "/home/test".into());
        let events = parser.read_new().unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn fallback_to_extension_mime() {
        let tmp = tempfile::tempdir().unwrap();
        let now = unix_now();
        let recent = format_iso(now - 300);

        // MIME is wrong/missing but extension is a code file
        let xbel = make_xbel(&[(
            "file:///home/test/dev/script.sh",
            &recent,
            "application/octet-stream", // wrong MIME
        )]);
        let path = tmp.path().join("recently-used.xbel");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(xbel.as_bytes())
            .unwrap();

        let parser = XbelParser::with_path(path, "/home/test".into());
        let events = parser.read_new().unwrap();
        // Should still be included because is_code_file("script.sh") is true
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn percent_decoded_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let now = unix_now();
        let recent = format_iso(now - 300);

        let xbel = make_xbel(&[(
            "file:///home/test/my%20project/main.rs",
            &recent,
            "text/x-rust",
        )]);
        let path = tmp.path().join("recently-used.xbel");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(xbel.as_bytes())
            .unwrap();

        let parser = XbelParser::with_path(path, "/home/test".into());
        let events = parser.read_new().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].path, "/home/test/my project/main.rs");
    }

    #[test]
    fn missing_file_returns_empty() {
        let parser =
            XbelParser::with_path("/nonexistent/recently-used.xbel".into(), "/home/test".into());
        let events = parser.read_new().unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn extract_attr_basic() {
        assert_eq!(
            extract_attr(r#"href="file:///test" visited="2024""#, "href"),
            Some("file:///test".into())
        );
        assert_eq!(
            extract_attr(r#"href="file:///test" visited="2024""#, "visited"),
            Some("2024".into())
        );
        assert_eq!(extract_attr(r#"href="test""#, "missing"), None);
    }

    #[test]
    fn extract_mime_type_basic() {
        let block = r#"<info><metadata><mime:mime-type type="text/x-rust"/></metadata></info>"#;
        assert_eq!(extract_mime_type(block), Some("text/x-rust".into()));
    }

    #[test]
    fn skips_build_artifacts() {
        let tmp = tempfile::tempdir().unwrap();
        let now = unix_now();
        let recent = format_iso(now - 300);

        let xbel = make_xbel(&[
            (
                "file:///home/test/node_modules/pkg/index.js",
                &recent,
                "text/javascript",
            ),
            (
                "file:///home/test/dist/bundle.js",
                &recent,
                "text/javascript",
            ),
            (
                "file:///home/test/dev/Cargo.lock",
                &recent,
                "text/plain",
            ),
        ]);
        let path = tmp.path().join("recently-used.xbel");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(xbel.as_bytes())
            .unwrap();

        let parser = XbelParser::with_path(path, "/home/test".into());
        let events = parser.read_new().unwrap();
        assert!(events.is_empty());
    }
}
