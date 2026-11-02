//! Claude Code provider: the user's own `claude` command, driven over the
//! stream-JSON protocol that the Claude Agent SDK uses.
//!
//! Sayso starts `claude` with `--input-format stream-json` and
//! `--await-initialize`, so the process waits for an `initialize` request.
//! That request carries the system prompt and the JSON schema of the answer,
//! as the Agent SDK sends them. The transcript then goes in as one user
//! message, and the `result` message carries `structured_output`. One process
//! serves one request. The text stays out of the process list.
//!
//! The process can start before the transcript exists, so Sayso keeps one
//! started process ready for each binary, model, and thinking level (a spare).
//! A request takes the spare and starts the next one. A spare that waits for
//! [`SPARE_LIFETIME`] is stopped. [`prepare`] starts one when a dictation starts.
//!
//! Measured on this Mac (tool 2.1.314, Haiku, `low`): the process answers
//! `initialize` about 0.2 s after it starts, and one sentence takes 0.7 to
//! 1.4 s after the transcript goes in, with two model turns, because the
//! schema is a tool call. Without the schema it takes one turn and 0.1 to
//! 0.25 s less, but then the shape of the answer is not certain.
//!
//! The thinking level of the provider sets `--effort`. The tool reports the
//! levels of each model with its model list (see `models`). With Haiku 5.5
//! (6 runs of two sentences for each level, tool 2.1.296) the model thought in
//! 0 runs at `low`, 2 at `medium`, and 5 at `high`, and a run with thinking
//! wrote 200 to 450 tokens more and took about 0.5 to 1 s longer.
//! `MAX_THINKING_TOKENS=0` changed nothing there: `medium` thought in 4 of 6
//! runs with it. Sayso sets it at `low` only, for a model that still reads it.
//! No setting stops all thinking: `low` thought in about 1 run of 10 with
//! other prompts. Without `--strict-mcp-config` the CLI loads the user's MCP
//! servers and plugins: one run read 160K tokens of tool definitions, took
//! three turns and 5.7 s, and answered in prose. The CLI uses the user's own
//! login. Sayso never reads Claude credentials, and it never passes any.

use crate::auth::{Tool, config_env, push_config_env};
use crate::cli::{self, Invocation, Session, looks_like_login_problem};
use crate::detect::detect_claude_cli;
use parking_lot::Mutex;
use sayso_core::config::{DEFAULT_CLI_THINKING, Provider, ProviderKind};
use sayso_core::enhance::{
    EnhanceError, EnhanceRequest, EnhanceResponse, Enhancer, ProviderReport, output_schema, parse_value, user_message,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long a spare process waits for a request before Sayso stops it.
pub const SPARE_LIFETIME: Duration = Duration::from_secs(600);

/// The request id of the `initialize` request. The answer carries it back.
const INIT_ID: &str = "sayso-init";

pub struct ClaudeCli {
    id: String,
    /// None when no binary was found. `enhance` then fails with `NotConfigured`.
    program: Option<PathBuf>,
    model: String,
    default_timeout: Duration,
    config_dir: Option<PathBuf>,
    /// A name from the `supportedEffortLevels` of the model.
    thinking: String,
    /// Keep a started process ready for the next request.
    spare: bool,
}

impl ClaudeCli {
    /// `path` is the binary from the provider config. None searches for it.
    pub fn new(id: impl Into<String>, path: Option<PathBuf>, model: impl Into<String>) -> Self {
        let program = path.or_else(detect_claude_cli);
        Self {
            id: id.into(),
            program,
            model: model.into(),
            default_timeout: Duration::from_secs(8),
            config_dir: None,
            thinking: DEFAULT_CLI_THINKING.into(),
            spare: true,
        }
    }

    /// The timeout used when a request has a zero timeout.
    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// The folder with the login of the tool (`CLAUDE_CONFIG_DIR`). A leading `~/` is the home folder.
    pub fn with_config_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.config_dir = dir;
        self
    }

    /// The thinking level. None keeps the default, `low`.
    pub fn with_thinking(mut self, level: Option<&str>) -> Self {
        self.thinking = level.unwrap_or(DEFAULT_CLI_THINKING).into();
        self
    }

    /// Whether a request leaves a started process ready for the next one. On by default.
    pub fn with_spare(mut self, spare: bool) -> Self {
        self.spare = spare;
        self
    }

    /// What a process for `model` is started with.
    fn launch(&self, model: &str) -> Option<Launch> {
        Some(Launch { program: self.program.clone()?, model: model.to_string(), thinking: self.thinking.clone(), config_dir: self.config_dir.clone() })
    }
}

