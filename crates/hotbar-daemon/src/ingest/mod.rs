pub mod claude;
pub mod codex;
pub mod dirscan;
pub mod xbel;

/// Directories to skip during scanning
pub const SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    ".venv",
    "__pycache__",
    "target",
    "dist",
    "build",
    ".next",
    ".cache",
    ".vite",
];

/// Files to skip during scanning
pub const SKIP_FILES: &[&str] = &[
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "Cargo.lock",
    "flake.lock",
];

/// MIME prefixes considered relevant for code/text files (used by XBEL parser)
pub const RELEVANT_MIME_PREFIXES: &[&str] = &[
    "text/",
    "application/json",
    "application/javascript",
    "application/typescript",
    "application/xml",
    "application/x-shellscript",
    "application/x-python",
    "application/toml",
    "application/yaml",
    "application/x-ruby",
    "application/sql",
    "application/x-perl",
];

/// Shared ingestion error type
#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("data source not available: {0}")]
    NotAvailable(String),
}

/// Check if a path is under the given home directory
pub fn is_under_home(path: &str, home: &str) -> bool {
    path.starts_with(home)
        && (path.len() == home.len() || path.as_bytes().get(home.len()) == Some(&b'/'))
}

/// Check if a path is in a system directory (~/.codex/ or ~/.claude/)
pub fn is_system_path(path: &str, home: &str) -> bool {
    let codex_prefix = format!("{home}/.codex/");
    let claude_prefix = format!("{home}/.claude/");
    path.starts_with(&codex_prefix) || path.starts_with(&claude_prefix)
}

/// Determine source for a path. Returns None for system paths when system events are excluded.
pub fn source_for_path(
    path: &str,
    home: &str,
    non_system_source: hotbar_common::Source,
    include_system: bool,
) -> Option<hotbar_common::Source> {
    if !is_system_path(path, home) {
        return Some(non_system_source);
    }
    if include_system {
        Some(hotbar_common::Source::System)
    } else {
        None
    }
}

/// Check if a file extension indicates a code/text file
pub fn is_code_file(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    matches!(
        ext.as_str(),
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "go"
            | "rb"
            | "sh"
            | "bash"
            | "zsh"
            | "json"
            | "toml"
            | "yaml"
            | "yml"
            | "md"
            | "txt"
            | "html"
            | "css"
            | "scss"
            | "sql"
            | "xml"
            | "nix"
            | "lua"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "cc"
            | "java"
            | "kt"
            | "swift"
            | "conf"
            | "cfg"
            | "ini"
            | "env"
            | "lock"
            | "php"
            | "vue"
            | "svelte"
            | "wgsl"
            | "glsl"
            | "hlsl"
            | "gitignore"
            | "dockerignore"
            | "editorconfig"
    )
}

/// Check if a MIME type is relevant for code/text files
pub fn is_relevant_mime(mime: &str) -> bool {
    RELEVANT_MIME_PREFIXES
        .iter()
        .any(|prefix| mime.starts_with(prefix))
}

/// Check if a path should be skipped (contains build/tmp dirs or skip files)
pub fn should_skip_path(path: &str) -> bool {
    if path.contains("/tmp/") || path.contains("/node_modules/") {
        return true;
    }
    if path.contains("/dist/") || path.contains("/build/") {
        return true;
    }
    let basename = path.rsplit('/').next().unwrap_or("");
    if SKIP_FILES.contains(&basename) {
        return true;
    }
    if basename.ends_with(".min.js") || basename.ends_with(".bundle.js") {
        return true;
    }
    false
}

/// Get home directory from environment
pub fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string())
}

/// Check if HOTBAR_INCLUDE_SYSTEM_EVENTS is set
pub fn include_system_events() -> bool {
    std::env::var("HOTBAR_INCLUDE_SYSTEM_EVENTS")
        .map(|v| {
            let v = v.trim().to_lowercase();
            v == "1" || v == "true" || v == "yes" || v == "on"
        })
        .unwrap_or(false)
}

