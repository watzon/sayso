//! The client side of the Agent Client Protocol (ACP), version 1: initialize,
//! sessions, the model choice, one prompt turn, cancel, and the sign-in of
//! the agent. Sayso gives the agent no tools: it refuses every permission,
//! file and terminal request.

use super::rpc::{CallError, Connection, Handler, RpcError};
use crate::cli::looks_like_login_problem;
use sayso_core::config::sort_thinking;
use sayso_core::enhance::{EnhanceError, ModelChoice};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The protocol version that Sayso speaks.
pub(crate) const PROTOCOL_VERSION: u64 = 1;
/// The error code of a request that needs a sign-in first.
pub(crate) const AUTH_REQUIRED: i64 = -32000;
/// The error code of a method that the receiver does not have.
pub(crate) const METHOD_NOT_FOUND: i64 = -32601;

/// The kind of a sign-in method of an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuthKind {
    /// The agent runs the sign-in itself, after an `authenticate` request.
    Agent,
    /// The user runs a command in a terminal. `command` is None when the
    /// command is the agent's own command with `args`.
    Terminal { command: Option<String>, args: Vec<String> },
    /// A kind that this version does not know.
    Other(String),
}

/// The end of a call, with the timeout that set it, for the `Timeout` error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Deadline {
    pub at: Instant,
    pub timeout: Duration,
}

impl Deadline {
    pub fn after(timeout: Duration) -> Self {
        Self { at: Instant::now() + timeout, timeout }
    }

    pub fn timeout_ms(&self) -> u64 {
        self.timeout.as_millis() as u64
    }

    /// The earlier of this deadline and one `limit` from now.
    pub fn within(self, limit: Duration) -> Self {
        let other = Deadline::after(limit);
        if other.at < self.at { other } else { self }
    }
}

/// A sign-in method of an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthMethod {
    pub id: String,
    pub name: String,
    pub kind: AuthKind,
}

/// What an agent says about itself in its `initialize` answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentInfo {
    /// The title of the agent, else its name. None when it gives neither.
    pub name: Option<String>,
    pub version: Option<String>,
    pub protocol_version: u64,
    pub auth_methods: Vec<AuthMethod>,
    /// True when the agent can close a session (`session/close`).
    pub closes_sessions: bool,
}

/// How a session changes its model.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ModelControl {
    /// `session/set_model`, of agents that send `models` (an older, unstable part of the protocol).
    SetModel,
    /// `session/set_config_option` with the config option of the `model` category.
    ConfigOption(String),
}

/// A session that `session/new` opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Session {
    pub id: String,
    /// The models of the session, in the order of the agent.
    pub models: Vec<ModelChoice>,
    pub current_model: Option<String>,
    control: Option<ModelControl>,
}

/// What a prompt turn gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Turn {
    /// The text of the agent message chunks, joined.
    pub text: String,
}

/// An initialized connection to one agent.
pub(crate) struct AcpClient {
    conn: Connection,
    info: AgentInfo,
    /// The name for messages: the agent's name, else the command.
    label: String,
    /// Set when the agent can no longer be trusted with a request: a call
    /// timed out or was cancelled, so it may still work on it.
    broken: AtomicBool,
}

/// Answer the requests of the agent. Sayso gives it no tools.
pub(crate) fn handler() -> Handler {
    Box::new(|method, params| match method {
        "session/request_permission" => Ok(refuse_permission(params)),
        other => Err(RpcError::new(METHOD_NOT_FOUND, format!("Method not found: {other}"))),
    })
}

/// Refuse a permission request: pick a reject option, else cancel.
fn refuse_permission(params: &Value) -> Value {
    let options = params["options"].as_array().map(Vec::as_slice).unwrap_or_default();
    let reject = ["reject_once", "reject_always"]
        .iter()
        .find_map(|kind| options.iter().find(|o| o["kind"] == *kind))
        .and_then(|o| o["optionId"].as_str());
    match reject {
        Some(id) => json!({ "outcome": { "outcome": "selected", "optionId": id } }),
        None => json!({ "outcome": { "outcome": "cancelled" } }),
    }
}

