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
    /// What the provider said about its own run. None when it says nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<ProviderReport>,
}

/// The numbers a provider gives about one run. `elapsed_ms` of the response
/// minus `run_ms` is the time the tool needed to start and to stop.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderReport {
    /// The length of the run, as the tool measured it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_ms: Option<u64>,
    /// The API time, as the tool counts it. It can be more than `run_ms`,
    /// because the tool adds the times of calls that ran together.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_ms: Option<u64>,
    /// The number of model turns. The Claude tool gives 2 for a usual run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// The part of the output tokens that the model used to think.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
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

impl From<crate::cleanup::CleanError> for EnhanceError {
    fn from(error: crate::cleanup::CleanError) -> EnhanceError {
        use crate::cleanup::CleanError;
        match error {
            CleanError::Timeout(ms) => EnhanceError::Timeout(ms),
            CleanError::NotConfigured(message) => EnhanceError::NotConfigured(message),
            CleanError::Runtime(message) => EnhanceError::Cli(message),
            CleanError::InvalidOutput(message) => EnhanceError::InvalidOutput(message),
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
    /// The thinking levels of the model, from the least thinking to the most.
    /// Empty when the model has none, and when the provider does not say.
    #[serde(default)]
    pub thinking: Vec<String>,
}

/// An AI provider. Calls block. Run them on a background thread.
pub trait Enhancer: Send + Sync {
    fn id(&self) -> &str;
    /// True when text leaves this Mac.
    fn is_cloud(&self) -> bool;
    fn enhance(&self, request: &EnhanceRequest) -> Result<EnhanceResponse, EnhanceError>;
}

/// The text of a [`LanguageModel`] and the name of the model that wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generated {
    pub text: String,
    pub model: String,
}

/// Why the language model of the engine cannot run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable {
    /// The stable code, when the engine sent a known one.
    pub reason: Option<crate::stt::EngineReason>,
    /// The English detail from the engine.
    pub message: String,
}

/// A language model that the engine runs on this computer, for example Apple
/// Intelligence on macOS. Calls block. Run them on a background thread.
pub trait LanguageModel: Send + Sync {
    /// The name of the model when it can run. `NotConfigured` says why it cannot.
    fn model_name(&self) -> Result<String, EnhanceError>;
    /// Like `model_name`, but a model that cannot run gives the reason code of the
    /// engine too, so the app can translate it. An engine that sends no code gives
    /// `None`.
    fn availability(&self) -> Result<String, Unavailable> {
        self.model_name().map_err(|e| Unavailable {
            reason: None,
            message: match e {
                EnhanceError::NotConfigured(message) => message,
                other => other.to_string(),
            },
        })
    }
    /// Run the model. The engine binds the reply to the `text` field of the schema.
    fn generate(
        &self,
        instructions: &str,
        prompt: &str,
        temperature: Option<f32>,
        timeout: Duration,
    ) -> Result<Generated, EnhanceError>;
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
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| EnhanceError::InvalidOutput(format!("not JSON: {e}")))?;
    parse_value(&value)
}

pub fn parse_value(value: &serde_json::Value) -> Result<String, EnhanceError> {
    let obj = value
        .as_object()
        .ok_or_else(|| EnhanceError::InvalidOutput("not a JSON object".into()))?;
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
        assert_eq!(
            parse_output("```json\n{\"text\": \"Hi\"}\n```").unwrap(),
            "Hi"
        );
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
}
