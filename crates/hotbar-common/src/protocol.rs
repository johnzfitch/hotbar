use serde::{Deserialize, Serialize};

use crate::types::{ActionFilter, ActivityLevel, Filter, HotFile, Pin};

/// Commands sent to hotbar (from bartender, CLI, scripts) via Unix socket.
/// Wire format: one JSON object per line (JSON-lines).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Toggle panel visibility
    Toggle,

    /// Quit hotbar
    Quit,

    /// Set source filter
    SetFilter {
        source: Filter,
    },

    /// Set action filter
    SetActionFilter {
        action: ActionFilter,
    },

    /// Pin a file/folder to the Pit Stop shelf
    Pin {
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },

    /// Unpin a file/folder
    Unpin {
        path: String,
    },

    /// Request LLM summary of a file
    Summarize {
        path: String,
    },

    /// Full-text search across tracked files
    Search {
        query: String,
        #[serde(default = "default_search_limit")]
        limit: usize,
    },

    /// Request current full state snapshot
    GetState,

    /// Force a refresh of all data sources
    Refresh,
}

fn default_search_limit() -> usize {
    50
}

impl Command {
    /// Short name for logging/tracing (matches the serde tag).
    pub fn name(&self) -> &'static str {
        match self {
            Self::Toggle => "toggle",
            Self::Quit => "quit",
            Self::SetFilter { .. } => "set_filter",
            Self::SetActionFilter { .. } => "set_action_filter",
            Self::Pin { .. } => "pin",
            Self::Unpin { .. } => "unpin",
            Self::Summarize { .. } => "summarize",
            Self::Search { .. } => "search",
            Self::GetState => "get_state",
            Self::Refresh => "refresh",
        }
    }
}

/// Responses from hotbar back to clients
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// Full state snapshot
    State {
        files: Vec<HotFile>,
        pins: Vec<Pin>,
        activity_level: ActivityLevel,
    },

    /// Success acknowledgement
    Ok {
        message: String,
    },

    /// Error response
    Error {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },

    /// Search results
    SearchResults {
        query: String,
        results: Vec<HotFile>,
    },

    /// Summary result
    SummaryResult {
        path: String,
        summary: String,
        model: String,
    },
}

/// State delta — sent to panel for incremental updates.
/// Not used over IPC (panel reads shared state directly via Arc<RwLock>),
/// but useful for logging and testing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Delta {
    /// Newly appearing files
    pub added: Vec<HotFile>,
    /// Files with updated timestamp/action/source
    pub updated: Vec<HotFile>,
    /// Paths that were removed (deleted files, expired from window)
    pub removed: Vec<String>,
    /// Current activity level after applying this delta
    pub activity_level: ActivityLevel,
}

impl Delta {
    /// Whether this delta contains any changes
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.updated.is_empty() && self.removed.is_empty()
    }

    /// Total number of changes
    pub fn change_count(&self) -> usize {
        self.added.len() + self.updated.len() + self.removed.len()
    }
}

/// Encode a command as a JSON-lines string (with trailing newline)
pub fn encode_command(cmd: &Command) -> Result<String, serde_json::Error> {
    let mut s = serde_json::to_string(cmd)?;
    s.push('\n');
    Ok(s)
}

/// Decode a command from a JSON line
pub fn decode_command(line: &str) -> Result<Command, serde_json::Error> {
    serde_json::from_str(line.trim())
}

/// Encode a response as a JSON-lines string (with trailing newline)
pub fn encode_response(resp: &Response) -> Result<String, serde_json::Error> {
    let mut s = serde_json::to_string(resp)?;
    s.push('\n');
    Ok(s)
}