impl AcpClient {
    /// Send `initialize` and read the answer. `label` names the agent in
    /// messages until it gives its own name.
    pub fn initialize(conn: Connection, label: &str, deadline: Deadline) -> Result<AcpClient, EnhanceError> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false },
            "clientInfo": { "name": "sayso", "title": "Sayso", "version": env!("CARGO_PKG_VERSION") },
        });
        let result = conn.request("initialize", params, deadline.at).map_err(|e| call_error(e, label, "initialize", deadline))?;
        let info = parse_info(&result)?;
        if info.protocol_version != PROTOCOL_VERSION {
            return Err(EnhanceError::NotConfigured(format!(
                "{label} speaks version {} of the Agent Client Protocol, and Sayso speaks version {PROTOCOL_VERSION}",
                info.protocol_version
            )));
        }
        let label = info.name.clone().unwrap_or_else(|| label.to_string());
        Ok(AcpClient { conn, info, label, broken: AtomicBool::new(false) })
    }

    pub fn info(&self) -> &AgentInfo {
        &self.info
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    /// True when the connection closed: the agent stopped, or a write failed.
    pub fn is_closed(&self) -> bool {
        self.conn.is_closed()
    }

    /// True when the connection closed, or a call timed out or was cancelled.
    pub fn is_unusable(&self) -> bool {
        self.conn.is_closed() || self.broken.load(Ordering::SeqCst)
    }

    /// Close the connection: the agent reads the end of its input.
    pub fn close(&self) {
        self.conn.close();
    }

    fn call(&self, method: &str, params: Value, deadline: Deadline, cancel: Option<&AtomicBool>) -> Result<Value, EnhanceError> {
        self.conn.request_until(method, params, deadline.at, cancel).map_err(|e| {
            if matches!(e, CallError::Timeout | CallError::Cancelled) {
                self.broken.store(true, Ordering::SeqCst);
            }
            call_error(e, &self.label, method, deadline)
        })
    }

    /// Open a session in `cwd`, an empty folder, with no MCP servers.
    pub fn new_session(&self, cwd: &Path, deadline: Deadline) -> Result<Session, EnhanceError> {
        let params = json!({ "cwd": cwd.to_string_lossy(), "mcpServers": [] });
        let result = self.call("session/new", params, deadline, None)?;
        parse_session(&result, &self.label)
    }

    /// Close a session when the agent can, without waiting for the answer.
    pub fn close_session(&self, session: &Session) {
        if self.info.closes_sessions {
            self.conn.request_detached("session/close", json!({ "sessionId": session.id }));
        }
    }

    /// Choose the model of a session. Nothing is sent when it is the current model.
    pub fn set_model(&self, session: &mut Session, model: &str, deadline: Deadline) -> Result<(), EnhanceError> {
        if session.current_model.as_deref() == Some(model) {
            return Ok(());
        }
        let result = match &session.control {
            Some(ModelControl::SetModel) => {
                self.call("session/set_model", json!({ "sessionId": session.id, "modelId": model }), deadline, None)
            }
            Some(ModelControl::ConfigOption(config_id)) => self.call(
                "session/set_config_option",
                json!({ "sessionId": session.id, "configId": config_id, "value": model }),
                deadline,
                None,
            ),
            None => {
                return Err(EnhanceError::NotConfigured(format!(
                    "{} does not let Sayso choose a model; clear the model of the provider",
                    self.label
                )));
            }
        };
        result.map_err(|e| match e {
            EnhanceError::Cli(m) => EnhanceError::NotConfigured(format!("{} could not use the model \"{model}\": {m}", self.label)),
            other => other,
        })?;
        session.current_model = Some(model.to_string());
        Ok(())
    }

    /// Run one prompt turn and collect the text of the agent. When the
    /// deadline passes, Sayso sends `session/cancel` and gives `Timeout`;
    /// the agent is then unusable.
    pub fn prompt(&self, session_id: &str, text: &str, deadline: Deadline) -> Result<Turn, EnhanceError> {
        let updates = self.conn.subscribe(session_id);
        let params = json!({ "sessionId": session_id, "prompt": [{ "type": "text", "text": text }] });
        let result = self.call("session/prompt", params, deadline, None);
        if matches!(result, Err(EnhanceError::Timeout(_))) {
            let _ = self.conn.notify("session/cancel", json!({ "sessionId": session_id }));
        }
        let result = result?;
        // The reader handles lines in order, so every update before the answer is here.
        let mut out = String::new();
        for note in updates.drain() {
            let update = &note.params["update"];
            if note.method == "session/update" && update["sessionUpdate"] == "agent_message_chunk" && update["content"]["type"] == "text" {
                out.push_str(update["content"]["text"].as_str().unwrap_or_default());
            }
        }
        match result["stopReason"].as_str().unwrap_or_default() {
            "end_turn" => Ok(Turn { text: out }),
            "refusal" => Err(EnhanceError::Refused(if out.trim().is_empty() { format!("{} refused the prompt", self.label) } else { out })),
            reason @ ("max_tokens" | "max_turn_requests") => {
                Err(EnhanceError::InvalidOutput(format!("{} stopped before the end of its reply: {reason}", self.label)))
            }
            "cancelled" => Err(EnhanceError::Cli(format!("{} cancelled the turn", self.label))),
            other => Err(EnhanceError::InvalidOutput(format!("{} ended the turn with an unknown reason \"{other}\"", self.label))),
        }
    }

    /// Run the sign-in of an `agent` method. The wait stops at the deadline, or when `cancel` becomes true.
    pub fn authenticate(&self, method_id: &str, deadline: Deadline, cancel: &AtomicBool) -> Result<(), EnhanceError> {
        self.call("authenticate", json!({ "methodId": method_id }), deadline, Some(cancel)).map(|_| ())
    }
}