/// What a process is started with. Requests with the same launch can use the same spare.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Launch {
    program: PathBuf,
    model: String,
    thinking: String,
    config_dir: Option<PathBuf>,
}

impl Launch {
    /// The command line arguments. The system prompt, the schema, and the transcript come later, on stdin.
    fn args(&self) -> Vec<std::ffi::OsString> {
        [
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--await-initialize",
            "--model",
            &self.model,
            "--effort",
            &self.thinking,
            "--tools",
            "",
            "--strict-mcp-config",
            "--no-session-persistence",
            "--disable-slash-commands",
            "--setting-sources",
            "",
        ]
        .iter()
        .map(Into::into)
        .collect()
    }

    /// Start the process. `timeout` is its first deadline.
    fn start(&self, timeout: Duration) -> Result<Started, EnhanceError> {
        let workdir = cli::workdir()?;
        let dir_env = config_env(Tool::Claude, self.config_dir.as_deref().and_then(|d| d.to_str()));
        // A limit of 0 at a higher level would work against the choice of the user.
        let mut envs = Vec::new();
        if self.thinking == DEFAULT_CLI_THINKING {
            envs.push(("MAX_THINKING_TOKENS", "0"));
        }
        push_config_env(&mut envs, &dir_env);
        let session = Session::start(&Invocation { program: &self.program, args: self.args(), envs, cwd: workdir.path(), stdin: None, timeout })?;
        Ok(Started { session, _workdir: workdir, at: Instant::now() })
    }
}

/// A started process with its empty folder, which must outlive it.
struct Started {
    session: Session,
    _workdir: tempfile::TempDir,
    at: Instant,
}

/// A spare and the number that tells it apart from a later spare.
struct Spare {
    started: Started,
    generation: u64,
}

static SPARES: LazyLock<Mutex<HashMap<Launch, Spare>>> = LazyLock::new(Mutex::default);
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Take the spare of a launch, when one is ready and still runs.
fn take_spare(launch: &Launch) -> Option<Started> {
    let mut spare = SPARES.lock().remove(launch)?.started;
    (spare.at.elapsed() < SPARE_LIFETIME && spare.session.is_running()).then_some(spare)
}

/// Start a spare for a launch in the background, unless one is there. It is
/// stopped when no request takes it within [`SPARE_LIFETIME`].
fn refill(launch: &Launch) {
    if SPARES.lock().contains_key(launch) {
        return;
    }
    let launch = launch.clone();
    std::thread::spawn(move || {
        let started = match launch.start(SPARE_LIFETIME + Duration::from_secs(60)) {
            Ok(started) => started,
            Err(e) => {
                log::debug!("could not start a spare claude process: {e}");
                return;
            }
        };
        let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
        {
            let mut spares = SPARES.lock();
            if spares.contains_key(&launch) {
                return;
            }
            spares.insert(launch.clone(), Spare { started, generation });
        }
        std::thread::sleep(SPARE_LIFETIME);
        let mut spares = SPARES.lock();
        if spares.get(&launch).is_some_and(|spare| spare.generation == generation) {
            spares.remove(&launch);
        }
    });
}

/// Start a process for a provider, so that the next request finds it ready.
/// Call it when a dictation starts. Does nothing for other kinds.
pub fn prepare(provider: &Provider) {
    if let ProviderKind::ClaudeCli { path, model, .. } = &provider.kind
        && let Some(launch) = ClaudeCli::new(&provider.id, path.as_ref().map(PathBuf::from), model)
            .with_thinking(provider.thinking_sent())
            .with_config_dir(provider.config_dir().map(PathBuf::from))
            .launch(model)
    {
        refill(&launch);
    }
}

