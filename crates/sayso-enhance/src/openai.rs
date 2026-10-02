//! OpenAI-compatible HTTP provider: OpenRouter, OpenAI, Ollama, LM Studio,
//! llama.cpp server (plan §3 "AI enhancement", research §9).
//!
//! Servers differ in how much of the schema feature they support. The provider
//! walks a *mode ladder* until a mode works:
//!
//! 1. `json_schema`: strict `response_format` with the output schema.
//! 2. `json_object`: `response_format` JSON mode, with the schema in the system prompt.
//! 3. `tool_call`: one function, `return_text`, that the model must call.
//!
//! A mode is skipped when the server answers HTTP 400 or 422, or when the reply
//! does not match the schema. Every reply is validated again on this side. The
//! mode that worked is remembered per `(base_url, model)` for the rest of the
//! process, so later calls skip the failed tries.
//!
//! The request timeout is a deadline for the whole call, all mode tries together.

use parking_lot::Mutex;
use sayso_core::config::is_local_url;
use sayso_core::enhance::{EnhanceError, EnhanceRequest, EnhanceResponse, Enhancer, output_schema, parse_output, user_message};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

const SCHEMA_NAME: &str = "sayso_text";
const TOOL_NAME: &str = "return_text";
const DEFAULT_TEMPERATURE: f32 = 0.2;

/// How the output schema is enforced. Ordered from strictest to loosest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    JsonSchema,
    JsonObject,
    ToolCall,
}

impl Mode {
    const LADDER: [Mode; 3] = [Mode::JsonSchema, Mode::JsonObject, Mode::ToolCall];

    fn as_str(self) -> &'static str {
        match self {
            Mode::JsonSchema => "json_schema",
            Mode::JsonObject => "json_object",
            Mode::ToolCall => "tool_call",
        }
    }
}

type ModeKey = (String, String);

/// The working mode per `(base_url, model)`. Process-wide, because the app
/// builds a new enhancer for each dictation.
static MODE_CACHE: LazyLock<Mutex<HashMap<ModeKey, Mode>>> = LazyLock::new(Mutex::default);

/// Forget all remembered modes. Call it when a provider setting changes and
/// the server may now behave differently, for example after a server update.
pub fn clear_mode_cache() {
    MODE_CACHE.lock().clear();
}

/// An HTTP agent. Local servers bypass any system proxy.
pub(crate) fn agent(base_url: &str) -> ureq::Agent {
    let mut config = ureq::Agent::config_builder().user_agent("Sayso");
    if is_local_url(base_url) {
        config = config.proxy(None);
    }
    config.build().into()
}

/// Turn a transport error into an [`EnhanceError`]. `timeout_ms` is the deadline that was set.
pub(crate) fn map_ureq_error(error: ureq::Error, timeout_ms: u64) -> EnhanceError {
    match error {
        ureq::Error::Timeout(_) => EnhanceError::Timeout(timeout_ms),
        ureq::Error::Io(e) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {
            EnhanceError::Timeout(timeout_ms)
        }
        other => EnhanceError::Network(other.to_string()),
    }
}

/// A provider for any server that speaks `POST {base_url}/chat/completions`.
pub struct OpenAiCompatible {
    id: String,
    base_url: String,
    model: String,
    api_key: Option<String>,
    /// Set when the provider needs an API key that the secret store did not have.
    setup_error: Option<String>,
    zero_data_retention: bool,
    cloud: bool,
    /// None detects OpenRouter from the base URL.
    openrouter: Option<bool>,
    default_timeout: Duration,
    agent: ureq::Agent,
}

impl OpenAiCompatible {
    /// `base_url` is the API root, for example `https://openrouter.ai/api/v1`.
    pub fn new(id: impl Into<String>, base_url: impl Into<String>, model: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Self {
            id: id.into(),
            cloud: !is_local_url(&base_url),
            agent: agent(&base_url),
            base_url,
            model: model.into(),
            api_key: None,
            setup_error: None,
            zero_data_retention: false,
            openrouter: None,
            default_timeout: Duration::from_secs(4),
        }
    }

    /// Send `Authorization: Bearer <key>` with every request.
    pub fn with_api_key(mut self, key: Option<String>) -> Self {
        self.api_key = key.filter(|k| !k.is_empty());
        self
    }

    /// Make every call fail with `NotConfigured(reason)`. Used when the provider
    /// config is not complete, so the user sees the reason and Sayso inserts the raw transcript.
    pub fn with_setup_error(mut self, reason: impl Into<String>) -> Self {
        self.setup_error = Some(reason.into());
        self
    }

