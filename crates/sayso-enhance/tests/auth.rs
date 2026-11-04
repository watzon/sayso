//! Login state and sign-in against a fake `claude` script.
//! No real tool runs and no browser opens. The fake tools are shell scripts,
//! so this file runs on Unix only.
#![cfg(unix)]

mod common;

use common::fake_tool;
use sayso_core::config::{Provider, ProviderKind};
use sayso_enhance::{AuthStatus, MemoryStore, SecretStore, SignIn, SignInEvent, auth_status};
use std::sync::Arc;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn claude(path: PathBuf, config_dir: Option<&Path>) -> Provider {
    Provider {
        id: "claude".into(),
        name: "Claude".into(),
        kind: ProviderKind::ClaudeCli {
            path: Some(path.display().to_string()),
            model: "haiku".into(),
            config_dir: config_dir.map(|d| d.display().to_string()),
            thinking: Default::default(),
        },
    }
}

fn store() -> Arc<dyn SecretStore> {
    Arc::new(MemoryStore::new())
}

/// A fake `claude`. `auth login` prints a URL, reads one line (the code), and
/// signs in by writing a file in the config folder. `auth status` reads that file.
fn fake_claude(dir: &Path) -> PathBuf {
    let seen = dir.display();
    let script = format!(
        r#"case "$1 $2" in
"auth login")
  echo $$ > {seen}/pid
  echo "If the browser didn't open, visit: https://example.test/auth?x=1"
  read code
  echo "$code" > {seen}/code
  echo "$CLAUDE_CONFIG_DIR" > {seen}/env
  touch "$CLAUDE_CONFIG_DIR/signed-in"
  ;;
"auth status")
  if [ -f "$CLAUDE_CONFIG_DIR/signed-in" ]; then
    echo '{{"loggedIn": true, "authMethod": "claude.ai", "email": "a@b.com", "subscriptionType": "max"}}'
  else
    echo '{{"loggedIn": false, "authMethod": "none", "configDirectory": "'"$CLAUDE_CONFIG_DIR"'"}}'
  fi
  ;;
esac"#
    );
    fake_tool(dir, "claude", &script)
}

/// A fake `claude` whose sign-in prints a URL and then waits for a long time.
fn hanging_claude(dir: &Path) -> PathBuf {
    let seen = dir.display();
    let script = format!(
        r#"case "$1 $2" in
"auth login")
  echo $$ > {seen}/pid
  echo "visit https://example.test/wait"
  exec sleep 30
  ;;
"auth status")
  echo '{{"loggedIn": false}}'
  ;;
esac"#
    );
    fake_tool(dir, "claude", &script)
}

fn pid_is_gone(dir: &Path) -> bool {
    let pid = std::fs::read_to_string(dir.join("pid")).unwrap();
    // A reaped process cannot be signalled.
    !Command::new("kill").args(["-0", pid.trim()]).output().unwrap().status.success()
}

fn url_event(sign_in: &SignIn) -> String {
    match sign_in.next_event() {
        Some(SignInEvent::Url(url)) => url,
        other => panic!("expected a URL, got {other:?}"),
    }
}

#[test]
fn claude_sign_in_url_then_code_then_signed_in() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("login").join("claude");
    let provider = claude(fake_claude(dir.path()), Some(&config));
    assert!(matches!(auth_status(&provider, &MemoryStore::new(), Duration::from_secs(10)), Some(AuthStatus::SignedOut { .. })), "a missing folder is signed out");

    let sign_in = SignIn::start(&provider, store()).unwrap();
    assert!(sign_in.takes_code());
    assert!(config.is_dir(), "the configured folder is created");
    assert_eq!(url_event(&sign_in), "https://example.test/auth?x=1");
    sign_in.send_code("code-123").unwrap();
    let done = sign_in.next_event();
    assert_eq!(
        done,
        Some(SignInEvent::Finished(AuthStatus::SignedIn { account: Some("Max plan · a@b.com".into()) }))
    );
    assert_eq!(sign_in.next_event(), None, "nothing comes after Finished");
    assert_eq!(std::fs::read_to_string(dir.path().join("code")).unwrap(), "code-123\n");
    assert_eq!(std::fs::read_to_string(dir.path().join("env")).unwrap().trim(), config.display().to_string());
    assert!(pid_is_gone(dir.path()));
}