/// Stop all spare processes. For tests, and when the app quits.
pub fn stop_spares() {
    SPARES.lock().clear();
}

/// The `initialize` request: the system prompt and the schema of the answer.
fn initialize_request(system_prompt: &str) -> Value {
    json!({
        "type": "control_request",
        "request_id": INIT_ID,
        "request": { "subtype": "initialize", "systemPrompt": [system_prompt], "jsonSchema": output_schema() },
    })
}

/// The user message with the transcript.
fn user_request(transcript: &str) -> Value {
    json!({
        "type": "user",
        "session_id": "",
        "message": { "role": "user", "content": [{ "type": "text", "text": user_message(transcript) }] },
        "parent_tool_use_id": null,
    })
}

/// The answer to `initialize`: None for other lines, an error when the tool refused it.
fn init_answer(line: &str) -> Option<Result<(), EnhanceError>> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value["type"] != "control_response" || value["response"]["request_id"] != INIT_ID {
        return None;
    }
    let response = &value["response"];
    if response["subtype"] == "error" {
        let text = response["error"].as_str().unwrap_or("no reason given");
        return Some(Err(if looks_like_login_problem(text) {
            EnhanceError::NotConfigured(format!("claude is not signed in: {text}"))
        } else {
            EnhanceError::Cli(format!("claude refused the request: {text}"))
        }));
    }
    Some(Ok(()))
}

/// The `result` line, or None for other lines.
fn result_line(line: &str) -> Option<String> {
    let value: Value = serde_json::from_str(line).ok()?;
    (value["type"] == "result").then(|| line.to_string())
}

/// Run one request on a started process.
fn run(session: &mut Session, request: &EnhanceRequest) -> Result<(String, ProviderReport), EnhanceError> {
    session.send(&initialize_request(&request.system_prompt).to_string())?;
    session.read_until(init_answer)??;
    session.send(&user_request(&request.transcript).to_string())?;
    let line = session.read_until(result_line)?;
    parse_envelope(&line)
}

/// A process that stopped early says why on its error output.
fn stopped_early(error: EnhanceError, session: &Session) -> EnhanceError {
    match error {
        EnhanceError::Cli(message) if message.contains("closed its output") => {
            let stderr = session.stderr_tail();
            if looks_like_login_problem(&stderr) {
                EnhanceError::NotConfigured(format!("claude is not signed in: {stderr}"))
            } else {
                EnhanceError::Cli(format!("claude stopped before it answered: {stderr}"))
            }
        }
        other => other,
    }
}

impl Enhancer for ClaudeCli {
    fn id(&self) -> &str {
        &self.id
    }

    /// The CLI sends text to Anthropic.
    fn is_cloud(&self) -> bool {
        true
    }

    fn enhance(&self, request: &EnhanceRequest) -> Result<EnhanceResponse, EnhanceError> {
        let timeout = if request.timeout.is_zero() { self.default_timeout } else { request.timeout };
        let model = request.model.clone().unwrap_or_else(|| self.model.clone());
        let launch = self.launch(&model).ok_or_else(|| EnhanceError::NotConfigured("the claude command was not found".into()))?;
        let started_at = Instant::now();
        let mut started = match self.spare.then(|| take_spare(&launch)).flatten() {
            Some(spare) => spare,
            None => launch.start(timeout)?,
        };
        started.session.restart_deadline(timeout.saturating_sub(started_at.elapsed()));
        if self.spare {
            refill(&launch);
        }
        let (text, report) = run(&mut started.session, request).map_err(|e| stopped_early(e, &started.session))?;
        Ok(EnhanceResponse {
            text,
            provider_id: self.id.clone(),
            model,
            elapsed_ms: started_at.elapsed().as_millis() as u64,
            mode: "claude_cli".into(),
            report: Some(report),
        })
    }
}

