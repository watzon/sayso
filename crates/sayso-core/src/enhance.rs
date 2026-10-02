//! The AI enhancement contract. Providers live in `sayso-enhance`.

use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct EnhanceRequest {
    pub system_prompt: String,
    pub transcript: String,
    /// Model override from the style. None uses the provider's model.
    pub model: Option<String>,
    pub temperature: Option<f32>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnhanceResponse {
    pub text: String,
    pub provider_id: String,
    pub model: String,
    pub elapsed_ms: u64,
    /// How the schema was enforced, for example "json_schema" or "tool_call".
    pub mode: String,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EnhanceError {
    #[error("timed out after {0} ms")]
    Timeout(u64),
    #[error("the provider is not set up: {0}")]
    NotConfigured(String),
    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("network error: {0}")]
    Network(String),
    #[error("the response did not match the schema: {0}")]
    InvalidOutput(String),
    #[error("the model refused: {0}")]
    Refused(String),
    #[error("{0}")]
    Cli(String),
}

impl EnhanceError {
    /// Short text for the overlay, for example "timed out after 4 s".
    pub fn short(&self) -> String {
        match self {
            EnhanceError::Timeout(ms) => format!("timed out after {} s", (*ms as f64 / 1000.0).round() as u64),
            EnhanceError::NotConfigured(_) => "is not set up".into(),
            EnhanceError::Http { status, .. } => format!("returned HTTP {status}"),
            EnhanceError::Network(_) => "could not be reached".into(),
            EnhanceError::InvalidOutput(_) => "returned text in the wrong format".into(),
            EnhanceError::Refused(_) => "refused the request".into(),
            EnhanceError::Cli(_) => "failed".into(),
        }
    }
}

/// One model a provider offers, for the model picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelChoice {
    /// The value Sayso sends to the provider, for example "haiku" or "openai/gpt-5-mini".
    pub id: String,
    /// The name to show, for example "Haiku 4.5". The id when the provider gives no name.
    pub name: String,
    /// A short note from the provider, for example "Fastest for quick answers".
    pub description: Option<String>,
}

/// An AI provider. Calls block. Run them on a background thread.
pub trait Enhancer: Send + Sync {
    fn id(&self) -> &str;
    /// True when text leaves this Mac.
    fn is_cloud(&self) -> bool;
    fn enhance(&self, request: &EnhanceRequest) -> Result<EnhanceResponse, EnhanceError>;
}

/// The JSON schema every provider must follow: `{"text": string}`.
pub fn output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": { "text": { "type": "string" } },
        "required": ["text"],
        "additionalProperties": false
    })
}

/// The user message for a transcript. The system prompt from
/// [`crate::style::system_prompt`] refers to these tags.
pub fn user_message(transcript: &str) -> String {
    format!("<transcript>\n{transcript}\n</transcript>")
}

/// Validate a model reply against the schema. Accepts a JSON object, also when
/// it is wrapped in a Markdown code fence.
pub fn parse_output(raw: &str) -> Result<String, EnhanceError> {
    let trimmed = raw.trim();
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|s| s.strip_suffix("```"))
        .unwrap_or(trimmed)
        .trim();
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| EnhanceError::InvalidOutput(format!("not JSON: {e}")))?;
    parse_value(&value)
}

pub fn parse_value(value: &serde_json::Value) -> Result<String, EnhanceError> {
    let obj = value.as_object().ok_or_else(|| EnhanceError::InvalidOutput("not a JSON object".into()))?;
    let text = obj
        .get("text")
        .and_then(|t| t.as_str())
        .ok_or_else(|| EnhanceError::InvalidOutput("missing string field \"text\"".into()))?;
    let text = unwrap_text(text);
    if text.trim().is_empty() {
        return Err(EnhanceError::InvalidOutput("\"text\" is empty".into()));
    }
    Ok(text)
}

/// Undo two model mistakes: a `{"text": ...}` object inside the text field,
/// and the transcript tags copied into the reply.
fn unwrap_text(text: &str) -> String {
    let mut text = text.trim();
    let nested;
    if text.starts_with('{')
        && let Ok(serde_json::Value::Object(obj)) = serde_json::from_str::<serde_json::Value>(text)
        && obj.len() == 1
        && let Some(inner) = obj.get("text").and_then(|t| t.as_str())
    {
        nested = inner.to_string();
        text = nested.trim();
    }
    let text = text.strip_prefix("<transcript>").unwrap_or(text);
    let text = text.strip_suffix("</transcript>").unwrap_or(text);
    text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_fenced_json() {
        assert_eq!(parse_output(r#"{"text":"Hello."}"#).unwrap(), "Hello.");
        assert_eq!(parse_output("```json\n{\"text\": \"Hi\"}\n```").unwrap(), "Hi");
    }

    #[test]
    fn unwraps_json_inside_the_text_field() {
        // Seen from Claude Haiku with a JSON schema and a JSON instruction in the prompt.
        let reply = serde_json::json!({ "text": "{\"text\": \"Sayso is great.\"}" });
        assert_eq!(parse_value(&reply).unwrap(), "Sayso is great.");
        // Ordinary braces in dictated text stay.
        let braces = serde_json::json!({ "text": "{a, b} is a set." });
        assert_eq!(parse_value(&braces).unwrap(), "{a, b} is a set.");
        let other = serde_json::json!({ "text": "{\"name\": \"x\"}" });
        assert_eq!(parse_value(&other).unwrap(), "{\"name\": \"x\"}");
    }

    #[test]
    fn removes_copied_transcript_tags() {
        let reply = serde_json::json!({ "text": "<transcript>\nHello.\n</transcript>" });
        assert_eq!(parse_value(&reply).unwrap(), "Hello.");
        assert_eq!(user_message("hi"), "<transcript>\nhi\n</transcript>");
    }

    #[test]
    fn rejects_wrong_shapes() {
        assert!(parse_output("Hello").is_err());
        assert!(parse_output(r#"{"txt":"x"}"#).is_err());
        assert!(parse_output(r#"{"text": 3}"#).is_err());
        assert!(parse_output(r#"{"text": "  "}"#).is_err());
        assert!(parse_output(r#"["text"]"#).is_err());
    }

    #[test]
    fn short_messages_read_well() {
        assert_eq!(EnhanceError::Timeout(4000).short(), "timed out after 4 s");
    }
}
