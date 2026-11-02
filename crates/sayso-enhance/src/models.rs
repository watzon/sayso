//! List the models a provider offers, for the model picker.
//!
//! - OpenAI-compatible servers: `GET {base_url}/models`.
//! - Claude Code has no "list models" command, but its stream-JSON control
//!   protocol answers an `initialize` request with the model list. No prompt
//!   is sent, so this costs nothing.
//! - ChatGPT: the model list of the Codex backend (see `chatgpt`).
//! - Apple Intelligence: the one model that the engine reports.
//!
//! All calls block: run them on a background thread.

use crate::auth::{Tool, config_env, push_config_env};
use crate::cli::{Invocation, Session, looks_like_login_problem, workdir};
use crate::detect::{detect_claude_cli, fetch_model_choices};
use crate::secrets::SecretStore;
use sayso_core::config::{Provider, ProviderKind, sort_thinking};
use sayso_core::enhance::{EnhanceError, LanguageModel, ModelChoice};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;

/// The request id of the Claude `initialize` request. The answer carries it back.
const CLAUDE_REQUEST_ID: &str = "sayso-models";

/// The models `provider` offers. `timeout` bounds the whole call.
///
/// HTTP lists are sorted by id. CLI lists keep the order of the tool. There
/// are never two choices with the same id. Claude's "default" entry is left
/// out: it means whatever Claude Code defaults to, which is usually its
/// slowest model.
pub fn list_provider_models(
    provider: &Provider,
    secrets: &dyn SecretStore,
    engine: Option<&dyn LanguageModel>,
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
            list_claude_models(&program, provider.config_dir(), timeout)
        }
        ProviderKind::ChatGpt { .. } => crate::chatgpt::list_models(secrets, &provider.id, timeout),
        // macOS chooses the model, so the list has one entry.
        ProviderKind::AppleIntelligence => {
            let engine = engine.ok_or_else(|| EnhanceError::NotConfigured("the engine does not run".into()))?;
            let name = engine.model_name()?;
            Ok(vec![choice(&name, None, Some(&sayso_i18n::t!("enhance.runs_on_this_mac")))])
        }
        // The catalog fixes the model of a cleanup model.
        ProviderKind::CleanupModel { .. } => Ok(Vec::new()),
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

fn list_claude_models(
    program: &std::path::Path,
    config_dir: Option<&str>,
    timeout: Duration,
) -> Result<Vec<ModelChoice>, EnhanceError> {
    let workdir = workdir()?;
    let dir_env = config_env(Tool::Claude, config_dir);
    let mut envs = vec![("MAX_THINKING_TOKENS", "0")];
    push_config_env(&mut envs, &dir_env);
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
        envs,
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
            let mut choice = choice(id, m["displayName"].as_str(), m["description"].as_str());
            choice.thinking = levels(&m["supportedEffortLevels"], |level| level.as_str());
            Some(choice)
        })
        .collect();
    Ok(dedup(choices))
}

/// A choice. An empty or missing name becomes the id, an empty note becomes none.
fn choice(id: &str, name: Option<&str>, description: Option<&str>) -> ModelChoice {
    ModelChoice {
        id: id.to_string(),
        name: name.filter(|n| !n.is_empty()).unwrap_or(id).to_string(),
        description: description.filter(|d| !d.is_empty()).map(str::to_string),
        thinking: Vec::new(),
    }
}

/// The thinking levels in `list`, in order. `name` reads the name of one
/// item. A missing list is a model without levels.
pub(crate) fn levels<'a>(list: &'a Value, name: impl Fn(&'a Value) -> Option<&'a str>) -> Vec<String> {
    let names = list.as_array().map(|items| items.iter().filter_map(name).filter(|n| !n.is_empty()).map(str::to_string).collect());
    sort_thinking(names.unwrap_or_default())
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
        {"value":"opus","displayName":"Opus 5.5","description":"For complex work and everyday tasks","supportsEffort":true,"supportedEffortLevels":["max","low","medium","high","xhigh"]},
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
        // The levels of the tool, from the least thinking to the most. Haiku 4.5 has none.
        assert_eq!(choices[0].thinking, ["low", "medium", "high", "xhigh", "max"]);
        assert!(choices[1].thinking.is_empty());
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
}
