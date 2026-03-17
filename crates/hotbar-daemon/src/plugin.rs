use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Plugin system error types
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("plugin not found: {0}")]
    NotFound(String),

    #[error("plugin timed out: {0}")]
    Timeout(String),

    #[error("plugin exited with code {0}")]
    ExitCode(i32),

    #[error("plugin spawn failed: {0}")]
    Spawn(std::io::Error),
}

/// When a plugin should be invoked
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginTrigger {
    /// Invoked when tracked files change
    OnFileChange,
    /// Invoked when a file is pinned
    OnPin,
    /// Invoked only by explicit user action
    Manual,
}

/// Plugin manifest from `plugin.toml`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Plugin name (used for invocation)
    pub name: String,
    /// Human-readable description
    #[serde(default)]
    pub description: String,
    /// Events that trigger this plugin
    #[serde(default = "default_triggers")]
    pub triggers: Vec<PluginTrigger>,
    /// Invocation timeout in milliseconds
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_triggers() -> Vec<PluginTrigger> {
    vec![PluginTrigger::Manual]
}

fn default_timeout_ms() -> u64 {
    5000
}

/// A discovered plugin with its executable path
#[derive(Debug, Clone)]
pub struct Plugin {
    /// Parsed manifest
    pub manifest: PluginManifest,
    /// Path to the plugin executable
    pub executable: PathBuf,
}

/// Manages plugin discovery and invocation.
///
/// Plugins live in `$XDG_CONFIG_HOME/hotbar/plugins/`. Each plugin is a
/// directory containing an executable and an optional `plugin.toml` manifest.
pub struct PluginManager {
    plugins: Vec<Plugin>,
    plugin_dir: PathBuf,
}

impl PluginManager {
    /// Create a new plugin manager for the given directory.
    pub fn new(plugin_dir: PathBuf) -> Self {
        Self {
            plugins: Vec::new(),
            plugin_dir,
        }
    }

    /// Create a plugin manager using the default XDG config path.
    pub fn default_dir() -> PathBuf {
        let config = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            format!("{home}/.config")
        });
        PathBuf::from(config).join("hotbar/plugins")
    }

    /// Scan the plugin directory and populate the plugin list.
    ///
    /// Each subdirectory is checked for an executable and optional manifest.
    /// Returns the number of plugins discovered.
    pub fn discover(&mut self) -> Result<usize, PluginError> {
        self.plugins.clear();

        if !self.plugin_dir.is_dir() {
            tracing::debug!(
                path = %self.plugin_dir.display(),
                "plugin directory does not exist"
            );
            return Ok(0);
        }

        for entry in std::fs::read_dir(&self.plugin_dir)? {
            let entry = entry?;
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            match self.load_plugin(&path) {
                Ok(plugin) => {
                    tracing::debug!(
                        name = %plugin.manifest.name,
                        exe = %plugin.executable.display(),
                        "discovered plugin"
                    );
                    self.plugins.push(plugin);
                }
                Err(e) => {
                    tracing::warn!(
                        dir = %path.display(),
                        error = %e,
                        "skipping invalid plugin"
                    );
                }
            }
        }

        tracing::info!(count = self.plugins.len(), "plugin discovery complete");
        Ok(self.plugins.len())
    }

    /// Load a single plugin from its directory.
    fn load_plugin(&self, dir: &std::path::Path) -> Result<Plugin, PluginError> {
        let manifest_path = dir.join("plugin.toml");
        let manifest = if manifest_path.exists() {
            let content = std::fs::read_to_string(&manifest_path)?;
            toml::from_str(&content)?
        } else {
            let name = dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            PluginManifest {
                name,
                description: String::new(),
                triggers: default_triggers(),
                timeout_ms: default_timeout_ms(),
            }
        };

        let executable = find_executable(dir, &manifest.name)?;

        Ok(Plugin {
            manifest,
            executable,
        })
    }

    /// Invoke a plugin by name with the given JSON payload.
    ///
    /// Sends `payload` on stdin, reads JSON response from stdout.
    /// Stderr is logged via tracing. Times out after the manifest-configured
    /// duration (default 5s).
    pub async fn invoke(
        &self,
        plugin_name: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, PluginError> {
        let plugin = self
            .plugins
            .iter()
            .find(|p| p.manifest.name == plugin_name)
            .ok_or_else(|| PluginError::NotFound(plugin_name.to_string()))?;

        let timeout = std::time::Duration::from_millis(plugin.manifest.timeout_ms);

        tokio::time::timeout(timeout, run_plugin(plugin, payload))
            .await
            .map_err(|_| PluginError::Timeout(plugin.manifest.name.clone()))?
    }

    /// Get all discovered plugins.
    pub fn plugins(&self) -> &[Plugin] {
        &self.plugins
    }

    /// Get plugins that match a specific trigger.
    pub fn plugins_for_trigger(&self, trigger: &PluginTrigger) -> Vec<&Plugin> {
        self.plugins
            .iter()
            .filter(|p| p.manifest.triggers.contains(trigger))
            .collect()
    }

    /// Get the plugin directory path.
    pub fn plugin_dir(&self) -> &PathBuf {
        &self.plugin_dir
    }
}

