//! List the models a provider offers, for the model picker.
//!
//! - OpenAI-compatible servers: `GET {base_url}/models`.
//! - Claude Code has no "list models" command, but its stream-JSON control
//!   protocol answers an `initialize` request with the model list. No prompt
//!   is sent, so this costs nothing.
//! - Codex: the `codex app-server` JSON-RPC `model/list` method.
//!
//! All calls block: run them on a background thread.

use crate::cli::{Invocation, Session, looks_like_login_problem};
use crate::detect::{detect_claude_cli, detect_codex_cli, fetch_model_choices};
use crate::secrets::SecretStore;
use sayso_core::config::{Provider, ProviderKind};
use sayso_core::enhance::{EnhanceError, ModelChoice};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;

/// The request id of the Claude `initialize` request. The answer carries it back.
const CLAUDE_REQUEST_ID: &str = "sayso-models";

/// The most pages of `model/list` that Sayso reads from Codex.
const CODEX_MAX_PAGES: u64 = 5;

/// The models `provider` offers. `timeout` bounds the whole call.
///
/// HTTP lists are sorted by id. CLI lists keep the order of the tool. There
/// are never two choices with the same id. Claude's "default" entry is left
/// out: it means whatever Claude Code defaults to, which is usually its
/// slowest model.
pub fn list_provider_models(
    provider: &Provider,
    secrets: &dyn SecretStore,
    timeout: Duration,
) -> Result<Vec<ModelChoice>, EnhanceError> {
    match &provider.kind {
        ProviderKind::OpenAiCompatible {
            base_url,
            api_key_account,
            ..
        } => {
            // A missing key is not an error here: some servers list models without one.
            let key = api_key_account
                .as_deref()
                .and_then(|account| secrets.get(account));
            fetch_model_choices(base_url, key.as_deref(), timeout)
        }
        ProviderKind::ClaudeCli { path, .. } => {
            let program = program(path.as_deref(), detect_claude_cli, "claude")?;
            list_claude_models(&program, timeout)
        }
        ProviderKind::CodexCli { path, .. } => {
            let program = program(path.as_deref(), detect_codex_cli, "codex")?;
            list_codex_models(&program, timeout)
        }
    }
}

/// The configured binary, else the detected one.
fn program(
    path: Option<&str>,
    detect: fn() -> Option<PathBuf>,
    name: &str,
) -> Result<PathBuf, EnhanceError> {
    path.map(PathBuf::from)
        .or_else(detect)
        .ok_or_else(|| EnhanceError::NotConfigured(format!("the {name} command was not found")))
}

fn workdir() -> Result<tempfile::TempDir, EnhanceError> {
    tempfile::tempdir().map_err(|e| EnhanceError::Cli(format!("no temporary folder: {e}")))
}

fn list_claude_models(
    program: &std::path::Path,
    timeout: Duration,
) -> Result<Vec<ModelChoice>, EnhanceError> {
    let workdir = workdir()?;
    let mut session = Session::start(&Invocation {
        program,
        args: [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--tools",
            "",
            "--strict-mcp-config",
            "--setting-sources",
            "",
            "--no-session-persistence",
            "--disable-slash-commands",
        ]
        .iter()
        .map(Into::into)
        .collect(),
        envs: vec![("MAX_THINKING_TOKENS", "0")],
        cwd: workdir.path(),
        stdin: None,
        timeout,
    })?;
    let request = json!({
        "type": "control_request",
        "request_id": CLAUDE_REQUEST_ID,
        "request": { "subtype": "initialize" },
    });
    // The input stays open until the session drops: Claude Code exits when it closes.
    session.send(&request.to_string())?;
    session.read_until(claude_answer)?
}

/// The answer to our `initialize` request, or None for any other line.
fn claude_answer(line: &str) -> Option<Result<Vec<ModelChoice>, EnhanceError>> {
    let value: Value = serde_json::from_str(line).ok()?;
    (value["type"] == "control_response" && value["response"]["request_id"] == CLAUDE_REQUEST_ID)
        .then(|| parse_claude_models(&value))
}