/// Read the `--output-format json` result and return the validated text, with
/// the numbers of the run.
///
/// Fails on `is_error`, on a `subtype` other than `success`, and on a missing
/// or invalid `structured_output`.
pub(crate) fn parse_envelope(stdout: &str) -> Result<(String, ProviderReport), EnhanceError> {
    let value: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| EnhanceError::InvalidOutput(format!("claude output is not JSON: {e}")))?;
    // Some versions print the whole message list. The result is the last "result" item.
    let envelope = match &value {
        serde_json::Value::Array(items) => items.iter().rev().find(|i| i["type"] == "result"),
        other => Some(other),
    }
    .ok_or_else(|| EnhanceError::InvalidOutput("claude output has no result message".into()))?;

    let message = envelope["result"].as_str().unwrap_or_default();
    if envelope["is_error"].as_bool().unwrap_or(false) {
        return Err(if looks_like_login_problem(message) {
            EnhanceError::NotConfigured(format!("claude is not signed in: {message}"))
        } else {
            EnhanceError::Cli(format!("claude reported an error: {message}"))
        });
    }
    if let Some(subtype) = envelope["subtype"].as_str().filter(|s| *s != "success") {
        return Err(EnhanceError::Cli(format!("claude ended with \"{subtype}\": {message}")));
    }
    let structured = envelope
        .get("structured_output")
        .filter(|v| !v.is_null())
        .ok_or_else(|| EnhanceError::InvalidOutput("claude returned no structured_output".into()))?;
    Ok((parse_value(structured)?, report(envelope)))
}