/// Run a plugin process: write payload to stdin, read response from stdout.
async fn run_plugin(
    plugin: &Plugin,
    payload: &serde_json::Value,
) -> Result<serde_json::Value, PluginError> {
    use tokio::io::AsyncWriteExt;

    let mut child = tokio::process::Command::new(&plugin.executable)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(PluginError::Spawn)?;

    // Write payload to stdin, then drop to signal EOF
    if let Some(mut stdin) = child.stdin.take() {
        let bytes = serde_json::to_vec(payload)?;
        stdin.write_all(&bytes).await?;
        // stdin dropped here, sending EOF
    }

    let output = child.wait_with_output().await?;

    // Log stderr
    if !output.stderr.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(
            plugin = %plugin.manifest.name,
            stderr = %stderr.trim(),
            "plugin stderr"
        );
    }

    if !output.status.success() {
        return Err(PluginError::ExitCode(
            output.status.code().unwrap_or(-1),
        ));
    }

    if output.stdout.is_empty() {
        return Ok(serde_json::Value::Null);
    }

    let result: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    Ok(result)
}

/// Find the plugin executable inside a plugin directory.
///
/// Checks in order: `<name>`, `run`, `main`. The file must be executable.
fn find_executable(dir: &std::path::Path, name: &str) -> Result<PathBuf, PluginError> {
    let candidates = [dir.join(name), dir.join("run"), dir.join("main")];

    for candidate in &candidates {
        if candidate.is_file() && is_executable(candidate) {
            return Ok(candidate.clone());
        }
    }

    Err(PluginError::NotFound(format!(
        "no executable in {}",
        dir.display()
    )))
}