    /// OpenRouter only: route to zero-data-retention endpoints that do not train on prompts.
    pub fn with_zero_data_retention(mut self, on: bool) -> Self {
        self.zero_data_retention = on;
        self
    }

    /// Override the cloud flag. By default a URL that is not local counts as cloud.
    pub fn with_cloud(mut self, cloud: bool) -> Self {
        self.cloud = cloud;
        self
    }

    /// Force OpenRouter routing fields on or off. By default they are on when
    /// the host is `openrouter.ai`. Use this for a proxy in front of OpenRouter.
    pub fn with_openrouter(mut self, on: bool) -> Self {
        self.openrouter = Some(on);
        self
    }

    /// The timeout used when a request has a zero timeout.
    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    fn is_openrouter(&self) -> bool {
        if let Some(forced) = self.openrouter {
            return forced;
        }
        let rest = self.base_url.split("://").nth(1).unwrap_or(&self.base_url);
        let host = rest.split(['/', ':']).next().unwrap_or("");
        host == "openrouter.ai" || host.ends_with(".openrouter.ai")
    }

    /// Output token limit. The text should be about as long as the transcript.
    /// A bound stops a runaway model from using the whole timeout.
    fn max_tokens(request: &EnhanceRequest) -> u32 {
        let chars = request.transcript.chars().count() as u32;
        (chars / 2 + 128).clamp(256, 8192)
    }

    /// The JSON request body for one mode.
    pub(crate) fn build_body(&self, mode: Mode, model: &str, request: &EnhanceRequest) -> Value {
        let schema = output_schema();
        let mut system = request.system_prompt.clone();
        if mode == Mode::JsonObject {
            system.push_str(&format!("\n\nReply with only a JSON object that matches this JSON schema:\n{schema}"));
        }
        let mut body = json!({
            "model": model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user_message(&request.transcript) },
            ],
            "temperature": request.temperature.unwrap_or(DEFAULT_TEMPERATURE),
            "max_tokens": Self::max_tokens(request),
            "stream": false,
        });
        match mode {
            Mode::JsonSchema => {
                body["response_format"] = json!({
                    "type": "json_schema",
                    "json_schema": { "name": SCHEMA_NAME, "strict": true, "schema": schema },
                });
            }
            Mode::JsonObject => body["response_format"] = json!({ "type": "json_object" }),
            Mode::ToolCall => {
                body["tools"] = json!([{
                    "type": "function",
                    "function": { "name": TOOL_NAME, "description": "Return the final text.", "parameters": schema },
                }]);
                body["tool_choice"] = json!({ "type": "function", "function": { "name": TOOL_NAME } });
            }
        }
        if self.is_openrouter() {
            let mut routing = serde_json::Map::new();
            // Without this, OpenRouter may route to an endpoint that ignores the
            // parameter the mode depends on.
            if matches!(mode, Mode::JsonSchema | Mode::ToolCall) {
                routing.insert("require_parameters".into(), json!(true));
            }
            if self.zero_data_retention {
                routing.insert("zdr".into(), json!(true));
                routing.insert("data_collection".into(), json!("deny"));
            }
            if !routing.is_empty() {
                body["provider"] = Value::Object(routing);
            }
        }
        body
    }

    /// One request in one mode. `timeout` is what is left of the whole-call deadline.
    fn attempt(
        &self,
        mode: Mode,
        model: &str,
        request: &EnhanceRequest,
        remaining: Duration,
        timeout_ms: u64,
    ) -> Result<String, EnhanceError> {
        let body = self.build_body(mode, model, request);
        let url = format!("{}/chat/completions", self.base_url);
        let mut http = self.agent.post(&url).config().timeout_global(Some(remaining)).http_status_as_error(false).build();
        if let Some(key) = &self.api_key {
            http = http.header("Authorization", format!("Bearer {key}"));
        }
        let mut response = http.send_json(&body).map_err(|e| map_ureq_error(e, timeout_ms))?;
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().map_err(|e| map_ureq_error(e, timeout_ms))?;
        if status >= 400 {
            return Err(EnhanceError::Http { status, body: text.chars().take(500).collect() });
        }
        let reply: Value = serde_json::from_str(&text)
            .map_err(|e| EnhanceError::InvalidOutput(format!("the server reply is not JSON: {e}")))?;
        parse_reply(mode, &reply)
    }
}

/// Should the ladder try the next mode after this error?
fn tries_next_mode(error: &EnhanceError) -> bool {
    matches!(error, EnhanceError::Http { status: 400 | 422, .. } | EnhanceError::InvalidOutput(_))
}

