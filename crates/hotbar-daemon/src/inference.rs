use std::io;

use crate::db::Db;

/// Inference backend selection
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferenceBackend {
    /// Local ollama server via HTTP
    Ollama,
    /// Burn ONNX runtime (future)
    Burn,
    /// Disabled
    None,
}

impl InferenceBackend {
    /// Parse from a config string value.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "ollama" => Self::Ollama,
            "burn" => Self::Burn,
            _ => Self::None,
        }
    }
}

/// Configuration for the inference module.
#[derive(Debug, Clone)]
pub struct InferenceConfig {
    /// Which backend to use
    pub backend: InferenceBackend,
    /// Ollama server URL (e.g. `http://localhost:11434`)
    pub ollama_url: String,
    /// Ollama model name (e.g. `qwen2.5-coder:7b`)
    pub ollama_model: String,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            backend: InferenceBackend::None,
            ollama_url: "http://localhost:11434".into(),
            ollama_model: "qwen2.5-coder:7b".into(),
        }
    }
}

impl InferenceConfig {
    /// Parse from a TOML value (the `[inference]` section).
    pub fn from_toml(table: &toml::Value) -> Self {
        let backend = table
            .get("backend")
            .and_then(|v| v.as_str())
            .map(InferenceBackend::parse)
            .unwrap_or(InferenceBackend::None);

        let ollama_url = table
            .get("ollama_url")
            .and_then(|v| v.as_str())
            .unwrap_or("http://localhost:11434")
            .to_string();

        let ollama_model = table
            .get("ollama_model")
            .and_then(|v| v.as_str())
            .unwrap_or("qwen2.5-coder:7b")
            .to_string();

        Self {
            backend,
            ollama_url,
            ollama_model,
        }
    }
}

/// Inference error types
#[derive(Debug, thiserror::Error)]
pub enum InferenceError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("database error: {0}")]
    Db(#[from] crate::db::DbError),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("connection failed: {0}")]
    Connection(String),

    #[error("ollama error: {0}")]
    OllamaError(String),

    #[error("backend not available: {0}")]
    NotAvailable(String),

    #[error("inference timed out")]
    Timeout,

    #[error("file too large to summarize: {0} bytes")]
    FileTooLarge(u64),
}

/// System prompt for file summarization
const SYSTEM_PROMPT: &str =
    "Summarize this source file in 2-3 sentences. Focus on purpose, key abstractions, and dependencies.";

/// Max file size to read for summarization (64KB)
const MAX_FILE_SIZE: u64 = 65536;

/// Max content to send to model (8KB)
const MAX_PROMPT_CONTENT: usize = 8000;

/// Inference timeout
const INFERENCE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// File summarizer using configurable LLM backends.
///
/// Currently supports ollama (localhost HTTP). Burn ONNX support is planned
/// for a future phase. Results are cached in the database.
pub struct Summarizer {
    config: InferenceConfig,
}

impl Summarizer {
    /// Create a new summarizer with the given config.
    pub fn new(config: InferenceConfig) -> Self {
        Self { config }
    }

    /// Get the configured backend.
    pub fn backend(&self) -> &InferenceBackend {
        &self.config.backend
    }

    /// Run inference on a file without database access.
    ///
    /// Returns `(summary_text, model_name)`. Caller is responsible for
    /// cache lookup and storage. This avoids holding a Db lock across
    /// async boundaries (Db contains RefCell and is !Sync).
    pub async fn infer(&self, path: &str) -> Result<(String, String), InferenceError> {
        match self.config.backend {
            InferenceBackend::Ollama => {}
            InferenceBackend::Burn => {
                return Err(InferenceError::NotAvailable(
                    "burn backend not yet implemented; use 'ollama' or 'none'".into(),
                ));
            }
            InferenceBackend::None => {
                return Err(InferenceError::NotAvailable(
                    "inference disabled in config".into(),
                ));
            }
        }

        let metadata = tokio::fs::metadata(path).await?;
        if metadata.len() > MAX_FILE_SIZE {
            return Err(InferenceError::FileTooLarge(metadata.len()));
        }

        let content = tokio::fs::read_to_string(path).await?;
        let truncated = if content.len() > MAX_PROMPT_CONTENT {
            let mut end = MAX_PROMPT_CONTENT;
            while !content.is_char_boundary(end) {
                end -= 1;
            }
            &content[..end]
        } else {
            &content
        };

        let summary = tokio::time::timeout(
            INFERENCE_TIMEOUT,
            ollama_generate(&self.config.ollama_url, &self.config.ollama_model, truncated),
        )
        .await
        .map_err(|_| InferenceError::Timeout)??;

        let model = self.config.ollama_model.clone();
        tracing::debug!(path, model = %model, "inference complete");
        Ok((summary, model))
    }

