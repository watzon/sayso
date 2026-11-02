//! The Claude CLI provider against fake `claude` scripts that speak the
//! stream-JSON protocol. No real tool runs and no model is called. The fake
//! tools are shell scripts, so this file runs on Unix only.
#![cfg(unix)]

mod common;

use common::fake_tool;
use sayso_core::enhance::{EnhanceError, EnhanceRequest, Enhancer, output_schema};
use sayso_enhance::ClaudeCli;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn request(timeout_ms: u64) -> EnhanceRequest {
    EnhanceRequest {
        system_prompt: "Fix the grammar.".into(),
        transcript: "me and him goes home".into(),
        model: None,
        temperature: None,
        timeout: Duration::from_millis(timeout_ms),
    }
}

const SUCCESS_ENVELOPE: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"","num_turns":2,"structured_output":{"text":"He and I go home."}}"#;
const INIT_OK: &str = r#"{"type":"control_response","response":{"request_id":"sayso-init","subtype":"success","response":{}}}"#;

/// A fake `claude` that records what it gets as `dir/seen.*`, answers
/// `initialize` with `init`, and answers the user message with `envelope`.
fn speaks(dir: &Path, init: &str, envelope: &str) -> String {
    let seen = dir.join("seen");
    format!(
        r#"echo $$ >> {seen}.pids
printf '%s\n' "$@" > {seen}.args
pwd > {seen}.cwd
echo "$MAX_THINKING_TOKENS" > {seen}.env
while IFS= read -r line; do
  printf '%s\n' "$line" >> {seen}.stdin
  case "$line" in
    *'"subtype":"initialize"'*)
      echo '{{"type":"system","subtype":"init"}}'
      cat <<'INIT'
{init}
INIT
      ;;
    *'"type":"user"'*)
      echo $$ > {seen}.answered
      cat <<'ENVELOPE'
{envelope}
ENVELOPE
      exit 0 ;;
  esac
done
"#,
        seen = seen.display()
    )
}

fn read(dir: &Path, suffix: &str) -> String {
    std::fs::read_to_string(dir.join(format!("seen.{suffix}"))).unwrap_or_default()
}

fn claude_with(script: &str, dir: &Path) -> ClaudeCli {
    ClaudeCli::new("claude", Some(fake_tool(dir, "claude", script)), "haiku").with_spare(false)
}

#[test]
fn claude_success_reads_structured_output() {
    let dir = tempfile::tempdir().unwrap();
    let script = speaks(dir.path(), INIT_OK, SUCCESS_ENVELOPE);
    let response = claude_with(&script, dir.path()).enhance(&request(20_000)).unwrap();
    assert_eq!(response.text, "He and I go home.");
    assert_eq!(
        (response.mode.as_str(), response.model.as_str(), response.provider_id.as_str()),
        ("claude_cli", "haiku", "claude")
    );
    assert_eq!(response.report.unwrap().turns, Some(2));

    let args: Vec<String> = read(dir.path(), "args").lines().map(String::from).collect();
    let value_after = |flag: &str| args.iter().position(|a| a == flag).map(|i| args[i + 1].clone());
    assert_eq!(value_after("--model").as_deref(), Some("haiku"));
    assert_eq!(value_after("--effort").as_deref(), Some("low"));
    assert_eq!(value_after("--input-format").as_deref(), Some("stream-json"));
    assert_eq!(value_after("--output-format").as_deref(), Some("stream-json"));
    assert_eq!(value_after("--tools").as_deref(), Some(""));
    assert_eq!(value_after("--setting-sources").as_deref(), Some(""));
    for flag in ["--await-initialize", "--no-session-persistence", "--disable-slash-commands", "--strict-mcp-config"] {
        assert!(args.contains(&flag.to_string()), "missing {flag}");
    }
    assert!(!args.iter().any(|a| a.contains("me and him") || a.contains("Fix the grammar")), "the text stays out of the arguments");

    let lines: Vec<serde_json::Value> = read(dir.path(), "stdin").lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2, "initialize, then the user message");
    assert_eq!(lines[0]["request"]["systemPrompt"][0], "Fix the grammar.");
    assert_eq!(lines[0]["request"]["jsonSchema"], output_schema());
    assert_eq!(lines[1]["message"]["content"][0]["text"], "<transcript>\nme and him goes home\n</transcript>");
    assert_eq!(read(dir.path(), "env").trim(), "0", "MAX_THINKING_TOKENS=0");

    // A higher thinking level goes to the tool, with no limit that works against it.
    claude_with(&script, dir.path()).with_thinking(Some("medium")).enhance(&request(20_000)).unwrap();
    let args: Vec<String> = read(dir.path(), "args").lines().map(String::from).collect();
    let effort = args.iter().position(|a| a == "--effort").unwrap();
    assert_eq!(args[effort + 1], "medium");
    assert_eq!(read(dir.path(), "env").trim(), "", "MAX_THINKING_TOKENS is set at a level above low");
    let cwd = read(dir.path(), "cwd");
    assert!(!Path::new(cwd.trim()).exists(), "the temporary folder is removed after the call");
    assert_ne!(Path::new(cwd.trim()), std::env::current_dir().unwrap());
}

#[test]
fn claude_model_override_wins() {
    let dir = tempfile::tempdir().unwrap();
    let script = speaks(dir.path(), INIT_OK, SUCCESS_ENVELOPE);
    let mut req = request(20_000);
    req.model = Some("sonnet".into());
    let response = claude_with(&script, dir.path()).enhance(&req).unwrap();
    assert_eq!(response.model, "sonnet");
    assert!(read(dir.path(), "args").lines().any(|l| l == "sonnet"));
}