/// Read the text out of a chat completion reply and validate it.
pub(crate) fn parse_reply(mode: Mode, reply: &Value) -> Result<String, EnhanceError> {
    // Some gateways, OpenRouter among them, answer HTTP 200 with an error body.
    if reply.get("choices").is_none_or(|c| c.as_array().is_some_and(Vec::is_empty))
        && let Some(error) = reply.get("error")
    {
        let status = error["code"].as_u64().filter(|c| (400..600).contains(c)).unwrap_or(502) as u16;
        return Err(EnhanceError::Http { status, body: error.to_string().chars().take(500).collect() });
    }
    let choice = reply
        .pointer("/choices/0")
        .ok_or_else(|| EnhanceError::InvalidOutput("the reply has no choices".into()))?;
    let message = &choice["message"];
    if let Some(refusal) = message["refusal"].as_str().filter(|r| !r.trim().is_empty()) {
        return Err(EnhanceError::Refused(refusal.to_string()));
    }
    if choice["finish_reason"] == "length" {
        return Err(EnhanceError::InvalidOutput("the reply was cut off by the token limit".into()));
    }
    if mode == Mode::ToolCall
        && let Some(arguments) = message.pointer("/tool_calls/0/function/arguments").and_then(Value::as_str)
    {
        return parse_output(arguments);
    }
    // Plain content. A tool-call reply that came back as content is accepted too.
    let content = match &message["content"] {
        Value::String(s) => s.clone(),
        // Some servers return a list of parts.
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join(""),
        _ => String::new(),
    };
    if content.trim().is_empty() {
        return Err(EnhanceError::InvalidOutput("the reply has no content".into()));
    }
    parse_output(&content)
}

impl Enhancer for OpenAiCompatible {
    fn id(&self) -> &str {
        &self.id
    }

    fn is_cloud(&self) -> bool {
        self.cloud
    }