/// The numbers of the run in the result. A version of the tool that leaves
/// one out gives None for it.
fn report(envelope: &serde_json::Value) -> ProviderReport {
    let usage = &envelope["usage"];
    ProviderReport {
        run_ms: envelope["duration_ms"].as_u64(),
        api_ms: envelope["duration_api_ms"].as_u64(),
        turns: envelope["num_turns"].as_u64(),
        input_tokens: usage["input_tokens"].as_u64(),
        output_tokens: usage["output_tokens"].as_u64(),
        thinking_tokens: usage["output_tokens_details"]["thinking_tokens"].as_u64(),
        cache_read_tokens: usage["cache_read_input_tokens"].as_u64(),
        cache_write_tokens: usage["cache_creation_input_tokens"].as_u64(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_structured_output() {
        let out = r#"{"type":"result","subtype":"success","is_error":false,"result":"","structured_output":{"text":"Hello."}}"#;
        assert_eq!(parse_envelope(out).unwrap(), ("Hello.".to_string(), ProviderReport::default()));
    }

    #[test]
    fn reads_the_numbers_of_the_run() {
        let out = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":5210,"duration_api_ms":4870,"num_turns":3,
            "usage":{"input_tokens":12,"cache_creation_input_tokens":160000,"cache_read_input_tokens":0,"output_tokens":41,"output_tokens_details":{"thinking_tokens":30}},
            "structured_output":{"text":"Hello."}}"#;
        let (_, report) = parse_envelope(out).unwrap();
        assert_eq!((report.run_ms, report.api_ms, report.turns), (Some(5210), Some(4870), Some(3)));
        assert_eq!((report.input_tokens, report.output_tokens, report.thinking_tokens), (Some(12), Some(41), Some(30)));
        assert_eq!((report.cache_read_tokens, report.cache_write_tokens), (Some(0), Some(160_000)));
    }

    #[test]
    fn reads_the_result_item_of_a_message_list() {
        let out = r#"[{"type":"system"},{"type":"result","subtype":"success","structured_output":{"text":"Hi"}}]"#;
        assert_eq!(parse_envelope(out).unwrap().0, "Hi");
    }

    #[test]
    fn errors_map_to_the_right_variants() {
        let is_error = r#"{"type":"result","subtype":"success","is_error":true,"result":"boom"}"#;
        assert!(matches!(parse_envelope(is_error), Err(EnhanceError::Cli(m)) if m.contains("boom")));
        let login = r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in. Run /login"}"#;
        assert!(matches!(parse_envelope(login), Err(EnhanceError::NotConfigured(_))));
        let subtype = r#"{"type":"result","subtype":"error_max_turns","is_error":false}"#;
        assert!(matches!(parse_envelope(subtype), Err(EnhanceError::Cli(m)) if m.contains("error_max_turns")));
        let missing = r#"{"type":"result","subtype":"success","is_error":false,"result":"plain text"}"#;
        assert!(matches!(parse_envelope(missing), Err(EnhanceError::InvalidOutput(_))));
        let wrong = r#"{"type":"result","subtype":"success","structured_output":{"txt":"x"}}"#;
        assert!(matches!(parse_envelope(wrong), Err(EnhanceError::InvalidOutput(_))));
        assert!(matches!(parse_envelope("not json"), Err(EnhanceError::InvalidOutput(_))));
    }

    #[test]
    fn command_line_has_the_hygiene_flags() {
        let cli = ClaudeCli::new("c", Some("/bin/claude".into()), "haiku");
        let args: Vec<String> = cli.launch("haiku").unwrap().args().iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let has_pair = |a: &str, b: &str| args.windows(2).any(|w| w[0] == a && w[1] == b);
        assert!(has_pair("--model", "haiku"));
        assert!(has_pair("--effort", "low"));
        let high = cli.with_thinking(Some("high")).launch("haiku").unwrap().args();
        assert!(high.windows(2).any(|w| w[0] == "--effort" && w[1] == "high"));
        assert!(has_pair("--input-format", "stream-json"));
        assert!(has_pair("--output-format", "stream-json"));
        assert!(args.contains(&"--await-initialize".to_string()), "the process waits for the system prompt");
        assert!(has_pair("--tools", ""));
        assert!(has_pair("--setting-sources", ""));
        assert!(!args.iter().any(|a| a == "--system-prompt" || a == "-p"), "the prompt goes in on stdin");
        assert!(args.contains(&"--no-session-persistence".to_string()));
        assert!(args.contains(&"--strict-mcp-config".to_string()), "no MCP servers or plugins from the user's setup");
        assert!(args.contains(&"--disable-slash-commands".to_string()));
        assert!(!args.contains(&"--bare".to_string()), "--bare blocks the user's login");
    }

    #[test]
    fn the_requests_carry_the_prompt_the_schema_and_the_transcript() {
        let init = initialize_request("SYSTEM");
        assert_eq!(init["request"]["subtype"], "initialize");
        assert_eq!(init["request"]["systemPrompt"], json!(["SYSTEM"]));
        assert_eq!(init["request"]["jsonSchema"], output_schema());
        let user = user_request("me and him");
        assert_eq!(user["message"]["content"][0]["text"], "<transcript>\nme and him\n</transcript>");
    }

    #[test]
    fn the_init_answer_is_read_and_its_errors_are_mapped() {
        assert!(init_answer(r#"{"type":"system","subtype":"init"}"#).is_none());
        assert!(init_answer(r#"{"type":"control_response","response":{"request_id":"other","subtype":"success"}}"#).is_none());
        assert_eq!(init_answer(r#"{"type":"control_response","response":{"request_id":"sayso-init","subtype":"success"}}"#), Some(Ok(())));
        let login = init_answer(r#"{"type":"control_response","response":{"request_id":"sayso-init","subtype":"error","error":"Not logged in"}}"#);
        assert!(matches!(login, Some(Err(EnhanceError::NotConfigured(_)))));
        let other = init_answer(r#"{"type":"control_response","response":{"request_id":"sayso-init","subtype":"error","error":"bad"}}"#);
        assert!(matches!(other, Some(Err(EnhanceError::Cli(m))) if m.contains("bad")));
        assert!(result_line(r#"{"type":"assistant"}"#).is_none());
        assert!(result_line(r#"{"type":"result","subtype":"success"}"#).is_some());
    }
}
