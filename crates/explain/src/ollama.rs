//! A minimal client for a local Ollama server (`POST /api/chat`).

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::prompt::Message;

/// Which model to ask, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelConfig {
    /// Base URL of the Ollama server, e.g. `http://127.0.0.1:11434`.
    pub url: String,
    pub model: String,
    pub timeout: Duration,
}

impl ModelConfig {
    /// From `CODEATLAS_OLLAMA_URL` (default `http://127.0.0.1:11434`) and
    /// `CODEATLAS_OLLAMA_MODEL` (default `llama3.2`). `None` when
    /// `CODEATLAS_EXPLAIN=template`, which disables the model entirely.
    pub fn from_env() -> Option<Self> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        if var("CODEATLAS_EXPLAIN").as_deref() == Some("template") {
            return None;
        }
        Some(Self {
            url: var("CODEATLAS_OLLAMA_URL").unwrap_or_else(|| "http://127.0.0.1:11434".into()),
            model: var("CODEATLAS_OLLAMA_MODEL").unwrap_or_else(|| "llama3.2".into()),
            timeout: Duration::from_secs(
                var("CODEATLAS_OLLAMA_TIMEOUT_SECS")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(120),
            ),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("no model server answered at {0}")]
    Unavailable(String),
    #[error("the model server has no model `{0}` (run `ollama pull {0}`)")]
    ModelNotFound(String),
    #[error("the model server answered {status}: {body}")]
    Http { status: u16, body: String },
    #[error("unexpected answer from the model server: {0}")]
    Invalid(String),
}

#[derive(Deserialize)]
struct ChatResponse {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: String,
}

/// Sends the messages and returns the answer. Temperature 0 and a fixed
/// seed, so the same evidence gives the same answer where the model allows.
pub async fn chat(config: &ModelConfig, messages: &[Message]) -> Result<String, ModelError> {
    let client = reqwest::Client::builder()
        .timeout(config.timeout)
        .build()
        .map_err(|e| ModelError::Invalid(e.to_string()))?;
    let url = format!("{}/api/chat", config.url.trim_end_matches('/'));
    let response = client
        .post(&url)
        .json(&json!({
            "model": config.model,
            "messages": messages,
            "stream": false,
            "options": { "temperature": 0, "seed": 0 }
        }))
        .send()
        .await
        .map_err(|e| {
            if e.is_connect() || e.is_timeout() {
                ModelError::Unavailable(config.url.clone())
            } else {
                ModelError::Invalid(e.to_string())
            }
        })?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| ModelError::Invalid(e.to_string()))?;
    if status.as_u16() == 404 && body.contains("not found") {
        return Err(ModelError::ModelNotFound(config.model.clone()));
    }
    if !status.is_success() {
        return Err(ModelError::Http {
            status: status.as_u16(),
            body: body.chars().take(300).collect(),
        });
    }
    let parsed: ChatResponse =
        serde_json::from_str(&body).map_err(|e| ModelError::Invalid(e.to_string()))?;
    Ok(parsed.message.content.trim().to_string())
}