/// Read a `control_response` message into model choices.
fn parse_claude_models(message: &Value) -> Result<Vec<ModelChoice>, EnhanceError> {
    let response = &message["response"];
    if response["subtype"] == "error" {
        let text = response["error"].as_str().unwrap_or("no reason given");
        return Err(if looks_like_login_problem(text) {
            EnhanceError::NotConfigured(format!("claude is not signed in: {text}"))
        } else {
            EnhanceError::Cli(format!("claude could not list its models: {text}"))
        });
    }
    let models = response["response"]["models"].as_array().ok_or_else(|| {
        EnhanceError::InvalidOutput("claude answered without a model list".into())
    })?;
    let choices = models
        .iter()
        .filter_map(|m| {
            let id = m["value"]
                .as_str()
                .filter(|v| !v.is_empty() && *v != "default")?;
            Some(choice(
                id,
                m["displayName"].as_str(),
                m["description"].as_str(),
            ))
        })
        .collect();
    Ok(dedup(choices))
}

fn list_codex_models(
    program: &std::path::Path,
    timeout: Duration,
) -> Result<Vec<ModelChoice>, EnhanceError> {
    let workdir = workdir()?;
    let mut session = Session::start(&Invocation {
        program,
        args: vec!["app-server".into()],
        envs: vec![],
        cwd: workdir.path(),
        stdin: None,
        timeout,
    })?;
    let hello = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "clientInfo": { "name": "sayso", "version": env!("CARGO_PKG_VERSION") } },
    });
    session.send(&hello.to_string())?;
    session.read_until(|line| rpc_reply(line, 1))??;
    session.send(&json!({ "jsonrpc": "2.0", "method": "initialized" }).to_string())?;

    let mut choices = Vec::new();
    let mut cursor: Option<String> = None;
    for page in 0..CODEX_MAX_PAGES {
        let id = 2 + page;
        let params = cursor
            .as_ref()
            .map_or_else(|| json!({}), |c| json!({ "cursor": c }));
        let request =
            json!({ "jsonrpc": "2.0", "id": id, "method": "model/list", "params": params });
        session.send(&request.to_string())?;
        let result = session.read_until(|line| rpc_reply(line, id))??;
        let (mut found, next) = parse_codex_page(&result)?;
        choices.append(&mut found);
        match next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    Ok(dedup(choices))
}

/// The reply to request `id`: its `result`, or the error it carries. None for
/// notifications, requests from the server, and replies to other requests.
fn rpc_reply(line: &str, id: u64) -> Option<Result<Value, EnhanceError>> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("method").is_some() || value["id"].as_u64() != Some(id) {
        return None;
    }
    if let Some(error) = value.get("error") {
        let text = error["message"].as_str().unwrap_or("no reason given");
        return Some(Err(if looks_like_login_problem(text) {
            EnhanceError::NotConfigured(format!("codex is not signed in: {text}"))
        } else {
            EnhanceError::Cli(format!("codex reported an error: {text}"))
        }));
    }
    Some(Ok(value["result"].clone()))
}

/// Read the `result` of one `model/list` reply: the visible models, and the
/// cursor of the next page when there is one.
fn parse_codex_page(result: &Value) -> Result<(Vec<ModelChoice>, Option<String>), EnhanceError> {
    let data = result["data"]
        .as_array()
        .ok_or_else(|| EnhanceError::InvalidOutput("codex answered without a model list".into()))?;
    let choices = data
        .iter()
        .filter(|m| m["hidden"].as_bool() != Some(true))
        .filter_map(|m| {
            let id = m["model"]
                .as_str()
                .or_else(|| m["id"].as_str())
                .filter(|v| !v.is_empty())?;
            Some(choice(
                id,
                m["displayName"].as_str(),
                m["description"].as_str(),
            ))
        })
        .collect();
    let next = result["nextCursor"]
        .as_str()
        .filter(|c| !c.is_empty())
        .map(str::to_string);
    Ok((choices, next))
}

/// A choice. An empty or missing name becomes the id, an empty note becomes none.
fn choice(id: &str, name: Option<&str>, description: Option<&str>) -> ModelChoice {
    ModelChoice {
        id: id.to_string(),
        name: name.filter(|n| !n.is_empty()).unwrap_or(id).to_string(),
        description: description.filter(|d| !d.is_empty()).map(str::to_string),
    }
}

