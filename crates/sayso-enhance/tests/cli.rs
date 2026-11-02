//! The Claude CLI provider against fake `claude` scripts.
//! No real tool runs and no model is called. The fake tools are shell
//! scripts, so this file runs on Unix only.
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

const SUCCESS_ENVELOPE: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"","structured_output":{"text":"He and I go home."}}"#;

/// Script text that prints a canned envelope after it reads its input.
fn prints(envelope: &str) -> String {
    format!("cat >/dev/null\ncat <<'ENVELOPE'\n{envelope}\nENVELOPE\n")
}

/// Script text that records its arguments, environment, folder, and input as `dir/seen.*`.
fn recorder(dir: &Path) -> String {
    let seen = dir.join("seen");
    format!(
        "printf '%s\\n' \"$@\" > {seen}.args\npwd > {seen}.cwd\necho \"$MAX_THINKING_TOKENS\" > {seen}.env\ncat > {seen}.stdin\n",
        seen = seen.display()
    )
}

fn read(dir: &Path, suffix: &str) -> String {
    std::fs::read_to_string(dir.join(format!("seen.{suffix}"))).unwrap()
}

fn claude_with(script: &str, dir: &Path) -> ClaudeCli {
    ClaudeCli::new("claude", Some(fake_tool(dir, "claude", script)), "haiku")
}

#[test]
fn claude_success_reads_structured_output() {
    let dir = tempfile::tempdir().unwrap();
    let script = format!("{}cat <<'ENVELOPE'\n{SUCCESS_ENVELOPE}\nENVELOPE\n", recorder(dir.path()));
    let response = claude_with(&script, dir.path()).enhance(&request(20_000)).unwrap();
    assert_eq!(response.text, "He and I go home.");
    assert_eq!(
        (response.mode.as_str(), response.model.as_str(), response.provider_id.as_str()),
        ("claude_cli", "haiku", "claude")
    );

    let args: Vec<String> = read(dir.path(), "args").lines().map(String::from).collect();
    assert_eq!(args[0], "-p");
    let value_after = |flag: &str| args.iter().position(|a| a == flag).map(|i| args[i + 1].clone());
    assert_eq!(value_after("--model").as_deref(), Some("haiku"));
    assert_eq!(value_after("--effort").as_deref(), Some("low"));
    assert_eq!(value_after("--output-format").as_deref(), Some("json"));
    assert_eq!(value_after("--system-prompt").as_deref(), Some("Fix the grammar."));
    assert_eq!(value_after("--json-schema").unwrap(), output_schema().to_string());
    assert_eq!(value_after("--tools").as_deref(), Some(""));
    assert_eq!(value_after("--setting-sources").as_deref(), Some(""));
    assert!(args.contains(&"--no-session-persistence".to_string()));
    assert!(args.contains(&"--disable-slash-commands".to_string()));
    assert!(args.contains(&"--strict-mcp-config".to_string()));
    assert!(!args.iter().any(|a| a.contains("me and him")), "the transcript stays out of the arguments");

    assert_eq!(read(dir.path(), "env").trim(), "0", "MAX_THINKING_TOKENS=0");
    assert_eq!(read(dir.path(), "stdin"), "<transcript>\nme and him goes home\n</transcript>", "the transcript goes in tags on stdin");

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
    let script = format!("{}cat <<'ENVELOPE'\n{SUCCESS_ENVELOPE}\nENVELOPE\n", recorder(dir.path()));
    let mut req = request(20_000);
    req.model = Some("sonnet".into());
    let response = claude_with(&script, dir.path()).enhance(&req).unwrap();
    assert_eq!(response.model, "sonnet");
    assert!(read(dir.path(), "args").lines().any(|l| l == "sonnet"));
}

#[test]
fn claude_error_envelope_fails() {
    let dir = tempfile::tempdir().unwrap();
    let script = format!(
        "{}exit 1\n",
        prints(r#"{"type":"result","subtype":"success","is_error":true,"result":"overloaded"}"#)
    );
    let error = claude_with(&script, dir.path()).enhance(&request(20_000)).unwrap_err();
    assert!(matches!(&error, EnhanceError::Cli(m) if m.contains("overloaded")), "{error:?}");
}

#[test]
fn claude_non_success_subtype_fails() {
    let dir = tempfile::tempdir().unwrap();
    let script = prints(r#"{"type":"result","subtype":"error_during_execution","is_error":false}"#);
    let error = claude_with(&script, dir.path()).enhance(&request(20_000)).unwrap_err();
    assert!(matches!(&error, EnhanceError::Cli(m) if m.contains("error_during_execution")), "{error:?}");
}

#[test]
fn claude_crash_without_envelope_reports_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let script = "cat >/dev/null\necho 'segmentation trouble' >&2\nexit 3";
    let error = claude_with(script, dir.path()).enhance(&request(20_000)).unwrap_err();
    assert!(matches!(&error, EnhanceError::Cli(m) if m.contains("segmentation trouble")), "{error:?}");
}

#[test]
fn claude_invalid_structured_output_is_invalid_output() {
    let dir = tempfile::tempdir().unwrap();
    let script = prints(r#"{"type":"result","subtype":"success","is_error":false,"structured_output":{"text":""}}"#);
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
    assert_eq!(error, EnhanceError::Timeout(400));
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
    // The shell exits at once, but `sleep` keeps stdout open for a while.
    let dir = tempfile::tempdir().unwrap();
    let script = format!("{}sleep 20 &\n", prints(SUCCESS_ENVELOPE));
    let started = Instant::now();
    let response = claude_with(&script, dir.path()).enhance(&request(20_000)).unwrap();
    assert_eq!(response.text, "He and I go home.");
    assert!(started.elapsed() < Duration::from_secs(4), "took {:?}", started.elapsed());
}