    /// Summarize a file, returning the summary text.
    ///
    /// Checks the database cache first. On cache miss, reads the file,
    /// runs inference, and caches the result.
    pub async fn summarize(&self, path: &str, db: &Db) -> Result<String, InferenceError> {
        // Check cache
        if let Some(summary) = db.get_summary(path)? {
            tracing::debug!(path, "summary cache hit");
            return Ok(summary.content);
        }

        match self.config.backend {
            InferenceBackend::Ollama => {}
            InferenceBackend::Burn => {
                return Err(InferenceError::NotAvailable(
                    "burn backend not yet implemented; use 'ollama' or 'none'".into(),
                ));
            }
            InferenceBackend::None => {
                return Err(InferenceError::NotAvailable(
                    "inference disabled in config".into(),
                ));
            }
        }

        // Read file content
        let metadata = tokio::fs::metadata(path).await?;
        if metadata.len() > MAX_FILE_SIZE {
            return Err(InferenceError::FileTooLarge(metadata.len()));
        }

        let content = tokio::fs::read_to_string(path).await?;

        // Truncate for model context window
        let truncated = if content.len() > MAX_PROMPT_CONTENT {
            let mut end = MAX_PROMPT_CONTENT;
            while !content.is_char_boundary(end) {
                end -= 1;
            }
            &content[..end]
        } else {
            &content
        };

        // Run inference with timeout
        let summary = tokio::time::timeout(
            INFERENCE_TIMEOUT,
            ollama_generate(
                &self.config.ollama_url,
                &self.config.ollama_model,
                truncated,
            ),
        )
        .await
        .map_err(|_| InferenceError::Timeout)??;

        // Cache the result
        db.upsert_summary(path, &summary, &self.config.ollama_model)?;

        tracing::debug!(path, model = %self.config.ollama_model, "summary generated and cached");
        Ok(summary)
    }
}