/// Decode a response from a JSON line
pub fn decode_response(line: &str) -> Result<Response, serde_json::Error> {
    serde_json::from_str(line.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_toggle_roundtrip() {
        let cmd = Command::Toggle;
        let encoded = encode_command(&cmd).unwrap();
        assert!(encoded.ends_with('\n'));
        let decoded = decode_command(&encoded).unwrap();
        assert_eq!(cmd, decoded);
    }

    #[test]
    fn command_quit_roundtrip() {
        let cmd = Command::Quit;
        let decoded = decode_command(&encode_command(&cmd).unwrap()).unwrap();
        assert_eq!(cmd, decoded);
    }

    #[test]
    fn command_set_filter_roundtrip() {
        let cmd = Command::SetFilter {
            source: Filter::Claude,
        };
        let encoded = encode_command(&cmd).unwrap();
        assert!(encoded.contains(r#""cmd":"set_filter""#));
        assert!(encoded.contains(r#""source":"claude""#));
        let decoded = decode_command(&encoded).unwrap();
        assert_eq!(cmd, decoded);
    }

    #[test]
    fn command_pin_roundtrip() {
        let cmd = Command::Pin {
            path: "/home/zack/dev/hotbar/main.rs".into(),
            label: Some("entry point".into()),
        };
        let decoded = decode_command(&encode_command(&cmd).unwrap()).unwrap();
        assert_eq!(cmd, decoded);
    }

    #[test]
    fn command_pin_no_label_roundtrip() {
        let cmd = Command::Pin {
            path: "/test".into(),
            label: None,
        };
        let encoded = encode_command(&cmd).unwrap();
        // label should be omitted entirely
        assert!(!encoded.contains("label"));
        let decoded = decode_command(&encoded).unwrap();
        assert_eq!(cmd, decoded);
    }

    #[test]
    fn command_search_roundtrip() {
        let cmd = Command::Search {
            query: "hotfiles".into(),
            limit: 20,
        };
        let decoded = decode_command(&encode_command(&cmd).unwrap()).unwrap();
        assert_eq!(cmd, decoded);
    }

    #[test]
    fn command_search_default_limit() {
        let json = r#"{"cmd":"search","query":"test"}"#;
        let cmd: Command = decode_command(json).unwrap();
        match cmd {
            Command::Search { limit, .. } => assert_eq!(limit, 50),
            _ => panic!("expected Search"),
        }
    }

    #[test]
    fn command_summarize_roundtrip() {
        let cmd = Command::Summarize {
            path: "/home/zack/dev/hotbar/main.rs".into(),
        };
        let decoded = decode_command(&encode_command(&cmd).unwrap()).unwrap();
        assert_eq!(cmd, decoded);
    }

    #[test]
    fn command_get_state_roundtrip() {
        let cmd = Command::GetState;
        let decoded = decode_command(&encode_command(&cmd).unwrap()).unwrap();
        assert_eq!(cmd, decoded);
    }

    #[test]
    fn command_refresh_roundtrip() {
        let cmd = Command::Refresh;
        let decoded = decode_command(&encode_command(&cmd).unwrap()).unwrap();
        assert_eq!(cmd, decoded);
    }

    #[test]
    fn response_ok_roundtrip() {
        let resp = Response::Ok {
            message: "toggled".into(),
        };
        let decoded = decode_response(&encode_response(&resp).unwrap()).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn response_error_roundtrip() {
        let resp = Response::Error {
            message: "file not found".into(),
            code: Some("NOT_FOUND".into()),
        };
        let decoded = decode_response(&encode_response(&resp).unwrap()).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn response_error_no_code() {
        let resp = Response::Error {
            message: "unknown".into(),
            code: None,
        };
        let encoded = encode_response(&resp).unwrap();
        assert!(!encoded.contains("code"));
        let decoded = decode_response(&encoded).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn response_state_roundtrip() {
        let resp = Response::State {
            files: vec![],
            pins: vec![],
            activity_level: ActivityLevel(3.7),
        };
        let decoded = decode_response(&encode_response(&resp).unwrap()).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn delta_empty() {
        let d = Delta::default();
        assert!(d.is_empty());
        assert_eq!(d.change_count(), 0);
    }

    #[test]
    fn delta_not_empty() {
        let d = Delta {
            added: vec![],
            updated: vec![],
            removed: vec!["gone.rs".into()],
            activity_level: ActivityLevel(1.0),
        };
        assert!(!d.is_empty());
        assert_eq!(d.change_count(), 1);
    }

    #[test]
    fn all_command_variants_roundtrip() {
        let commands = vec![
            Command::Toggle,
            Command::Quit,
            Command::SetFilter {
                source: Filter::All,
            },
            Command::SetActionFilter {
                action: ActionFilter::Created,
            },
            Command::Pin {
                path: "/test".into(),
                label: None,
            },
            Command::Unpin {
                path: "/test".into(),
            },
            Command::Summarize {
                path: "/test".into(),
            },
            Command::Search {
                query: "q".into(),
                limit: 10,
            },
            Command::GetState,
            Command::Refresh,
        ];
        for cmd in &commands {
            let encoded = encode_command(cmd).unwrap();
            let decoded = decode_command(&encoded).unwrap();
            assert_eq!(cmd, &decoded, "roundtrip failed for {cmd:?}");
        }
    }
}
