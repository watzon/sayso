//! Detection helpers for onboarding and the provider Settings view.

use crate::openai::{agent, map_ureq_error};
use sayso_core::enhance::{EnhanceError, EnhanceRequest, Enhancer, ModelChoice};
use std::path::PathBuf;
use std::time::Duration;

/// Where Ollama listens by default.
const OLLAMA_URL: &str = "http://localhost:11434";

/// Folders to search after `PATH`. A Dock-launched app has a short `PATH`, so
/// the usual install folders are listed here.
#[cfg(not(windows))]
fn common_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".claude/local"));
        dirs.push(home.join(".claude/local/node_modules/.bin"));
        dirs.push(home.join(".npm-global/bin"));
        dirs.push(home.join(".bun/bin"));
    }
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs
}

/// Folders to search after `PATH` on Windows: the native installers' folders
/// and npm's global folder.
#[cfg(windows)]
fn common_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        dirs.push(home.join(".local").join("bin"));
        dirs.push(home.join(".claude").join("local"));
        dirs.push(home.join(".bun").join("bin"));
    }
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
        dirs.push(appdata.join("npm"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        dirs.push(local.join("pnpm"));
        dirs.push(local.join("Volta").join("bin"));
    }
    dirs
}

fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default()
}

/// The first executable file called `name` in `dirs`.
#[cfg(not(windows))]
pub(crate) fn find_executable(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    dirs.iter().map(|d| d.join(name)).find(|candidate| {
        std::fs::metadata(candidate).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

/// The first file called `name` with an executable extension (`PATHEXT`) in
/// `dirs`. In each folder `.exe` wins over an npm `.cmd` shim.
#[cfg(windows)]
pub(crate) fn find_executable(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let names = crate::win_shim::executable_names(name);
    dirs.iter().flat_map(|d| names.iter().map(move |n| d.join(n))).find(|candidate| candidate.is_file())
}

fn detect(name: &str) -> Option<PathBuf> {
    let mut dirs = path_dirs();
    dirs.extend(common_dirs());
    find_executable(name, &dirs)
}

/// The `claude` binary, searched in `PATH` and the usual install folders.
pub fn detect_claude_cli() -> Option<PathBuf> {
    detect("claude")
}

/// The `codex` binary, searched in `PATH` and the usual install folders.
pub fn detect_codex_cli() -> Option<PathBuf> {
    detect("codex")
}

/// Ask a local Ollama for its models. None when Ollama does not answer within `timeout`.
pub fn detect_ollama(timeout: Duration) -> Option<Vec<String>> {
    detect_ollama_at(OLLAMA_URL, timeout)
}

/// Like [`detect_ollama`], for a server at another address.
pub fn detect_ollama_at(base_url: &str, timeout: Duration) -> Option<Vec<String>> {
    let url = format!("{}/api/tags", base_url.trim_end_matches('/'));
    let mut response = agent(base_url).get(&url).config().timeout_global(Some(timeout)).build().call().ok()?;
    let json: serde_json::Value = response.body_mut().read_json().ok()?;
    let mut names: Vec<String> =
        json["models"].as_array()?.iter().filter_map(|m| m["name"].as_str().map(str::to_string)).collect();
    names.sort();
    Some(names)
}

/// List the models of an OpenAI-compatible server: `GET {base_url}/models`.
/// Works for OpenAI, OpenRouter, Ollama (`/v1`), LM Studio, and llama.cpp.
/// The ids are sorted and unique. The request stops after 8 seconds.
pub fn list_models(base_url: &str, api_key: Option<&str>) -> Result<Vec<String>, EnhanceError> {
    let choices = fetch_model_choices(base_url, api_key, Duration::from_secs(8))?;
    Ok(choices.into_iter().map(|c| c.id).collect())
}

/// Like [`list_models`], with the names, and a deadline of `timeout`.
pub(crate) fn fetch_model_choices(
    base_url: &str,
    api_key: Option<&str>,
    timeout: Duration,
) -> Result<Vec<ModelChoice>, EnhanceError> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut request = agent(base_url).get(&url).config().timeout_global(Some(timeout)).http_status_as_error(false).build();
    if let Some(key) = api_key.filter(|k| !k.is_empty()) {
        request = request.header("Authorization", format!("Bearer {key}"));
    }
    let timeout_ms = timeout.as_millis() as u64;
    let mut response = request.call().map_err(|e| map_ureq_error(e, timeout_ms))?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string().map_err(|e| map_ureq_error(e, timeout_ms))?;
    if status >= 400 {
        return Err(EnhanceError::Http { status, body: text.chars().take(500).collect() });
    }
    parse_model_list(&text)
}

/// Read a `{"data": [{"id": ..., "name": ...}]}` body. The name is the entry's
/// `"name"` when it has one, else the id. Sorted by id, without duplicates.
/// Ollama with no models pulled sends `"data": null`, which is an empty list.
fn parse_model_list(text: &str) -> Result<Vec<ModelChoice>, EnhanceError> {
    let json: serde_json::Value =
        serde_json::from_str(text).map_err(|e| EnhanceError::InvalidOutput(format!("model list is not JSON: {e}")))?;
    let empty = Vec::new();
    let data = match &json["data"] {
        serde_json::Value::Null if json.get("data").is_some() => &empty,
        other => other.as_array().ok_or_else(|| EnhanceError::InvalidOutput("model list has no \"data\" array".into()))?,
    };
    let mut choices: Vec<ModelChoice> = data
        .iter()
        .filter_map(|m| {
            let id = m["id"].as_str()?;
            let name = m["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(id);
            Some(ModelChoice { id: id.to_string(), name: name.to_string(), description: None })
        })
        .collect();
    choices.sort_by(|a, b| a.id.cmp(&b.id));
    choices.dedup_by(|a, b| a.id == b.id);
    Ok(choices)
}

/// One tiny call for the "Test" button. Returns the time it took, in milliseconds.
/// This makes a real request, so automated tests do not call it with a real provider.
pub fn test_provider(enhancer: &dyn Enhancer) -> Result<u64, EnhanceError> {
    let request = EnhanceRequest {
        system_prompt: "Repeat the transcript exactly as the \"text\" field.".into(),
        transcript: "Testing one two three.".into(),
        model: None,
        temperature: Some(0.0),
        timeout: Duration::from_secs(20),
    };
    enhancer.enhance(&request).map(|r| r.elapsed_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg(unix)]
    fn touch(dir: &std::path::Path, name: &str, mode: u32) {
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn finds_exe_and_cmd_files_in_order() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(a.path().join("claude"), "").unwrap(); // no extension: skipped
        std::fs::write(b.path().join("claude.cmd"), "").unwrap();
        let dirs = vec![a.path().to_path_buf(), b.path().to_path_buf()];
        assert_eq!(find_executable("claude", &dirs), Some(b.path().join("claude.cmd")));
        std::fs::write(b.path().join("claude.exe"), "").unwrap();
        assert_eq!(find_executable("claude", &dirs), Some(b.path().join("claude.exe")));
        assert_eq!(find_executable("codex", &dirs), None);
    }

    #[test]
    #[cfg(unix)]
    fn finds_the_first_executable_in_order() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        touch(a.path(), "claude", 0o644); // not executable: skipped
        touch(b.path(), "claude", 0o755);
        let dirs = vec![a.path().to_path_buf(), b.path().to_path_buf()];
        assert_eq!(find_executable("claude", &dirs), Some(b.path().join("claude")));
        touch(a.path(), "claude", 0o755);
        assert_eq!(find_executable("claude", &dirs), Some(a.path().join("claude")));
        assert_eq!(find_executable("codex", &dirs), None);
    }

    #[test]
    fn a_directory_is_not_an_executable() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("claude")).unwrap();
        assert_eq!(find_executable("claude", &[dir.path().to_path_buf()]), None);
    }

    #[test]
    #[cfg(not(windows))]
    fn common_dirs_cover_the_usual_install_folders() {
        let dirs = common_dirs();
        for needle in ["/opt/homebrew/bin", "/usr/local/bin", ".local/bin", ".claude/local"] {
            assert!(dirs.iter().any(|d| d.ends_with(needle) || d.to_string_lossy().contains(needle)), "{needle}");
        }
    }

    #[test]
    fn model_list_uses_names_sorts_and_drops_duplicates() {
        let body = r#"{"data":[{"id":"b/two","name":"Two"},{"id":"a/one"},{"id":"b/two","name":"Again"},{"id":"c","name":""},{"name":"no id"}]}"#;
        let choices = parse_model_list(body).unwrap();
        let pairs: Vec<(&str, &str)> = choices.iter().map(|c| (c.id.as_str(), c.name.as_str())).collect();
        assert_eq!(pairs, [("a/one", "a/one"), ("b/two", "Two"), ("c", "c")]);
        assert!(choices.iter().all(|c| c.description.is_none()));
    }

    #[test]
    fn model_list_rejects_other_shapes() {
        assert!(matches!(parse_model_list("nope"), Err(EnhanceError::InvalidOutput(_))));
        assert!(matches!(parse_model_list(r#"{"models":[]}"#), Err(EnhanceError::InvalidOutput(_))));
    }

    #[test]
    fn ollama_with_no_models_is_an_empty_list() {
        assert_eq!(parse_model_list(r#"{"object":"list","data":null}"#).unwrap(), []);
    }
}
