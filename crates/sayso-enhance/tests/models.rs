//! `list_provider_models` against a mock server and fake `claude` and `codex` scripts.
//! No real tool runs and no model is called.

mod common;

use common::{MockServer, Reply, fake_tool};
use sayso_core::config::{Provider, ProviderKind};
use sayso_core::enhance::EnhanceError;
use sayso_enhance::{MemoryStore, SecretStore, list_provider_models};
use serde_json::json;
use std::path::Path;
use std::time::{Duration, Instant};

fn provider(kind: ProviderKind) -> Provider {
    Provider {
        id: "p".into(),
        name: "P".into(),
        kind,
    }
}

fn claude(path: &Path) -> Provider {
    provider(ProviderKind::ClaudeCli {
        path: Some(path.display().to_string()),
        model: "haiku".into(),
    })
}

fn codex(path: &Path) -> Provider {
    provider(ProviderKind::CodexCli {
        path: Some(path.display().to_string()),
        model: None,
    })
}

fn list(
    p: &Provider,
    timeout: Duration,
) -> Result<Vec<sayso_core::enhance::ModelChoice>, EnhanceError> {
    list_provider_models(p, &MemoryStore::new(), timeout)
}

fn ids(choices: &[sayso_core::enhance::ModelChoice]) -> Vec<&str> {
    choices.iter().map(|c| c.id.as_str()).collect()
}

const CLAUDE_REPLY: &str = r#"{"type":"control_response","response":{"subtype":"success","request_id":"sayso-models","response":{"models":[{"value":"default","displayName":"Default"},{"value":"opus","displayName":"Opus 5.5","description":"Complex work"},{"value":"haiku","displayName":"Haiku 4.5"}]}}}"#;

#[test]
fn claude_models_are_read_and_the_child_is_killed() {
    let dir = tempfile::tempdir().unwrap();
    let seen = dir.path().join("seen");
    // The script prints a noise line first, answers, then sleeps. `exec` makes the
    // sleep the child itself, so one kill ends it.
    let script = format!(
        "printf '%s\\n' \"$@\" > {seen}.args\necho \"$MAX_THINKING_TOKENS\" > {seen}.env\nIFS= read -r line\necho \"$line\" > {seen}.line\n\
         echo '{{\"type\":\"system\",\"subtype\":\"init\"}}'\ncat <<'REPLY'\n{CLAUDE_REPLY}\nREPLY\nexec sleep 30\n",
        seen = seen.display()
    );
    let path = fake_tool(dir.path(), "claude", &script);

    let started = Instant::now();
    let choices = list(&claude(&path), Duration::from_secs(10)).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "waited for the child to exit"
    );
    assert_eq!(ids(&choices), ["opus", "haiku"]);
    assert_eq!(choices[0].name, "Opus 5.5");
    assert_eq!(choices[0].description.as_deref(), Some("Complex work"));

    let read =
        |suffix: &str| std::fs::read_to_string(dir.path().join(format!("seen.{suffix}"))).unwrap();
    let args = read("args");
    for flag in [
        "-p",
        "--input-format",
        "--verbose",
        "--strict-mcp-config",
        "--no-session-persistence",
    ] {
        assert!(args.lines().any(|a| a == flag), "missing {flag}");
    }
    assert_eq!(read("env").trim(), "0");
    let sent: serde_json::Value = serde_json::from_str(read("line").trim()).unwrap();
    assert_eq!(
        sent,
        json!({"type":"control_request","request_id":"sayso-models","request":{"subtype":"initialize"}})
    );
}

#[test]
fn claude_that_never_answers_times_out_quickly() {
    let dir = tempfile::tempdir().unwrap();
    let path = fake_tool(dir.path(), "claude", "exec sleep 30");
    let started = Instant::now();
    let result = list(&claude(&path), Duration::from_millis(400));
    let elapsed = started.elapsed();
    assert!(
        matches!(result, Err(EnhanceError::Timeout(400))),
        "{result:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(400) && elapsed < Duration::from_millis(1400),
        "{elapsed:?}"
    );
}

#[test]
fn claude_login_error_is_not_configured() {
    let dir = tempfile::tempdir().unwrap();
    let reply = r#"{"type":"control_response","response":{"subtype":"error","request_id":"sayso-models","error":"Not logged in. Run /login"}}"#;
    let path = fake_tool(
        dir.path(),
        "claude",
        &format!("IFS= read -r line\ncat <<'REPLY'\n{reply}\nREPLY\nexec sleep 30\n"),
    );
    assert!(matches!(
        list(&claude(&path), Duration::from_secs(10)),
        Err(EnhanceError::NotConfigured(_))
    ));
}