/// Current Unix timestamp in seconds
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Parse ISO 8601 timestamp to Unix seconds.
/// Handles: `2026-03-15T14:22:00.000Z`, `2026-03-15T14:22:00Z`, `2026-03-15T10:30:00Z`
pub fn parse_iso8601(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let b = s.as_bytes();

    let year: i32 = s.get(0..4)?.parse().ok()?;
    if b.get(4)? != &b'-' {
        return None;
    }
    let month: u32 = s.get(5..7)?.parse().ok()?;
    if b.get(7)? != &b'-' {
        return None;
    }
    let day: u32 = s.get(8..10)?.parse().ok()?;
    if b.get(10)? != &b'T' {
        return None;
    }
    let hour: u32 = s.get(11..13)?.parse().ok()?;
    if b.get(13)? != &b':' {
        return None;
    }
    let minute: u32 = s.get(14..16)?.parse().ok()?;
    if b.get(16)? != &b':' {
        return None;
    }
    let second: u32 = s.get(17..19)?.parse().ok()?;

    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    // Howard Hinnant's civil_from_days algorithm
    let y = if month <= 2 { year - 1 } else { year };
    let m = if month <= 2 { month + 9 } else { month - 3 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era as i64 * 146097 + doe as i64 - 719468;

    Some(days * 86400 + hour as i64 * 3600 + minute as i64 * 60 + second as i64)
}

/// Decode percent-encoded URI (e.g. `file:///path%20name` -> `/path name`)
pub fn decode_percent(s: &str) -> String {
    let mut result = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(val) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
        {
            result.push(val);
            i += 3;
            continue;
        }
        result.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(result).unwrap_or_else(|_| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_under_home_basic() {
        assert!(is_under_home("/home/zack/dev/file.rs", "/home/zack"));
        assert!(is_under_home("/home/zack", "/home/zack"));
        assert!(!is_under_home("/home/zacker", "/home/zack"));
        assert!(!is_under_home("/tmp/file", "/home/zack"));
    }

    #[test]
    fn is_system_path_basic() {
        assert!(is_system_path("/home/z/.codex/sessions/f.jsonl", "/home/z"));
        assert!(is_system_path("/home/z/.claude/events.jsonl", "/home/z"));
        assert!(!is_system_path("/home/z/dev/file.rs", "/home/z"));
    }

    #[test]
    fn source_for_path_system_excluded() {
        let src = source_for_path(
            "/home/z/.codex/sessions/f.jsonl",
            "/home/z",
            hotbar_common::Source::Codex,
            false,
        );
        assert!(src.is_none());
    }

    #[test]
    fn source_for_path_system_included() {
        let src = source_for_path(
            "/home/z/.codex/sessions/f.jsonl",
            "/home/z",
            hotbar_common::Source::Codex,
            true,
        );
        assert_eq!(src, Some(hotbar_common::Source::System));
    }

    #[test]
    fn source_for_path_non_system() {
        let src = source_for_path(
            "/home/z/dev/file.rs",
            "/home/z",
            hotbar_common::Source::Claude,
            false,
        );
        assert_eq!(src, Some(hotbar_common::Source::Claude));
    }

    #[test]
    fn is_code_file_known() {
        assert!(is_code_file("main.rs"));
        assert!(is_code_file("app.tsx"));
        assert!(!is_code_file("Dockerfile")); // no recognized ext
        assert!(is_code_file(".gitignore"));
    }

    #[test]
    fn is_code_file_unknown() {
        assert!(!is_code_file("image.png"));
        assert!(!is_code_file("data.bin"));
    }

    #[test]
    fn is_relevant_mime_basic() {
        assert!(is_relevant_mime("text/plain"));
        assert!(is_relevant_mime("text/x-rust"));
        assert!(is_relevant_mime("application/json"));
        assert!(!is_relevant_mime("image/png"));
        assert!(!is_relevant_mime("application/pdf"));
    }

    #[test]
    fn should_skip_path_basic() {
        assert!(should_skip_path("/home/z/node_modules/pkg/index.js"));
        assert!(should_skip_path("/home/z/dist/bundle.js"));
        assert!(should_skip_path("/home/z/dev/package-lock.json"));
        assert!(should_skip_path("/home/z/dev/app.min.js"));
        assert!(!should_skip_path("/home/z/dev/hotbar/main.rs"));
    }

    #[test]
    fn parse_iso8601_valid() {
        // 2026-03-15T14:22:00.000Z
        let ts = parse_iso8601("2026-03-15T14:22:00.000Z").unwrap();
        // Just verify it's in a reasonable range (March 2026)
        assert!(ts > 1773000000 && ts < 1774000000);
    }

    #[test]
    fn parse_iso8601_no_millis() {
        let ts = parse_iso8601("2026-03-15T14:22:00Z").unwrap();
        assert!(ts > 1773000000);
    }

    #[test]
    fn parse_iso8601_epoch() {
        let ts = parse_iso8601("1970-01-01T00:00:00Z").unwrap();
        assert_eq!(ts, 0);
    }

    #[test]
    fn parse_iso8601_known_value() {
        // 2024-01-01T00:00:00Z = 1704067200
        let ts = parse_iso8601("2024-01-01T00:00:00Z").unwrap();
        assert_eq!(ts, 1704067200);
    }

    #[test]
    fn parse_iso8601_invalid() {
        assert!(parse_iso8601("not a date").is_none());
        assert!(parse_iso8601("2026-13-01T00:00:00Z").is_none());
        assert!(parse_iso8601("").is_none());
    }

    #[test]
    fn decode_percent_basic() {
        assert_eq!(decode_percent("hello%20world"), "hello world");
        assert_eq!(decode_percent("no%2Fslash"), "no/slash");
        assert_eq!(decode_percent("clean"), "clean");
        assert_eq!(decode_percent("%"), "%"); // incomplete sequence left as-is
    }
}