/// Send a generate request to a local ollama instance via raw TCP HTTP.
///
/// Uses `tokio::net::TcpStream` directly — no reqwest dependency needed
/// for localhost HTTP. Ollama with `stream: false` returns a single
/// JSON response with Content-Length (no chunked encoding).
async fn ollama_generate(
    base_url: &str,
    model: &str,
    file_content: &str,
) -> Result<String, InferenceError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    tracing::debug!(
        url = base_url,
        model,
        content_len = file_content.len(),
        "ollama inference request"
    );

    // Parse host:port from URL
    let url = base_url
        .strip_prefix("http://")
        .unwrap_or(base_url);
    let host_port = url.split('/').next().unwrap_or(url);

    let body = serde_json::json!({
        "model": model,
        "system": SYSTEM_PROMPT,
        "prompt": file_content,
        "stream": false,
    });
    let body_bytes = serde_json::to_vec(&body)?;

    let mut stream = TcpStream::connect(host_port)
        .await
        .map_err(|e| InferenceError::Connection(format!("{host_port}: {e}")))?;

    // Build raw HTTP/1.1 request
    let request = format!(
        "POST /api/generate HTTP/1.1\r\n\
         Host: {host_port}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        body_bytes.len()
    );

    stream.write_all(request.as_bytes()).await?;
    stream.write_all(&body_bytes).await?;

    // Read full response (Connection: close ensures the server closes after response)
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;

    // Parse HTTP response: split headers from body at blank line
    let response_str = String::from_utf8_lossy(&response);
    let body_start = response_str
        .find("\r\n\r\n")
        .ok_or_else(|| InferenceError::OllamaError("malformed HTTP response".into()))?
        + 4;

    // Check status line
    let status_line = response_str
        .lines()
        .next()
        .unwrap_or("");
    if !status_line.contains("200") {
        return Err(InferenceError::OllamaError(format!(
            "HTTP error: {status_line}"
        )));
    }

    let body = &response_str[body_start..];
    let result: serde_json::Value = serde_json::from_str(body.trim())
        .map_err(|e| InferenceError::OllamaError(format!("response JSON parse: {e}")))?;

    result["response"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| InferenceError::OllamaError("missing 'response' field in ollama output".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_from_str() {
        assert_eq!(InferenceBackend::parse("ollama"), InferenceBackend::Ollama);
        assert_eq!(InferenceBackend::parse("burn"), InferenceBackend::Burn);
        assert_eq!(InferenceBackend::parse("none"), InferenceBackend::None);
        assert_eq!(InferenceBackend::parse("OLLAMA"), InferenceBackend::Ollama);
        assert_eq!(InferenceBackend::parse("invalid"), InferenceBackend::None);
        assert_eq!(InferenceBackend::parse(""), InferenceBackend::None);
    }

    #[test]
    fn config_default() {
        let config = InferenceConfig::default();
        assert_eq!(config.backend, InferenceBackend::None);
        assert_eq!(config.ollama_url, "http://localhost:11434");
        assert_eq!(config.ollama_model, "qwen2.5-coder:7b");
    }

    #[test]
    fn config_from_toml() {
        let toml_str = r#"
            backend = "ollama"
            ollama_url = "http://localhost:11434"
            ollama_model = "phi-3"
        "#;
        let value: toml::Value = toml::from_str(toml_str).unwrap();
        let config = InferenceConfig::from_toml(&value);

        assert_eq!(config.backend, InferenceBackend::Ollama);
        assert_eq!(config.ollama_url, "http://localhost:11434");
        assert_eq!(config.ollama_model, "phi-3");
    }

    #[test]
    fn config_from_toml_defaults() {
        let value: toml::Value = toml::from_str("").unwrap();
        let config = InferenceConfig::from_toml(&value);

        assert_eq!(config.backend, InferenceBackend::None);
        assert_eq!(config.ollama_url, "http://localhost:11434");
    }

    #[tokio::test]
    async fn summarize_none_backend() {
        let config = InferenceConfig {
            backend: InferenceBackend::None,
            ..InferenceConfig::default()
        };
        let summarizer = Summarizer::new(config);
        let db = Db::open_in_memory().unwrap();

        let result = summarizer.summarize("/nonexistent.rs", &db).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InferenceError::NotAvailable(_)));
    }

    #[tokio::test]
    async fn summarize_burn_backend() {
        let config = InferenceConfig {
            backend: InferenceBackend::Burn,
            ..InferenceConfig::default()
        };
        let summarizer = Summarizer::new(config);
        let db = Db::open_in_memory().unwrap();

        let result = summarizer.summarize("/nonexistent.rs", &db).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InferenceError::NotAvailable(_)));
    }

    #[tokio::test]
    async fn summarize_returns_cached() {
        let config = InferenceConfig::default(); // "none" backend
        let summarizer = Summarizer::new(config);
        let db = Db::open_in_memory().unwrap();

        // Pre-cache a summary
        db.upsert_summary("/test.rs", "A test file for unit testing.", "test-model")
            .unwrap();

        // Even with "none" backend, cached summary is returned
        let result = summarizer.summarize("/test.rs", &db).await.unwrap();
        assert_eq!(result, "A test file for unit testing.");
    }

    #[tokio::test]
    async fn summarize_ollama_connection_refused() {
        // Ollama isn't running on this port
        let config = InferenceConfig {
            backend: InferenceBackend::Ollama,
            ollama_url: "http://127.0.0.1:19999".into(), // unlikely port
            ollama_model: "test".into(),
        };
        let summarizer = Summarizer::new(config);
        let db = Db::open_in_memory().unwrap();

        // Create a temp file to summarize
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut tmp.as_file(), b"fn main() {}").unwrap();

        let result = summarizer
            .summarize(tmp.path().to_str().unwrap(), &db)
            .await;
        assert!(result.is_err());
        // Should be a connection error, not a panic
        let err = result.unwrap_err();
        assert!(
            matches!(err, InferenceError::Connection(_)),
            "expected Connection error, got: {err}"
        );
    }

    #[tokio::test]
    async fn summarize_nonexistent_file() {
        let config = InferenceConfig {
            backend: InferenceBackend::Ollama,
            ..InferenceConfig::default()
        };
        let summarizer = Summarizer::new(config);
        let db = Db::open_in_memory().unwrap();

        let result = summarizer.summarize("/nonexistent/file.rs", &db).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InferenceError::Io(_)));
    }
}