#[test]
fn claude_that_exits_without_answering_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = fake_tool(dir.path(), "claude", "exit 1");
    let started = Instant::now();
    assert!(matches!(
        list(&claude(&path), Duration::from_secs(10)),
        Err(EnhanceError::Cli(_))
    ));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn missing_binaries_are_not_configured() {
    let result = list(
        &claude(Path::new("/nowhere/claude")),
        Duration::from_secs(1),
    );
    assert!(
        matches!(result, Err(EnhanceError::NotConfigured(_))),
        "{result:?}"
    );
    let result = list(&codex(Path::new("/nowhere/codex")), Duration::from_secs(1));
    assert!(
        matches!(result, Err(EnhanceError::NotConfigured(_))),
        "{result:?}"
    );
}

#[test]
fn codex_models_are_read_across_two_pages() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("lines");
    let script = format!(
        r#"while IFS= read -r line; do
  echo "$line" >> {log}
  case "$line" in
    *'"method":"initialize"'*)
      echo '{{"jsonrpc":"2.0","method":"remote/notice","params":{{}}}}'
      echo '{{"jsonrpc":"2.0","id":1,"result":{{"userAgent":"fake"}}}}' ;;
    *'"method":"model/list"'*cursor*)
      echo '{{"jsonrpc":"2.0","id":99,"result":{{"data":[]}}}}'
      echo '{{"jsonrpc":"2.0","id":3,"result":{{"data":[{{"id":"m3","model":"m3","displayName":"M3"}},{{"id":"m1","model":"m1","displayName":"Again"}}],"nextCursor":null}}}}' ;;
    *'"method":"model/list"'*)
      echo '{{"jsonrpc":"2.0","id":2,"result":{{"data":[{{"id":"m1","model":"m1","displayName":"M1","description":"First."}},{{"id":"mh","model":"mh","hidden":true}}],"nextCursor":"c2"}}}}' ;;
  esac
done
"#,
        log = log.display()
    );
    let path = fake_tool(dir.path(), "codex", &script);
    let choices = list(&codex(&path), Duration::from_secs(10)).unwrap();
    assert_eq!(ids(&choices), ["m1", "m3"]);
    assert_eq!(
        (choices[0].name.as_str(), choices[0].description.as_deref()),
        ("M1", Some("First."))
    );
    assert_eq!(choices[1].name, "M3");

    let sent = std::fs::read_to_string(&log).unwrap();
    assert!(
        sent.contains(r#""method":"initialized""#),
        "initialized notification missing"
    );
    assert!(
        sent.contains(r#""cursor":"c2""#),
        "second page was not requested with the cursor"
    );
    assert!(sent.contains(r#""name":"sayso""#));
}

#[test]
fn codex_error_reply_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let script = r#"while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) echo '{"jsonrpc":"2.0","id":1,"result":{}}' ;;
    *'"method":"model/list"'*) echo '{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"you must log in"}}' ;;
  esac
done"#;
    let path = fake_tool(dir.path(), "codex", script);
    assert!(matches!(
        list(&codex(&path), Duration::from_secs(10)),
        Err(EnhanceError::NotConfigured(_))
    ));
}

#[test]
fn codex_that_never_answers_times_out_quickly() {
    let dir = tempfile::tempdir().unwrap();
    let path = fake_tool(dir.path(), "codex", "exec sleep 30");
    let started = Instant::now();
    let result = list(&codex(&path), Duration::from_millis(400));
    assert!(
        matches!(result, Err(EnhanceError::Timeout(_))),
        "{result:?}"
    );
    assert!(started.elapsed() < Duration::from_millis(1400));
}

fn http_provider(server: &MockServer, account: Option<&str>) -> Provider {
    provider(ProviderKind::OpenAiCompatible {
        base_url: server.base_url.clone(),
        model: "unused".into(),
        api_key_account: account.map(String::from),
        zero_data_retention: false,
    })
}

#[test]
fn openai_models_use_names_sort_by_id_and_send_the_key() {
    let server = MockServer::start(|_, _| {
        Reply::ok(json!({"data":[{"id":"z/last","name":"Last"},{"id":"a/first"},{"id":"z/last"}]}))
    });
    let secrets = MemoryStore::new();
    secrets.set("acct", "sk-models").unwrap();
    let choices = list_provider_models(
        &http_provider(&server, Some("acct")),
        &secrets,
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(ids(&choices), ["a/first", "z/last"]);
    assert_eq!(
        (choices[0].name.as_str(), choices[1].name.as_str()),
        ("a/first", "Last")
    );
    let requests = server.requests();
    assert_eq!(
        (requests[0].method.as_str(), requests[0].path.as_str()),
        ("GET", "/v1/models")
    );
    assert_eq!(
        requests[0].headers.get("authorization").map(String::as_str),
        Some("Bearer sk-models")
    );
}

#[test]
fn openai_models_respect_the_timeout() {
    let server =
        MockServer::start(|_, _| Reply::ok(json!({"data":[]})).after(Duration::from_secs(3)));
    let started = Instant::now();
    let result = list(&http_provider(&server, None), Duration::from_millis(300));
    assert!(
        matches!(result, Err(EnhanceError::Timeout(300))),
        "{result:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}