#[test]
fn claude_error_envelope_fails() {
    let dir = tempfile::tempdir().unwrap();
    let script = speaks(dir.path(), INIT_OK, r#"{"type":"result","subtype":"success","is_error":true,"result":"overloaded"}"#);
    let error = claude_with(&script, dir.path()).enhance(&request(20_000)).unwrap_err();
    assert!(matches!(&error, EnhanceError::Cli(m) if m.contains("overloaded")), "{error:?}");
}

#[test]
fn claude_non_success_subtype_fails() {
    let dir = tempfile::tempdir().unwrap();
    let script = speaks(dir.path(), INIT_OK, r#"{"type":"result","subtype":"error_during_execution","is_error":false}"#);
    let error = claude_with(&script, dir.path()).enhance(&request(20_000)).unwrap_err();
    assert!(matches!(&error, EnhanceError::Cli(m) if m.contains("error_during_execution")), "{error:?}");
}

#[test]
fn a_refused_initialize_about_the_login_is_not_configured() {
    let dir = tempfile::tempdir().unwrap();
    let init = r#"{"type":"control_response","response":{"request_id":"sayso-init","subtype":"error","error":"Not logged in · Please run /login"}}"#;
    let script = speaks(dir.path(), init, SUCCESS_ENVELOPE);
    let error = claude_with(&script, dir.path()).enhance(&request(20_000)).unwrap_err();
    assert!(matches!(&error, EnhanceError::NotConfigured(m) if m.contains("not signed in")), "{error:?}");
}

#[test]
fn claude_crash_without_envelope_reports_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let script = "read line\necho 'segmentation trouble' >&2\nexit 3";
    let error = claude_with(script, dir.path()).enhance(&request(20_000)).unwrap_err();
    assert!(matches!(&error, EnhanceError::Cli(m) if m.contains("segmentation trouble")), "{error:?}");
}

#[test]
fn claude_invalid_structured_output_is_invalid_output() {
    let dir = tempfile::tempdir().unwrap();
    let script = speaks(dir.path(), INIT_OK, r#"{"type":"result","subtype":"success","is_error":false,"structured_output":{"text":""}}"#);
    let error = claude_with(&script, dir.path()).enhance(&request(20_000)).unwrap_err();
    assert!(matches!(error, EnhanceError::InvalidOutput(_)), "{error:?}");
}

fn process_is_alive(pid: &str) -> bool {
    Command::new("kill").args(["-0", pid]).output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn claude_timeout_kills_the_child() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let script = format!("echo $$ > {}\nexec sleep 30", pid_file.display());
    let started = Instant::now();
    let error = claude_with(&script, dir.path()).enhance(&request(400)).unwrap_err();
    assert!(matches!(error, EnhanceError::Timeout(_)), "{error:?}");
    assert!(started.elapsed() < Duration::from_secs(5), "took {:?}", started.elapsed());
    let pid = std::fs::read_to_string(&pid_file).unwrap();
    assert!(!process_is_alive(pid.trim()), "the child process {pid} is gone");
}

#[test]
fn claude_missing_binary_is_not_configured() {
    let claude = ClaudeCli::new("claude", Some("/nonexistent/claude".into()), "haiku");
    assert!(matches!(claude.enhance(&request(1_000)), Err(EnhanceError::NotConfigured(_))));
}

#[test]
fn claude_is_cloud() {
    assert!(ClaudeCli::new("c", Some("/x".into()), "haiku").is_cloud());
}

#[test]
fn a_helper_that_keeps_the_pipe_open_does_not_hang_the_call() {
    // The shell answers, then `sleep` keeps stdout open for a while.
    let dir = tempfile::tempdir().unwrap();
    let script = format!("sleep 20 &\n{}", speaks(dir.path(), INIT_OK, SUCCESS_ENVELOPE));
    let started = Instant::now();
    let response = claude_with(&script, dir.path()).enhance(&request(20_000)).unwrap();
    assert_eq!(response.text, "He and I go home.");
    assert!(started.elapsed() < Duration::from_secs(4), "took {:?}", started.elapsed());
}

/// Wait until `seen.pids` has `count` lines.
fn wait_for_pids(dir: &Path, count: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let pids: Vec<String> = read(dir, "pids").lines().map(String::from).collect();
        if pids.len() >= count || Instant::now() > deadline {
            return pids;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_spare_answers_the_next_request_and_a_new_one_is_started() {
    let dir = tempfile::tempdir().unwrap();
    let tool = fake_tool(dir.path(), "claude", &speaks(dir.path(), INIT_OK, SUCCESS_ENVELOPE));
    let claude = ClaudeCli::new("claude", Some(tool), "haiku");
    claude.enhance(&request(20_000)).unwrap();
    let first = read(dir.path(), "answered").trim().to_string();
    // The first request started a spare, the process that did not answer it.
    let pids = wait_for_pids(dir.path(), 2);
    assert_eq!(pids.len(), 2, "{pids:?}");
    let spare = pids.iter().find(|pid| **pid != first).unwrap().clone();
    claude.enhance(&request(20_000)).unwrap();
    assert_eq!(read(dir.path(), "answered").trim(), spare, "the spare answered");
    let pids = wait_for_pids(dir.path(), 3);
    assert_eq!(pids.len(), 3, "a new spare started: {pids:?}");
    sayso_enhance::stop_spares();
    std::thread::sleep(Duration::from_millis(200));
    assert!(!process_is_alive(&pids[2]), "stopping the spares ends their processes");
}