#[test]
fn claude_status_signed_out_names_the_folder() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("cfg");
    std::fs::create_dir(&config).unwrap();
    let provider = claude(fake_claude(dir.path()), Some(&config));
    match auth_status(&provider, &MemoryStore::new(), Duration::from_secs(10)) {
        Some(AuthStatus::SignedOut { detail }) => assert!(detail.contains(&config.display().to_string()), "{detail}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn cancel_ends_with_the_current_status_and_no_process_is_left() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("cfg");
    let provider = claude(hanging_claude(dir.path()), Some(&config));
    let sign_in = SignIn::start(&provider, store()).unwrap();
    assert_eq!(url_event(&sign_in), "https://example.test/wait");
    let started = Instant::now();
    sign_in.cancel();
    assert!(matches!(sign_in.next_event(), Some(SignInEvent::Finished(AuthStatus::SignedOut { .. }))));
    assert_eq!(sign_in.next_event(), None);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(pid_is_gone(dir.path()));
}

#[test]
fn the_limit_kills_the_tool() {
    let dir = tempfile::tempdir().unwrap();
    let provider = claude(hanging_claude(dir.path()), None);
    let started = Instant::now();
    let sign_in = SignIn::start_with_limit(&provider, store(), Duration::from_millis(300)).unwrap();
    assert_eq!(url_event(&sign_in), "https://example.test/wait");
    assert!(matches!(sign_in.next_event(), Some(SignInEvent::Finished(AuthStatus::SignedOut { .. }))));
    assert!(started.elapsed() < Duration::from_secs(10), "the sleep of 30 s was cut short");
    assert!(pid_is_gone(dir.path()));
}

#[test]
fn dropping_the_sign_in_kills_the_tool() {
    let dir = tempfile::tempdir().unwrap();
    let provider = claude(hanging_claude(dir.path()), None);
    let sign_in = SignIn::start(&provider, store()).unwrap();
    assert_eq!(url_event(&sign_in), "https://example.test/wait");
    drop(sign_in);
    assert!(pid_is_gone(dir.path()));
}

#[test]
fn a_missing_binary_is_not_installed() {
    let provider = claude(PathBuf::from("/nowhere/claude"), None);
    assert_eq!(auth_status(&provider, &MemoryStore::new(), Duration::from_secs(5)), Some(AuthStatus::NotInstalled));
    assert!(SignIn::start(&provider, store()).is_err());
}

#[test]
fn kinds_without_a_login_have_no_status() {
    let provider = Provider { id: "a".into(), name: "A".into(), kind: ProviderKind::AppleIntelligence };
    assert_eq!(auth_status(&provider, &MemoryStore::new(), Duration::from_secs(1)), None);
    assert!(SignIn::start(&provider, store()).is_err());
}

#[test]
fn a_tilde_folder_is_expanded_before_the_check() {
    let dir = tempfile::tempdir().unwrap();
    let script = format!("echo \"$CLAUDE_CONFIG_DIR\" > {}/env\necho '{{\"loggedIn\": true}}'", dir.path().display());
    let tool = fake_tool(dir.path(), "claude", &script);
    let mut provider = claude(tool, None);
    if let ProviderKind::ClaudeCli { config_dir, .. } = &mut provider.kind {
        *config_dir = Some("~/some-claude-folder".into());
    }
    let home = std::env::var("HOME").unwrap();
    // The folder does not exist, so the tool is not run: the answer comes without it.
    assert!(matches!(auth_status(&provider, &MemoryStore::new(), Duration::from_secs(5)), Some(AuthStatus::SignedOut { detail }) if detail.contains(&home)));
}

#[test]
fn enhance_runs_get_the_config_folder() {
    use sayso_core::enhance::{EnhanceRequest, Enhancer};
    use sayso_enhance::ClaudeCli;
    let dir = tempfile::tempdir().unwrap();
    let request = EnhanceRequest {
        system_prompt: "s".into(),
        transcript: "t".into(),
        model: None,
        temperature: None,
        timeout: Duration::from_secs(20),
    };
    // A fake that speaks the stream-JSON protocol: it answers `initialize`,
    // then the user message, and keeps stdin open in between, as claude does.
    let init = r#"{"type":"control_response","response":{"request_id":"sayso-init","subtype":"success","response":{}}}"#;
    let envelope = r#"{"type":"result","subtype":"success","is_error":false,"structured_output":{"text":"ok"}}"#;
    let script = format!(
        r#"echo "$CLAUDE_CONFIG_DIR" > {dir}/claude-env
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*) echo '{init}' ;;
    *'"type":"user"'*) echo '{envelope}'; exit 0 ;;
  esac
done
"#,
        dir = dir.path().display()
    );
    let tool = fake_tool(dir.path(), "claude", &script);
    ClaudeCli::new("c", Some(tool), "haiku")
        .with_spare(false)
        .with_config_dir(Some("/some/claude".into()))
        .enhance(&request)
        .unwrap();
    assert_eq!(std::fs::read_to_string(dir.path().join("claude-env")).unwrap().trim(), "/some/claude");
}
