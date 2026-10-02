//! Claude Code CLI provider: `claude -p` with a JSON schema.
//!
//! Measured on this Mac (research §9): 1.6 to 1.9 s for one sentence with
//! `--effort low` and `MAX_THINKING_TOKENS=0`. Without `--strict-mcp-config`
//! the CLI loads the user's MCP servers and plugins: one run read 160K tokens
//! of tool definitions, took three turns and 5.7 s, and answered in prose. The CLI uses the user's own
//! login. Sayso never reads Claude credentials, and it never passes any.
//!
//! The transcript goes in through standard input. `claude -p` reads its prompt
//! from stdin when no prompt argument exists. This also keeps the text out of
//! the process list, and `--tools` takes a list, so a trailing prompt argument
//! could be read as a tool name.

use crate::cli::{self, Invocation, looks_like_login_problem, tail};
use crate::detect::detect_claude_cli;
use sayso_core::enhance::{EnhanceError, EnhanceRequest, EnhanceResponse, Enhancer, output_schema, parse_value, user_message};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct ClaudeCli {
    id: String,
    /// None when no binary was found. `enhance` then fails with `NotConfigured`.
    program: Option<PathBuf>,
    model: String,
    default_timeout: Duration,
}

impl ClaudeCli {
    /// `path` is the binary from the provider config. None searches for it.
    pub fn new(id: impl Into<String>, path: Option<PathBuf>, model: impl Into<String>) -> Self {
        let program = path.or_else(detect_claude_cli);
        Self { id: id.into(), program, model: model.into(), default_timeout: Duration::from_secs(8) }
    }

    /// The timeout used when a request has a zero timeout.
    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// The command line arguments, without the transcript.
    fn args(&self, model: &str, system_prompt: &str) -> Vec<std::ffi::OsString> {
        let schema = output_schema().to_string();
        [
            "-p",
            "--model",
            model,
            "--effort",
            "low",
            "--output-format",
            "json",
            "--json-schema",
            &schema,
            "--tools",
            "",
            "--strict-mcp-config",
            "--no-session-persistence",
            "--disable-slash-commands",
            "--setting-sources",
            "",
            "--system-prompt",
            system_prompt,
        ]
        .iter()
        .map(Into::into)
        .collect()
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
        let program = self
            .program
            .as_deref()
            .ok_or_else(|| EnhanceError::NotConfigured("the claude command was not found".into()))?;
        let timeout = if request.timeout.is_zero() { self.default_timeout } else { request.timeout };
        let model = request.model.clone().unwrap_or_else(|| self.model.clone());
        let workdir = tempfile::tempdir().map_err(|e| EnhanceError::Cli(format!("no temporary folder: {e}")))?;
        let started = Instant::now();
        let output = cli::run(&Invocation {
            program,
            args: self.args(&model, &request.system_prompt),
            envs: vec![("MAX_THINKING_TOKENS", "0")],
            cwd: workdir.path(),
            stdin: Some(user_message(&request.transcript)),
            timeout,
        })?;
        let text = parse_envelope(&output.stdout).map_err(|e| match e {
            // No usable envelope: the tool failed before it could write one.
            EnhanceError::InvalidOutput(_) if !output.status.success() => {
                EnhanceError::Cli(format!("claude exited with {}: {}", output.status, tail(&output.stderr)))
            }
            other => other,
        })?;
        Ok(EnhanceResponse {
            text,
            provider_id: self.id.clone(),
            model,
            elapsed_ms: started.elapsed().as_millis() as u64,
            mode: "claude_cli".into(),
        })
    }
}

/// Read the `--output-format json` result and return the validated text.
///
/// Fails on `is_error`, on a `subtype` other than `success`, and on a missing
/// or invalid `structured_output`.
pub(crate) fn parse_envelope(stdout: &str) -> Result<String, EnhanceError> {
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
    parse_value(structured)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_structured_output() {
        let out = r#"{"type":"result","subtype":"success","is_error":false,"result":"","structured_output":{"text":"Hello."}}"#;
        assert_eq!(parse_envelope(out).unwrap(), "Hello.");
    }

    #[test]
    fn reads_the_result_item_of_a_message_list() {
        let out = r#"[{"type":"system"},{"type":"result","subtype":"success","structured_output":{"text":"Hi"}}]"#;
        assert_eq!(parse_envelope(out).unwrap(), "Hi");
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
        let args: Vec<String> = cli.args("haiku", "SYSTEM").iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let has_pair = |a: &str, b: &str| args.windows(2).any(|w| w[0] == a && w[1] == b);
        assert!(has_pair("--model", "haiku"));
        assert!(has_pair("--effort", "low"));
        assert!(has_pair("--output-format", "json"));
        assert!(has_pair("--tools", ""));
        assert!(has_pair("--setting-sources", ""));
        assert!(has_pair("--system-prompt", "SYSTEM"));
        assert!(args.contains(&"--no-session-persistence".to_string()));
        assert!(args.contains(&"--strict-mcp-config".to_string()), "no MCP servers or plugins from the user's setup");
        assert!(args.contains(&"--disable-slash-commands".to_string()));
        assert!(!args.contains(&"--bare".to_string()), "--bare blocks the user's login");
    }
}