/// Turn a failed call into the error of the provider.
fn call_error(error: CallError, label: &str, method: &str, deadline: Deadline) -> EnhanceError {
    match error {
        CallError::Rpc(e) if e.code == AUTH_REQUIRED => EnhanceError::NotConfigured(format!("not signed in to {label}: {}", e.detail())),
        CallError::Rpc(e) if looks_like_login_problem(&e.detail()) => {
            EnhanceError::NotConfigured(format!("not signed in to {label}: {}", e.detail()))
        }
        CallError::Rpc(e) => EnhanceError::Cli(format!("{label} could not run {method}: {} ({})", e.detail(), e.code)),
        CallError::Timeout => EnhanceError::Timeout(deadline.timeout_ms()),
        CallError::Cancelled => EnhanceError::Cli("the sign-in was cancelled".into()),
        CallError::Closed(reason) => EnhanceError::Cli(format!("{label} stopped: {reason}")),
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value[key].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

fn parse_info(result: &Value) -> Result<AgentInfo, EnhanceError> {
    let version = result["protocolVersion"]
        .as_u64()
        .ok_or_else(|| EnhanceError::InvalidOutput("the initialize answer has no protocol version".into()))?;
    let agent = &result["agentInfo"];
    let auth_methods = result["authMethods"].as_array().map(|list| list.iter().filter_map(parse_auth_method).collect()).unwrap_or_default();
    let session_caps = &result["agentCapabilities"]["sessionCapabilities"];
    Ok(AgentInfo {
        name: text(agent, "title").or_else(|| text(agent, "name")),
        version: text(agent, "version"),
        protocol_version: version,
        auth_methods,
        closes_sessions: session_caps.get("close").is_some_and(|c| !c.is_null()),
    })
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().map(|list| list.iter().filter_map(|a| a.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn parse_auth_method(method: &Value) -> Option<AuthMethod> {
    let id = text(method, "id")?;
    let name = text(method, "name").unwrap_or_else(|| id.clone());
    // Agents from before terminal methods were in the protocol name the command in `_meta`.
    let legacy = &method["_meta"]["terminal-auth"];
    let kind = match method["type"].as_str() {
        None | Some("agent") if legacy.is_object() => AuthKind::Terminal { command: text(legacy, "command"), args: strings(&legacy["args"]) },
        None | Some("agent") => AuthKind::Agent,
        Some("terminal") => AuthKind::Terminal { command: None, args: strings(&method["args"]) },
        Some(other) => AuthKind::Other(other.to_string()),
    };
    Some(AuthMethod { id, name, kind })
}

/// The options of a select config option, flat or in groups: (value, name, description).
fn select_options(option: &Value) -> Vec<(String, String, Option<String>)> {
    let mut out = Vec::new();
    for item in option["options"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let items = match item["options"].as_array() {
            Some(group) => group.iter().collect::<Vec<_>>(),
            None => vec![item],
        };
        for entry in items {
            let Some(value) = text(entry, "value") else { continue };
            let name = text(entry, "name").unwrap_or_else(|| value.clone());
            out.push((value, name, text(entry, "description")));
        }
    }
    out
}

fn parse_session(result: &Value, label: &str) -> Result<Session, EnhanceError> {
    let id = text(result, "sessionId").ok_or_else(|| EnhanceError::InvalidOutput(format!("{label} opened a session without an id")))?;
    let options = result["configOptions"].as_array().map(Vec::as_slice).unwrap_or_default();
    let by_category = |category: &str| options.iter().find(|o| o["category"] == category && o["type"] == "select");
    // The levels are those of the current model: another model can have others.
    // "default" leaves the level to the agent, so it is no level.
    let thinking: Vec<String> = by_category("thought_level")
        .map(|o| sort_thinking(select_options(o).into_iter().map(|(value, _, _)| value).filter(|v| v != "default").collect()))
        .unwrap_or_default();
    // The config option comes first: it replaces `models`.
    let (models, current_model, control) = if let Some(option) = by_category("model") {
        let models = select_options(option).into_iter().map(|(id, name, description)| ModelChoice { id, name, description, thinking: Vec::new() }).collect();
        (models, text(option, "currentValue"), text(option, "id").map(ModelControl::ConfigOption))
    } else if let Some(list) = result["models"]["availableModels"].as_array() {
        let models = list
            .iter()
            .filter_map(|m| {
                let id = text(m, "modelId")?;
                Some(ModelChoice { name: text(m, "name").unwrap_or_else(|| id.clone()), id, description: text(m, "description"), thinking: Vec::new() })
            })
            .collect();
        (models, text(&result["models"], "currentModelId"), Some(ModelControl::SetModel))
    } else {
        (Vec::new(), None, None)
    };
    let mut models = dedup(models);
    if let Some(current) = models.iter_mut().find(|m| current_model.as_deref() == Some(m.id.as_str())) {
        current.thinking = thinking;
    }
    Ok(Session { id, models, current_model, control })
}

/// Keep the first choice of each id, in order.
fn dedup(choices: Vec<ModelChoice>) -> Vec<ModelChoice> {
    let mut seen = std::collections::HashSet::new();
    choices.into_iter().filter(|c| seen.insert(c.id.clone())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_permission_request_is_rejected_or_cancelled() {
        let params = json!({ "sessionId": "s", "toolCall": { "toolCallId": "c" }, "options": [
            { "optionId": "yes", "name": "Allow", "kind": "allow_once" },
            { "optionId": "never", "name": "Never", "kind": "reject_always" },
            { "optionId": "no", "name": "Reject", "kind": "reject_once" },
        ]});
        assert_eq!(refuse_permission(&params), json!({ "outcome": { "outcome": "selected", "optionId": "no" } }));
        let only_allow = json!({ "options": [{ "optionId": "yes", "name": "Allow", "kind": "allow_once" }] });
        assert_eq!(refuse_permission(&only_allow), json!({ "outcome": { "outcome": "cancelled" } }));
        assert_eq!(refuse_permission(&json!({})), json!({ "outcome": { "outcome": "cancelled" } }));
    }

    #[test]
    fn the_handler_refuses_files_and_terminals() {
        let handler = handler();
        for method in ["fs/read_text_file", "fs/write_text_file", "terminal/create", "_vendor/thing"] {
            assert_eq!(handler(method, &json!({})).unwrap_err().code, METHOD_NOT_FOUND, "{method}");
        }
    }

    #[test]
    fn the_initialize_answer_gives_the_name_and_the_sign_in_methods() {
        let result = json!({
            "protocolVersion": 1,
            "agentCapabilities": { "loadSession": true, "sessionCapabilities": { "close": {} } },
            "agentInfo": { "name": "my-agent", "title": "My Agent", "version": "1.2.0" },
            "authMethods": [
                { "id": "login", "name": "Log in", "description": null },
                { "id": "tty", "name": "Terminal", "type": "terminal", "args": ["--login"], "env": { "X": "1" } },
                { "id": "old", "name": "Old", "_meta": { "terminal-auth": { "command": "node", "args": ["cli.js", "/login"], "label": "Login" } } },
                { "id": "env", "name": "Key", "type": "env_var" },
                { "name": "no id" },
            ],
        });
        let info = parse_info(&result).unwrap();
        assert_eq!((info.name.as_deref(), info.version.as_deref(), info.protocol_version), (Some("My Agent"), Some("1.2.0"), 1));
        assert!(info.closes_sessions);
        let kinds: Vec<&AuthKind> = info.auth_methods.iter().map(|m| &m.kind).collect();
        assert_eq!(
            kinds,
            [
                &AuthKind::Agent,
                &AuthKind::Terminal { command: None, args: vec!["--login".into()] },
                &AuthKind::Terminal { command: Some("node".into()), args: vec!["cli.js".into(), "/login".into()] },
                &AuthKind::Other("env_var".into()),
            ]
        );
        let bare = parse_info(&json!({ "protocolVersion": 1 })).unwrap();
        assert_eq!((bare.name, bare.auth_methods.len(), bare.closes_sessions), (None, 0, false));
        assert!(matches!(parse_info(&json!({})), Err(EnhanceError::InvalidOutput(_))));
    }

    #[test]
    fn a_session_reads_the_model_config_option_with_groups_and_levels() {
        let result = json!({ "sessionId": "s1", "configOptions": [
            { "id": "mode", "name": "Mode", "category": "mode", "type": "select", "currentValue": "ask", "options": [{ "value": "ask", "name": "Ask" }] },
            { "id": "effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": "low",
              "options": [{ "value": "high", "name": "High" }, { "value": "low", "name": "Low" }, { "value": "default", "name": "Default" }] },
            { "id": "pick", "name": "Model", "category": "model", "type": "select", "currentValue": "fast", "options": [
                { "group": "rec", "name": "Recommended", "options": [{ "value": "fast", "name": "Fast", "description": "Quick" }] },
                { "group": "all", "name": "All", "options": [{ "value": "big", "name": "Big" }, { "value": "fast", "name": "Again" }] },
            ]},
        ]});
        let session = parse_session(&result, "A").unwrap();
        assert_eq!(session.id, "s1");
        assert_eq!(session.current_model.as_deref(), Some("fast"));
        assert_eq!(session.control, Some(ModelControl::ConfigOption("pick".into())));
        let ids: Vec<(&str, &str)> = session.models.iter().map(|m| (m.id.as_str(), m.name.as_str())).collect();
        assert_eq!(ids, [("fast", "Fast"), ("big", "Big")]);
        assert_eq!(session.models[0].description.as_deref(), Some("Quick"));
        assert_eq!(session.models[0].thinking, ["low", "high"], "the levels of the current model, without \"default\"");
        assert!(session.models[1].thinking.is_empty(), "another model may have other levels");
    }

    #[test]
    fn a_session_reads_the_older_model_list() {
        let result = json!({ "sessionId": "s2", "models": { "currentModelId": "b", "availableModels": [
            { "modelId": "a", "name": "Model A", "description": "First" },
            { "modelId": "b", "name": "Model B" },
            { "name": "no id" },
        ]}});
        let session = parse_session(&result, "A").unwrap();
        assert_eq!(session.control, Some(ModelControl::SetModel));
        assert_eq!(session.current_model.as_deref(), Some("b"));
        assert_eq!(session.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert!(session.models.iter().all(|m| m.thinking.is_empty()));

        let plain = parse_session(&json!({ "sessionId": "s3" }), "A").unwrap();
        assert_eq!((plain.models.len(), plain.current_model, plain.control), (0, None, None));
        assert!(matches!(parse_session(&json!({}), "A"), Err(EnhanceError::InvalidOutput(_))));
    }

    #[test]
    fn call_errors_map_to_the_errors_of_the_provider() {
        let deadline = Deadline::after(Duration::from_millis(1500));
        let auth = CallError::Rpc(RpcError::new(AUTH_REQUIRED, "Authentication required"));
        assert!(matches!(call_error(auth, "Agent", "session/new", deadline), EnhanceError::NotConfigured(m) if m.starts_with("not signed in to Agent")));
        let login = CallError::Rpc(RpcError::new(-32603, "Please run /login first"));
        assert!(matches!(call_error(login, "Agent", "session/prompt", deadline), EnhanceError::NotConfigured(m) if m.contains("not signed in")));
        let other = CallError::Rpc(RpcError::new(-32603, "boom"));
        assert!(matches!(call_error(other, "Agent", "session/new", deadline), EnhanceError::Cli(m) if m.contains("boom") && m.contains("session/new")));
        assert_eq!(call_error(CallError::Timeout, "Agent", "x", deadline), EnhanceError::Timeout(1500));
        assert!(matches!(call_error(CallError::Closed("gone".into()), "Agent", "x", deadline), EnhanceError::Cli(m) if m == "Agent stopped: gone"));
    }
}