/// Keep the first choice of each id, in order.
fn dedup(choices: Vec<ModelChoice>) -> Vec<ModelChoice> {
    let mut seen = std::collections::HashSet::new();
    choices
        .into_iter()
        .filter(|c| seen.insert(c.id.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE_LINE: &str = r#"{"type":"control_response","response":{"subtype":"success","request_id":"sayso-models","response":{"commands":[],"models":[
        {"value":"default","resolvedModel":"claude-opus-5-5","displayName":"Default (recommended)","description":"Opus 5.5 · Best for everyday, complex tasks"},
        {"value":"opus","displayName":"Opus 5.5","description":"For complex work and everyday tasks"},
        {"value":"haiku","resolvedModel":"claude-haiku-4-5-20251001","displayName":"Haiku 4.5","description":"Fastest for quick answers"},
        {"value":"bare"},
        {"value":"haiku","displayName":"Haiku again"}],"account":{}}}}"#;

    #[test]
    fn claude_answer_drops_default_and_keeps_order() {
        let choices = claude_answer(CLAUDE_LINE).unwrap().unwrap();
        let ids: Vec<&str> = choices.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["opus", "haiku", "bare"]);
        assert_eq!(choices[1].name, "Haiku 4.5");
        assert_eq!(
            choices[1].description.as_deref(),
            Some("Fastest for quick answers")
        );
        assert_eq!(
            (choices[2].name.as_str(), choices[2].description.as_deref()),
            ("bare", None)
        );
    }

    #[test]
    fn claude_answer_skips_other_lines() {
        assert!(claude_answer(r#"{"type":"system","subtype":"init"}"#).is_none());
        assert!(claude_answer("not json").is_none());
        let other = r#"{"type":"control_response","response":{"subtype":"success","request_id":"other","response":{}}}"#;
        assert!(claude_answer(other).is_none());
    }

    #[test]
    fn claude_errors_map_to_the_right_variants() {
        let error = |text: &str| {
            format!(
                r#"{{"type":"control_response","response":{{"subtype":"error","request_id":"sayso-models","error":"{text}"}}}}"#
            )
        };
        let login = claude_answer(&error("Not logged in. Run /login")).unwrap();
        assert!(matches!(login, Err(EnhanceError::NotConfigured(_))));
        let other = claude_answer(&error("boom")).unwrap();
        assert!(matches!(other, Err(EnhanceError::Cli(m)) if m.contains("boom")));
        let empty = r#"{"type":"control_response","response":{"subtype":"success","request_id":"sayso-models","response":{}}}"#;
        assert!(matches!(
            claude_answer(empty).unwrap(),
            Err(EnhanceError::InvalidOutput(_))
        ));
    }

    #[test]
    fn codex_page_skips_hidden_and_applies_field_rules() {
        let result = json!({
            "data": [
                { "id": "gpt-a", "model": "gpt-a-model", "displayName": "GPT A", "description": "Fast.", "hidden": false },
                { "id": "gpt-b", "model": "gpt-b", "displayName": "GPT B", "hidden": true },
                { "id": "gpt-c", "displayName": "", "description": "" },
                { "displayName": "No id" },
            ],
            "nextCursor": "page2",
        });
        let (choices, next) = parse_codex_page(&result).unwrap();
        assert_eq!(next.as_deref(), Some("page2"));
        assert_eq!(
            choices,
            [
                ModelChoice {
                    id: "gpt-a-model".into(),
                    name: "GPT A".into(),
                    description: Some("Fast.".into())
                },
                ModelChoice {
                    id: "gpt-c".into(),
                    name: "gpt-c".into(),
                    description: None
                },
            ]
        );
        let (_, last) = parse_codex_page(&json!({ "data": [], "nextCursor": null })).unwrap();
        assert_eq!(last, None);
        assert!(matches!(
            parse_codex_page(&json!({})),
            Err(EnhanceError::InvalidOutput(_))
        ));
    }

    #[test]
    fn rpc_reply_matches_only_its_own_id() {
        assert!(
            rpc_reply(
                r#"{"jsonrpc":"2.0","method":"thread/started","params":{}}"#,
                1
            )
            .is_none()
        );
        assert!(
            rpc_reply(r#"{"jsonrpc":"2.0","id":1,"method":"ask","params":{}}"#, 1).is_none(),
            "server request"
        );
        assert!(rpc_reply(r#"{"jsonrpc":"2.0","id":2,"result":{}}"#, 1).is_none());
        assert!(rpc_reply("junk", 1).is_none());
        assert_eq!(
            rpc_reply(r#"{"jsonrpc":"2.0","id":1,"result":{"a":1}}"#, 1)
                .unwrap()
                .unwrap(),
            json!({"a":1})
        );
        let login = rpc_reply(
            r#"{"id":1,"error":{"code":-1,"message":"please log in"}}"#,
            1,
        )
        .unwrap();
        assert!(matches!(login, Err(EnhanceError::NotConfigured(_))));
        let other = rpc_reply(r#"{"id":1,"error":{"code":-1,"message":"bad"}}"#, 1).unwrap();
        assert!(matches!(other, Err(EnhanceError::Cli(_))));
    }
}