/// Check if a file has executable permission.
fn is_executable(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_plugin_dir(tmp: &std::path::Path, name: &str, script: &str) -> PathBuf {
        let dir = tmp.join(name);
        std::fs::create_dir_all(&dir).unwrap();

        let exe_path = dir.join("run");
        let mut file = std::fs::File::create(&exe_path).unwrap();
        file.write_all(script.as_bytes()).unwrap();

        // Make executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        dir
    }

    fn make_manifest(dir: &std::path::Path, manifest: &PluginManifest) {
        let toml = toml::to_string(manifest).unwrap();
        std::fs::write(dir.join("plugin.toml"), toml).unwrap();
    }

    #[test]
    fn discover_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let mut mgr = PluginManager::new(tmp.path().to_path_buf());
        let count = mgr.discover().unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn discover_nonexistent_dir() {
        let mut mgr = PluginManager::new(PathBuf::from("/nonexistent/plugins"));
        let count = mgr.discover().unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn discover_with_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = make_plugin_dir(tmp.path(), "test-plugin", "#!/bin/sh\necho '{}'");

        let manifest = PluginManifest {
            name: "test-plugin".into(),
            description: "A test plugin".into(),
            triggers: vec![PluginTrigger::OnFileChange, PluginTrigger::Manual],
            timeout_ms: 3000,
        };
        make_manifest(&dir, &manifest);

        let mut mgr = PluginManager::new(tmp.path().to_path_buf());
        let count = mgr.discover().unwrap();
        assert_eq!(count, 1);
        assert_eq!(mgr.plugins()[0].manifest.name, "test-plugin");
        assert_eq!(mgr.plugins()[0].manifest.timeout_ms, 3000);
    }

    #[test]
    fn discover_without_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        make_plugin_dir(tmp.path(), "auto-plugin", "#!/bin/sh\necho '{}'");

        let mut mgr = PluginManager::new(tmp.path().to_path_buf());
        let count = mgr.discover().unwrap();
        assert_eq!(count, 1);
        assert_eq!(mgr.plugins()[0].manifest.name, "auto-plugin");
        assert_eq!(mgr.plugins()[0].manifest.timeout_ms, 5000);
    }

    #[test]
    fn plugins_for_trigger_filter() {
        let tmp = tempfile::tempdir().unwrap();

        let dir1 = make_plugin_dir(tmp.path(), "p1", "#!/bin/sh\necho '{}'");
        make_manifest(
            &dir1,
            &PluginManifest {
                name: "p1".into(),
                description: String::new(),
                triggers: vec![PluginTrigger::OnFileChange],
                timeout_ms: 5000,
            },
        );

        let dir2 = make_plugin_dir(tmp.path(), "p2", "#!/bin/sh\necho '{}'");
        make_manifest(
            &dir2,
            &PluginManifest {
                name: "p2".into(),
                description: String::new(),
                triggers: vec![PluginTrigger::Manual],
                timeout_ms: 5000,
            },
        );

        let mut mgr = PluginManager::new(tmp.path().to_path_buf());
        mgr.discover().unwrap();

        let on_change = mgr.plugins_for_trigger(&PluginTrigger::OnFileChange);
        assert_eq!(on_change.len(), 1);
        assert_eq!(on_change[0].manifest.name, "p1");

        let manual = mgr.plugins_for_trigger(&PluginTrigger::Manual);
        assert_eq!(manual.len(), 1);
        assert_eq!(manual[0].manifest.name, "p2");
    }

    #[tokio::test]
    async fn invoke_echo_plugin() {
        let tmp = tempfile::tempdir().unwrap();

        // Create a plugin that echoes its input
        let script = r#"#!/bin/sh
cat
"#;
        let dir = make_plugin_dir(tmp.path(), "echo", script);
        make_manifest(
            &dir,
            &PluginManifest {
                name: "echo".into(),
                description: "echoes input".into(),
                triggers: vec![PluginTrigger::Manual],
                timeout_ms: 5000,
            },
        );

        let mut mgr = PluginManager::new(tmp.path().to_path_buf());
        mgr.discover().unwrap();

        let payload = serde_json::json!({"action": "test", "data": 42});
        let result = mgr.invoke("echo", &payload).await.unwrap();
        assert_eq!(result["action"], "test");
        assert_eq!(result["data"], 42);
    }

    #[tokio::test]
    async fn invoke_nonexistent_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = PluginManager::new(tmp.path().to_path_buf());

        let result = mgr
            .invoke("nonexistent", &serde_json::json!({}))
            .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PluginError::NotFound(_)));
    }

    #[tokio::test]
    async fn invoke_failing_plugin() {
        let tmp = tempfile::tempdir().unwrap();

        let script = "#!/bin/sh\nexit 1\n";
        let dir = make_plugin_dir(tmp.path(), "fail", script);
        make_manifest(
            &dir,
            &PluginManifest {
                name: "fail".into(),
                description: String::new(),
                triggers: vec![PluginTrigger::Manual],
                timeout_ms: 5000,
            },
        );

        let mut mgr = PluginManager::new(tmp.path().to_path_buf());
        mgr.discover().unwrap();

        let result = mgr.invoke("fail", &serde_json::json!({})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PluginError::ExitCode(1)));
    }

    #[tokio::test]
    async fn invoke_timeout() {
        let tmp = tempfile::tempdir().unwrap();

        let script = "#!/bin/sh\nsleep 10\n";
        let dir = make_plugin_dir(tmp.path(), "slow", script);
        make_manifest(
            &dir,
            &PluginManifest {
                name: "slow".into(),
                description: String::new(),
                triggers: vec![PluginTrigger::Manual],
                timeout_ms: 100, // Very short timeout
            },
        );

        let mut mgr = PluginManager::new(tmp.path().to_path_buf());
        mgr.discover().unwrap();

        let result = mgr.invoke("slow", &serde_json::json!({})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PluginError::Timeout(_)));
    }

    #[test]
    fn manifest_serde_roundtrip() {
        let manifest = PluginManifest {
            name: "test".into(),
            description: "test plugin".into(),
            triggers: vec![PluginTrigger::OnFileChange, PluginTrigger::Manual],
            timeout_ms: 3000,
        };

        let toml_str = toml::to_string(&manifest).unwrap();
        let back: PluginManifest = toml::from_str(&toml_str).unwrap();
        assert_eq!(back.name, "test");
        assert_eq!(back.triggers.len(), 2);
        assert_eq!(back.timeout_ms, 3000);
    }

    #[test]
    fn manifest_defaults() {
        let toml_str = r#"name = "minimal""#;
        let manifest: PluginManifest = toml::from_str(toml_str).unwrap();
        assert_eq!(manifest.name, "minimal");
        assert_eq!(manifest.triggers, vec![PluginTrigger::Manual]);
        assert_eq!(manifest.timeout_ms, 5000);
        assert!(manifest.description.is_empty());
    }

    #[test]
    fn is_executable_checks_permissions() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test");
        std::fs::write(&path, "#!/bin/sh").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Not executable yet
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(!is_executable(&path));

            // Make executable
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(is_executable(&path));
        }
    }

    #[test]
    fn default_dir_uses_xdg() {
        // Just verify it returns a reasonable path
        let dir = PluginManager::default_dir();
        let path_str = dir.to_string_lossy();
        assert!(path_str.contains("hotbar/plugins"));
    }
}