    fn enhance(&self, request: &EnhanceRequest) -> Result<EnhanceResponse, EnhanceError> {
        if let Some(reason) = &self.setup_error {
            return Err(EnhanceError::NotConfigured(reason.clone()));
        }
        let started = Instant::now();
        let timeout = if request.timeout.is_zero() { self.default_timeout } else { request.timeout };
        let timeout_ms = timeout.as_millis() as u64;
        let deadline = started + timeout;
        let model = request.model.clone().unwrap_or_else(|| self.model.clone());
        let key: ModeKey = (self.base_url.clone(), model.clone());

        let cached = MODE_CACHE.lock().get(&key).copied();
        let first = cached.and_then(|m| Mode::LADDER.iter().position(|&x| x == m)).unwrap_or(0);
        let mut last_error = None;
        for &mode in &Mode::LADDER[first..] {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(EnhanceError::Timeout(timeout_ms));
            }
            match self.attempt(mode, &model, request, remaining, timeout_ms) {
                Ok(text) => {
                    MODE_CACHE.lock().insert(key, mode);
                    return Ok(EnhanceResponse {
                        text,
                        provider_id: self.id.clone(),
                        model,
                        elapsed_ms: started.elapsed().as_millis() as u64,
                        mode: mode.as_str().into(),
                    });
                }
                Err(e) if tries_next_mode(&e) => {
                    log::info!("{}: mode {} failed ({e}), trying the next mode", self.id, mode.as_str());
                    last_error = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        // Every mode failed. Forget the cached mode, so the next call starts from the top.
        MODE_CACHE.lock().remove(&key);
        Err(last_error.unwrap_or_else(|| EnhanceError::InvalidOutput("no mode worked".into())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(text: &str) -> EnhanceRequest {
        EnhanceRequest {
            system_prompt: "Fix grammar.".into(),
            transcript: text.into(),
            model: None,
            temperature: None,
            timeout: Duration::from_secs(1),
        }
    }

    #[test]
    fn json_schema_body_is_strict_and_has_defaults() {
        let p = OpenAiCompatible::new("p", "http://localhost:1234/v1/", "m");
        let body = p.build_body(Mode::JsonSchema, "m", &request("hi"));
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(body["response_format"]["json_schema"]["strict"], true);
        assert_eq!(body["response_format"]["json_schema"]["name"], "sayso_text");
        assert_eq!(body["response_format"]["json_schema"]["schema"], output_schema());
        assert!((body["temperature"].as_f64().unwrap() - 0.2).abs() < 1e-6);
        assert_eq!(body["messages"][0]["role"], "system");
        // The transcript goes in tags, so the model treats it as data.
        assert_eq!(body["messages"][1]["content"], "<transcript>\nhi\n</transcript>");
        assert!(body.get("provider").is_none(), "no routing fields for other servers");
    }

    #[test]
    fn json_object_body_puts_the_schema_in_the_system_prompt() {
        let p = OpenAiCompatible::new("p", "http://localhost:1234/v1", "m");
        let body = p.build_body(Mode::JsonObject, "m", &request("hi"));
        assert_eq!(body["response_format"], json!({"type": "json_object"}));
        let system = body["messages"][0]["content"].as_str().unwrap();
        assert!(system.starts_with("Fix grammar.") && system.contains("\"additionalProperties\":false"));
    }

    #[test]
    fn tool_body_forces_the_function() {
        let p = OpenAiCompatible::new("p", "http://localhost:1234/v1", "m");
        let body = p.build_body(Mode::ToolCall, "m", &request("hi"));
        assert_eq!(body["tools"][0]["function"]["name"], "return_text");
        assert_eq!(body["tools"][0]["function"]["parameters"], output_schema());
        assert_eq!(body["tool_choice"]["function"]["name"], "return_text");
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn openrouter_routing_fields() {
        let p = OpenAiCompatible::new("p", "https://openrouter.ai/api/v1", "m");
        let body = p.build_body(Mode::JsonSchema, "m", &request("hi"));
        assert_eq!(body["provider"], json!({"require_parameters": true}));
        let json_object = p.build_body(Mode::JsonObject, "m", &request("hi"));
        assert!(json_object.get("provider").is_none());

        let zdr = OpenAiCompatible::new("p", "https://openrouter.ai/api/v1", "m").with_zero_data_retention(true);
        let body = zdr.build_body(Mode::JsonObject, "m", &request("hi"));
        assert_eq!(body["provider"], json!({"zdr": true, "data_collection": "deny"}));

        let other = OpenAiCompatible::new("p", "https://api.openai.com/v1", "m").with_zero_data_retention(true);
        assert!(other.build_body(Mode::JsonSchema, "m", &request("hi")).get("provider").is_none());
    }

    #[test]
    fn max_tokens_is_bounded() {
        assert_eq!(OpenAiCompatible::max_tokens(&request("short")), 256);
        assert_eq!(OpenAiCompatible::max_tokens(&request(&"a".repeat(1000))), 628);
        assert_eq!(OpenAiCompatible::max_tokens(&request(&"a".repeat(100_000))), 8192);
    }

    #[test]
    fn cloud_flag_follows_the_url() {
        assert!(OpenAiCompatible::new("p", "https://api.openai.com/v1", "m").is_cloud());
        assert!(!OpenAiCompatible::new("p", "http://localhost:11434/v1", "m").is_cloud());
        assert!(!OpenAiCompatible::new("p", "https://api.openai.com/v1", "m").with_cloud(false).is_cloud());
    }

    #[test]
    fn parses_content_tool_calls_refusals_and_errors() {
        let content = json!({"choices":[{"message":{"content":"{\"text\":\"Hi.\"}"},"finish_reason":"stop"}]});
        assert_eq!(parse_reply(Mode::JsonSchema, &content).unwrap(), "Hi.");
        let parts = json!({"choices":[{"message":{"content":[{"type":"text","text":"{\"text\":\"Hi\"}"}]}}]});
        assert_eq!(parse_reply(Mode::JsonObject, &parts).unwrap(), "Hi");
        let tool = json!({"choices":[{"message":{"content":null,"tool_calls":[{"function":{"name":"return_text","arguments":"{\"text\":\"Yo\"}"}}]}}]});
        assert_eq!(parse_reply(Mode::ToolCall, &tool).unwrap(), "Yo");
        let plain = json!({"choices":[{"message":{"content":"Just text, no JSON."}}]});
        assert!(matches!(parse_reply(Mode::JsonSchema, &plain), Err(EnhanceError::InvalidOutput(_))));
        let refusal = json!({"choices":[{"message":{"content":null,"refusal":"I cannot help."}}]});
        assert!(matches!(parse_reply(Mode::JsonSchema, &refusal), Err(EnhanceError::Refused(_))));
        let cut = json!({"choices":[{"message":{"content":"{\"text\":\"abc"},"finish_reason":"length"}]});
        assert!(matches!(parse_reply(Mode::JsonSchema, &cut), Err(EnhanceError::InvalidOutput(_))));
        let gateway = json!({"error":{"code":429,"message":"rate limited"}});
        assert!(matches!(parse_reply(Mode::JsonSchema, &gateway), Err(EnhanceError::Http { status: 429, .. })));
    }
}
