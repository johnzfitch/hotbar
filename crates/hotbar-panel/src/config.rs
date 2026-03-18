//! Configuration loader for hotbar.
//!
//! Reads `$XDG_CONFIG_HOME/hotbar/config.toml` and provides typed defaults
//! for all configurable values. Missing keys fall back to sane defaults.

use std::path::{Path, PathBuf};

use hotbar_daemon::inference::InferenceConfig;

/// Top-level hotbar configuration.
#[derive(Debug, Clone)]
pub struct HotbarConfig {
    /// Panel width in pixels
    pub panel_width: u32,
    /// Panel margin from screen edge
    pub panel_margin: i32,
    /// Inference backend settings
    pub inference: InferenceConfig,
    /// Plugin directory
    pub plugin_dir: PathBuf,
    /// IPC socket path
    pub socket_path: PathBuf,
    /// SQLite database path
    pub db_path: PathBuf,
    /// Claude Code events.jsonl path
    pub claude_events_path: PathBuf,
}

impl HotbarConfig {
    /// Load config from the default XDG path, falling back to defaults for missing keys.
    pub fn load() -> Self {
        let config_path = config_dir().join("config.toml");
        if config_path.exists() {
            match std::fs::read_to_string(&config_path) {
                Ok(contents) => return Self::from_toml_str(&contents),
                Err(e) => {
                    tracing::warn!(
                        path = %config_path.display(),
                        error = %e,
                        "failed to read config, using defaults"
                    );
                }
            }
        }
        Self::default()
    }

    /// Load config from a specific path.
    pub fn load_from(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(contents) => Self::from_toml_str(&contents),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to read config, using defaults"
                );
                Self::default()
            }
        }
    }

    /// Parse config from a TOML string.
    fn from_toml_str(s: &str) -> Self {
        let table: toml::Value = match s.parse() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "failed to parse config.toml, using defaults");
                return Self::default();
            }
        };

        let panel_width = table
            .get("theme")
            .and_then(|t| t.get("panel_width"))
            .and_then(|v| v.as_integer())
            .map(|v| v as u32)
            .unwrap_or(420);

        let panel_margin = table
            .get("theme")
            .and_then(|t| t.get("panel_margin"))
            .and_then(|v| v.as_integer())
            .map(|v| v as i32)
            .unwrap_or(8);

        let inference = table
            .get("inference")
            .map(InferenceConfig::from_toml)
            .unwrap_or_default();

        let plugin_dir = table
            .get("plugins")
            .and_then(|t| t.get("dir"))
            .and_then(|v| v.as_str())
            .map(expand_path)
            .unwrap_or_else(|| config_dir().join("plugins"));

        let socket_path = table
            .get("socket_path")
            .and_then(|v| v.as_str())
            .map(expand_path)
            .unwrap_or_else(default_socket_path);

        let db_path = table
            .get("db_path")
            .and_then(|v| v.as_str())
            .map(expand_path)
            .unwrap_or_else(default_db_path);

        let claude_events_path = table
            .get("claude_events_path")
            .and_then(|v| v.as_str())
            .map(expand_path)
            .unwrap_or_else(default_claude_events_path);

        Self {
            panel_width,
            panel_margin,
            inference,
            plugin_dir,
            socket_path,
            db_path,
            claude_events_path,
        }
    }
}

impl Default for HotbarConfig {
    fn default() -> Self {
        Self {
            panel_width: 420,
            panel_margin: 8,
            inference: InferenceConfig::default(),
            plugin_dir: config_dir().join("plugins"),
            socket_path: default_socket_path(),
            db_path: default_db_path(),
            claude_events_path: default_claude_events_path(),
        }
    }
}

/// XDG config directory for hotbar.
fn config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            PathBuf::from(home).join(".config")
        })
        .join("hotbar")
}

/// XDG data directory for hotbar.
fn data_dir() -> PathBuf {
    std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            PathBuf::from(home).join(".local/share")
        })
        .join("hotbar")
}

/// Default IPC socket path.
fn default_socket_path() -> PathBuf {
    let runtime_dir =
        std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(runtime_dir).join("hotbar.sock")
}

/// Default database path.
fn default_db_path() -> PathBuf {
    data_dir().join("hotbar.db")
}

/// Default Claude Code events.jsonl path.
fn default_claude_events_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".claude/projects/events.jsonl")
}

/// Expand `$HOME` and `$XDG_CONFIG_HOME` in a path string.
fn expand_path(s: &str) -> PathBuf {
    let expanded = s
        .replace(
            "$HOME",
            &std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()),
        )
        .replace(
            "$XDG_CONFIG_HOME",
            &std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                format!("{home}/.config")
            }),
        )
        .replace(
            "$XDG_RUNTIME_DIR",
            &std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into()),
        )
        .replace(
            "$XDG_DATA_HOME",
            &std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                format!("{home}/.local/share")
            }),
        );
    PathBuf::from(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        let config = HotbarConfig::default();
        assert_eq!(config.panel_width, 420);
        assert_eq!(config.panel_margin, 8);
    }

    #[test]
    fn parse_minimal_toml() {
        let config = HotbarConfig::from_toml_str("");
        assert_eq!(config.panel_width, 420);
    }

    #[test]
    fn parse_full_toml() {
        let toml = r#"
[theme]
panel_width = 500
panel_margin = 12

[inference]
backend = "ollama"
ollama_url = "http://localhost:11434"
ollama_model = "qwen2.5-coder:1.5b"

[plugins]
dir = "/tmp/hotbar-plugins"
"#;
        let config = HotbarConfig::from_toml_str(toml);
        assert_eq!(config.panel_width, 500);
        assert_eq!(config.panel_margin, 12);
        assert_eq!(config.plugin_dir, PathBuf::from("/tmp/hotbar-plugins"));
    }
}
