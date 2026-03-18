use serde::{Deserialize, Serialize};

/// File action agent/user took
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Opened,
    Modified,
    Created,
    Deleted,
}

impl Action {
    /// All variants for iteration
    pub const ALL: &[Action] = &[
        Action::Opened,
        Action::Modified,
        Action::Created,
        Action::Deleted,
    ];

    /// Zero-allocation string representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Action::Opened => "opened",
            Action::Modified => "modified",
            Action::Created => "created",
            Action::Deleted => "deleted",
        }
    }
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Who touched the file
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Claude,
    Codex,
    User,
    System,
}

impl Source {
    /// All variants for iteration
    pub const ALL: &[Source] = &[Source::Claude, Source::Codex, Source::User, Source::System];

    /// Zero-allocation string representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Source::Claude => "claude",
            Source::Codex => "codex",
            Source::User => "user",
            Source::System => "system",
        }
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Filter by source — "all" shows everything
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Filter {
    All,
    Claude,
    Codex,
    User,
    System,
}

/// Filter by action — "all" shows everything
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionFilter {
    All,
    Opened,
    Modified,
    Created,
    Deleted,
}

/// Confidence level for event attribution
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    /// Derived from explicit tool event or patch header
    #[default]
    High,
    /// Derived from heuristic (mtime, birthtime, etc.)
    Low,
}

/// Core file entry in the timeline
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HotFile {
    /// Absolute file path
    pub path: String,
    /// Basename
    pub filename: String,
    /// Shortened display directory (~ prefix, middle truncation)
    pub dir: String,
    /// Full directory path (for shift+click open folder)
    pub full_dir: String,
    /// Absolute Unix timestamp (seconds)
    pub timestamp: i64,
    /// Who touched it
    pub source: Source,
    /// MIME type (guessed from extension)
    pub mime_type: String,
    /// What happened
    pub action: Action,
    /// Attribution confidence
    #[serde(default)]
    pub confidence: Confidence,
    /// Optional JSON metadata blob
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<String>,
}

/// A file event from ingestion (before merge into HotFile)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileEvent {
    /// Absolute file path
    pub path: String,
    /// What happened
    pub action: Action,
    /// Who did it
    pub source: Source,
    /// Absolute Unix timestamp (seconds)
    pub timestamp: i64,
    /// Attribution confidence
    #[serde(default)]
    pub confidence: Confidence,
    /// Optional session ID for grouping
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// A pinned file or folder in the Pit Stop shelf
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pin {
    /// Absolute path
    pub path: String,
    /// Optional user-assigned label
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Grouping (default: "default")
    #[serde(default = "default_pin_group")]
    pub pin_group: String,
    /// Display position (for drag reorder)
    pub position: i32,
    /// When pinned (Unix timestamp)
    pub pinned_at: i64,
}

fn default_pin_group() -> String {
    "default".to_string()
}

/// Cached LLM summary for a file
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    /// File path this summary describes
    pub path: String,
    /// Summary text
    pub content: String,
    /// Model that generated it
    pub model: String,
    /// When generated (Unix timestamp)
    pub cached_at: i64,
}

/// User preference key-value
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preference {
    pub key: String,
    /// JSON-encoded value
    pub value: String,
}

/// Activity level wrapper — events per second over a rolling window
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ActivityLevel(pub f32);

impl ActivityLevel {
    pub const COLD: f32 = 1.0;
    pub const WARM: f32 = 5.0;
    pub const HOT: f32 = 15.0;

    /// Thermal state name for the heat system
    pub fn thermal_state(&self) -> &'static str {
        if self.0 <= Self::COLD {
            "cold"
        } else if self.0 <= Self::WARM {
            "warm"
        } else if self.0 <= Self::HOT {
            "hot"
        } else {
            "on_fire"
        }
    }

    /// Normalized 0.0-1.0 heat intensity for shaders
    pub fn intensity(&self) -> f32 {
        (self.0 / Self::HOT).min(1.0)
    }
}

impl Default for ActivityLevel {
    fn default() -> Self {
        ActivityLevel(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_serde_roundtrip() {
        for source in Source::ALL {
            let json = serde_json::to_string(source).unwrap();
            let back: Source = serde_json::from_str(&json).unwrap();
            assert_eq!(*source, back);
        }
    }

    #[test]
    fn action_serde_roundtrip() {
        for action in Action::ALL {
            let json = serde_json::to_string(action).unwrap();
            let back: Action = serde_json::from_str(&json).unwrap();
            assert_eq!(*action, back);
        }
    }

    #[test]
    fn source_display() {
        assert_eq!(Source::Claude.to_string(), "claude");
        assert_eq!(Source::Codex.to_string(), "codex");
        assert_eq!(Source::User.to_string(), "user");
        assert_eq!(Source::System.to_string(), "system");
    }

    #[test]
    fn action_display() {
        assert_eq!(Action::Opened.to_string(), "opened");
        assert_eq!(Action::Created.to_string(), "created");
    }

    #[test]
    fn hotfile_serde_roundtrip() {
        let file = HotFile {
            path: "/home/zack/dev/hotbar/app.tsx".into(),
            filename: "app.tsx".into(),
            dir: "~/dev/hotbar".into(),
            full_dir: "/home/zack/dev/hotbar".into(),
            timestamp: 1710500000,
            source: Source::Claude,
            mime_type: "text/typescript".into(),
            action: Action::Modified,
            confidence: Confidence::High,
            metadata: None,
        };
        let json = serde_json::to_string(&file).unwrap();
        let back: HotFile = serde_json::from_str(&json).unwrap();
        assert_eq!(file, back);
    }

    #[test]
    fn activity_level_thermal_states() {
        assert_eq!(ActivityLevel(0.0).thermal_state(), "cold");
        assert_eq!(ActivityLevel(0.5).thermal_state(), "cold");
        assert_eq!(ActivityLevel(1.0).thermal_state(), "cold");
        assert_eq!(ActivityLevel(3.0).thermal_state(), "warm");
        assert_eq!(ActivityLevel(10.0).thermal_state(), "hot");
        assert_eq!(ActivityLevel(20.0).thermal_state(), "on_fire");
    }

    #[test]
    fn activity_level_intensity_clamped() {
        assert_eq!(ActivityLevel(0.0).intensity(), 0.0);
        assert!((ActivityLevel(7.5).intensity() - 0.5).abs() < 0.01);
        assert_eq!(ActivityLevel(30.0).intensity(), 1.0);
    }

    #[test]
    fn pin_default_group() {
        let json = r#"{"path":"/test","position":0,"pinned_at":0}"#;
        let pin: Pin = serde_json::from_str(json).unwrap();
        assert_eq!(pin.pin_group, "default");
    }

    #[test]
    fn confidence_default_is_high() {
        assert_eq!(Confidence::default(), Confidence::High);
    }

    #[test]
    fn filter_serde() {
        let json = serde_json::to_string(&Filter::Claude).unwrap();
        assert_eq!(json, r#""claude""#);
        let back: Filter = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Filter::Claude);

        let json = serde_json::to_string(&Filter::All).unwrap();
        assert_eq!(json, r#""all""#);
    }
}
