//! The login of the providers that have one: read the state, and run the sign-in.
//!
//! - Claude CLI: Sayso runs the tool's own sign-in, and never reads or stores
//!   its credentials. The tool keeps them in its config folder, and `config_dir`
//!   of the provider says where that is. A Dock launch has no shell environment,
//!   so a user who set `CLAUDE_CONFIG_DIR` in the shell would look signed out.
//!   Every run of the tool gets the folder from the config instead.
//! - ChatGPT: Sayso runs the sign-in itself and keeps the tokens in the secret
//!   store (see `chatgpt`).
//! - ACP agents: the agent keeps its own sign-in. Sayso asks it to run the
//!   sign-in with an `authenticate` request (see `acp`).

use crate::acp::AcpSignIn;
use crate::chatgpt::{ChatGptSignIn, Endpoints};
use crate::cli::{self, Invocation, tail};
use crate::detect::detect_claude_cli;
use crate::secrets::SecretStore;
use sayso_core::config::{Provider, ProviderKind};
use sayso_core::enhance::EnhanceError;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a sign-in may run before Sayso stops it.
pub const SIGN_IN_LIMIT: Duration = Duration::from_secs(600);

/// How long Sayso waits for the tool when it reads the state at the end of a sign-in.
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);

/// The login state of a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthStatus {
    /// `account` is a short label for the UI, for example "Max plan · a@b.com", "ChatGPT plan", "API key". None when the tool gives nothing.
    SignedIn { account: Option<String> },
    /// `detail` is what the tool reported, for the "Copy details" action. One or a few lines, no secrets.
    SignedOut { detail: String },
    /// The command was not found.
    NotInstalled,
    /// The status command failed or printed something unreadable.
    Unknown { detail: String },
}

/// The tools that have a login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tool {
    Claude,
}

impl Tool {
    fn of(kind: &ProviderKind) -> Option<(Tool, Option<&str>)> {
        match kind {
            ProviderKind::ClaudeCli { path, .. } => Some((Tool::Claude, path.as_deref())),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Tool::Claude => "claude",
        }
    }

    /// The variable that tells the tool where its login is.
    fn env_name(self) -> &'static str {
        match self {
            Tool::Claude => "CLAUDE_CONFIG_DIR",
        }
    }

    fn status_args(self) -> Vec<std::ffi::OsString> {
        match self {
            Tool::Claude => vec!["auth".into(), "status".into()],
        }
    }

    fn sign_in_args(self) -> Vec<std::ffi::OsString> {
        match self {
            Tool::Claude => vec!["auth".into(), "login".into()],
        }
    }

    /// The binary from the provider config, else the one that Sayso finds.
    fn program(self, path: Option<&str>) -> Option<PathBuf> {
        path.map(PathBuf::from).or_else(match self {
            Tool::Claude => detect_claude_cli,
        })
    }
}

/// Replace a leading `~` with the home folder (`HOME`, or `USERPROFILE` on Windows).
pub(crate) fn expand_dir(dir: &str) -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    expand_with(dir, home.as_deref())
}

fn expand_with(dir: &str, home: Option<&Path>) -> PathBuf {
    let dir = dir.trim();
    let Some(home) = home else { return PathBuf::from(dir) };
    if dir == "~" {
        return home.to_path_buf();
    }
    match dir.strip_prefix("~/").or_else(|| if cfg!(windows) { dir.strip_prefix("~\\") } else { None }) {
        Some(rest) => home.join(rest),
        None => PathBuf::from(dir),
    }
}

/// The variable to set for a tool run, when the provider has a config folder.
pub(crate) fn config_env(tool: Tool, dir: Option<&str>) -> Option<(&'static str, String)> {
    let dir = dir.map(str::trim).filter(|d| !d.is_empty())?;
    Some((tool.env_name(), expand_dir(dir).to_string_lossy().into_owned()))
}

/// Add the entry of [`config_env`] to the variables of a run.
pub(crate) fn push_config_env<'a>(envs: &mut Vec<(&'a str, &'a str)>, entry: &'a Option<(&'static str, String)>) {
    if let Some((name, value)) = entry {
        envs.push((name, value.as_str()));
    }
}

/// True when the folder is known not to exist. In a Flatpak the folder is on
/// the host, which Sayso cannot look at, so the answer is no.
fn folder_missing(dir: &Path) -> bool {
    #[cfg(not(windows))]
    if sayso_core::flatpak::app_id().is_some() {
        return false;
    }
    !dir.exists()
}

/// The login state of a provider. None for a kind that has no login (OpenAI-compatible, Apple Intelligence). Blocks; `timeout` bounds the call.
/// `secrets` holds the ChatGPT sign-in and the secret variables of ACP agents.
/// An ACP agent is started (or the running one is used) and asked for a
/// session: it is signed in when it opens one.
pub fn auth_status(provider: &Provider, secrets: &dyn SecretStore, timeout: Duration) -> Option<AuthStatus> {
    match provider.kind {
        ProviderKind::ChatGpt { .. } => return Some(crate::chatgpt::status(secrets, &provider.id)),
        ProviderKind::Acp { .. } => return Some(crate::acp::status(provider, secrets, timeout)),
        _ => {}
    }
    cli_auth_status(provider, timeout)
}

/// The login state of a CLI provider, from its tool.
fn cli_auth_status(provider: &Provider, timeout: Duration) -> Option<AuthStatus> {
    let (tool, path) = Tool::of(&provider.kind)?;
    let dir = provider.config_dir();
    if let Some(missing) = dir.map(expand_dir).filter(|d| folder_missing(d)) {
        return Some(AuthStatus::SignedOut {
            detail: format!("{} config folder {} does not exist", tool.name(), missing.display()),
        });
    }
    let Some(program) = tool.program(path) else { return Some(AuthStatus::NotInstalled) };
    Some(read_status(tool, &program, dir, timeout))
}

fn read_status(tool: Tool, program: &Path, dir: Option<&str>, timeout: Duration) -> AuthStatus {
    let workdir = match cli::workdir() {
        Ok(workdir) => workdir,
        Err(e) => return AuthStatus::Unknown { detail: e.to_string() },
    };
    let dir_env = config_env(tool, dir);
    let mut envs = Vec::new();
    push_config_env(&mut envs, &dir_env);
    let output = cli::run(&Invocation { program, args: tool.status_args(), envs, cwd: workdir.path(), stdin: None, timeout });
    match output {
        Ok(out) => match tool {
            Tool::Claude => {
                // The state is on stdout. A crash that printed nothing there is still worth showing.
                let text = if out.stdout.trim().is_empty() { &out.stderr } else { &out.stdout };
                parse_claude_status(text)
            }
        },
        Err(EnhanceError::NotConfigured(_)) => AuthStatus::NotInstalled,
        Err(EnhanceError::Timeout(ms)) => {
            AuthStatus::Unknown { detail: format!("{} did not answer within {} ms", tool.name(), ms) }
        }
        Err(e) => AuthStatus::Unknown { detail: e.to_string() },
    }
}

/// Read the JSON of `claude auth status`.
fn parse_claude_status(stdout: &str) -> AuthStatus {
    let unreadable = || AuthStatus::Unknown { detail: format!("claude auth status: unreadable output: {}", tail(stdout)) };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(stdout.trim()) else { return unreadable() };
    let Some(logged_in) = value["loggedIn"].as_bool() else { return unreadable() };
    if !logged_in {
        let mut detail = String::from("claude auth status: loggedIn false");
        if let Some(dir) = value["configDirectory"].as_str() {
            detail.push_str(&format!(", configDirectory {dir}"));
        }
        return AuthStatus::SignedOut { detail };
    }
    let text = |key: &str| value[key].as_str().map(str::trim).filter(|s| !s.is_empty());
    let account = if text("authMethod") == Some("api_key") {
        Some("API key".to_string())
    } else {
        let plan = text("subscriptionType").map(|kind| {
            let mut chars = kind.chars();
            let first = chars.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
            format!("{first}{} plan", chars.as_str())
        });
        match (plan, text("email")) {
            (Some(plan), Some(email)) => Some(format!("{plan} · {email}")),
            (plan, email) => plan.or_else(|| email.map(String::from)),
        }
    };
    AuthStatus::SignedIn { account }
}

/// What a running sign-in reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInEvent {
    /// The sign-in page: the first http(s) URL the tool printed, or the page of the ChatGPT sign-in.
    Url(String),
    /// The sign-in ended, was cancelled, or ran out of time. The status is read again: that result decides, not the exit code.
    Finished(AuthStatus),
}

/// State that the caller and the helper threads share.
struct Shared {
    child: Mutex<Child>,
    stdin: Mutex<Option<ChildStdin>>,
    cancelled: AtomicBool,
    /// Set when the `SignIn` is dropped: nobody listens, so the end needs no status.
    dropped: AtomicBool,
    /// The folder of the tool run. It must outlive the tool.
    _workdir: tempfile::TempDir,
}

/// A running sign-in. Dropping it stops the sign-in.
pub struct SignIn {
    inner: Inner,
}

enum Inner {
    Cli(CliSignIn),
    ChatGpt(ChatGptSignIn),
    Acp(AcpSignIn),
}

impl SignIn {
    /// Start the sign-in of a provider. For the Claude CLI this is the tool's
    /// own `claude auth login`, with the provider's config folder, which it
    /// creates when it does not exist. For ChatGPT, Sayso runs the sign-in and
    /// saves the tokens in `secrets`. For an ACP agent, the agent runs the
    /// sign-in of its first method that it runs itself; an agent with only a
    /// terminal sign-in gives `NotConfigured` with the command to run. Err for
    /// a kind with no login, or when the tool is not found or does not start.
    /// This blocks while an ACP agent starts.
    pub fn start(provider: &Provider, secrets: Arc<dyn SecretStore>) -> Result<SignIn, EnhanceError> {
        Self::start_with_limit(provider, secrets, SIGN_IN_LIMIT)
    }

    /// Like [`SignIn::start`], with another time limit. For tests.
    #[doc(hidden)]
    pub fn start_with_limit(provider: &Provider, secrets: Arc<dyn SecretStore>, limit: Duration) -> Result<SignIn, EnhanceError> {
        let inner = match provider.kind {
            ProviderKind::ChatGpt { .. } => Inner::ChatGpt(ChatGptSignIn::start(secrets, &provider.id, limit, Endpoints::default())?),
            ProviderKind::Acp { .. } => Inner::Acp(AcpSignIn::start(provider, secrets, limit)?),
            _ => Inner::Cli(CliSignIn::start(provider, limit)?),
        };
        Ok(SignIn { inner })
    }

    /// True when the tool takes a code on its input (Claude).
    pub fn takes_code(&self) -> bool {
        match &self.inner {
            Inner::Cli(cli) => cli.takes_code,
            Inner::ChatGpt(_) | Inner::Acp(_) => false,
        }
    }

    /// True when the sign-in opens its page in the browser itself, as the
    /// Claude CLI and ACP agents do. Otherwise the caller opens the page of the `Url` event.
    pub fn opens_browser(&self) -> bool {
        matches!(self.inner, Inner::Cli(_) | Inner::Acp(_))
    }

    /// Blocks until the next event. None after `Finished` was returned.
    pub fn next_event(&self) -> Option<SignInEvent> {
        match &self.inner {
            Inner::Cli(cli) => cli.next_event(),
            Inner::ChatGpt(chatgpt) => chatgpt.next_event(),
            Inner::Acp(acp) => acp.next_event(),
        }
    }

    /// Write the code from the browser to the tool, with a line end.
    pub fn send_code(&self, code: &str) -> Result<(), EnhanceError> {
        match &self.inner {
            Inner::Cli(cli) => cli.send_code(code),
            Inner::ChatGpt(_) => Err(EnhanceError::Cli("the ChatGPT sign-in takes no code".into())),
            Inner::Acp(_) => Err(EnhanceError::Cli("the sign-in of the agent takes no code".into())),
        }
    }

    /// Stop the sign-in. `next_event` then returns `Finished` with the current status.
    pub fn cancel(&self) {
        match &self.inner {
            Inner::Cli(cli) => cli.cancel(),
            Inner::ChatGpt(chatgpt) => chatgpt.cancel(),
            Inner::Acp(acp) => acp.cancel(),
        }
    }
}

/// A running sign-in of a CLI tool. Dropping it kills the tool.
struct CliSignIn {
    shared: Arc<Shared>,
    /// None after `Finished` was returned.
    events: Mutex<Option<Receiver<SignInEvent>>>,
    takes_code: bool,
}

impl CliSignIn {
    fn start(provider: &Provider, limit: Duration) -> Result<CliSignIn, EnhanceError> {
        let (tool, path) = Tool::of(&provider.kind)
            .ok_or_else(|| EnhanceError::NotConfigured("this provider has no sign-in".into()))?;
        let program = tool
            .program(path)
            .ok_or_else(|| EnhanceError::NotConfigured(format!("the {} command was not found", tool.name())))?;
        let dir = provider.config_dir();
        // In a Flatpak the folder is on the host. The tool makes it there, or fails with a clear message.
        #[cfg(not(windows))]
        let sandboxed = sayso_core::flatpak::app_id().is_some();
        #[cfg(windows)]
        let sandboxed = false;
        if let (Some(dir), false) = (dir, sandboxed) {
            let dir = expand_dir(dir);
            std::fs::create_dir_all(&dir)
                .map_err(|e| EnhanceError::Cli(format!("could not create the folder {}: {e}", dir.display())))?;
        }

        let takes_code = tool == Tool::Claude;
        let workdir = cli::workdir()?;
        let dir_env = config_env(tool, dir);
        let mut envs = Vec::new();
        push_config_env(&mut envs, &dir_env);
        let inv = Invocation { program: &program, args: tool.sign_in_args(), envs, cwd: workdir.path(), stdin: None, timeout: limit };
        let mut child = cli::spawn(&inv, takes_code, false)?;
        let (tx, rx) = channel();
        let sent_url = Arc::new(AtomicBool::new(false));
        let (done_tx, done_rx) = channel();
        let mut readers = 0;
        if let Some(out) = child.stdout.take() {
            readers += 1;
            read_lines(out, tx.clone(), sent_url.clone(), done_tx.clone());
        }
        if let Some(err) = child.stderr.take() {
            readers += 1;
            read_lines(err, tx.clone(), sent_url, done_tx);
        }
        let shared = Arc::new(Shared {
            stdin: Mutex::new(child.stdin.take()),
            child: Mutex::new(child),
            cancelled: AtomicBool::new(false),
            dropped: AtomicBool::new(false),
            _workdir: workdir,
        });
        let supervisor = Supervisor { shared: shared.clone(), provider: provider.clone(), limit, tx, done: done_rx, readers };
        std::thread::spawn(move || supervisor.run());
        Ok(CliSignIn { shared, events: Mutex::new(Some(rx)), takes_code })
    }

    fn next_event(&self) -> Option<SignInEvent> {
        let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        let event = events.as_ref()?.recv().ok();
        if !matches!(event, Some(SignInEvent::Url(_))) {
            *events = None;
        }
        event
    }

    fn send_code(&self, code: &str) -> Result<(), EnhanceError> {
        let mut stdin = self.shared.stdin.lock().unwrap_or_else(|e| e.into_inner());
        let pipe = stdin.as_mut().ok_or_else(|| EnhanceError::Cli("the tool takes no code".into()))?;
        pipe.write_all(code.trim().as_bytes())
            .and_then(|()| pipe.write_all(b"\n"))
            .and_then(|()| pipe.flush())
            .map_err(|e| EnhanceError::Cli(format!("could not write to the tool: {e}")))
    }

    fn cancel(&self) {
        self.shared.cancelled.store(true, Ordering::SeqCst);
        let _ = self.shared.child.lock().unwrap_or_else(|e| e.into_inner()).kill();
    }
}

impl Drop for CliSignIn {
    fn drop(&mut self) {
        self.shared.dropped.store(true, Ordering::SeqCst);
        let mut child = self.shared.child.lock().unwrap_or_else(|e| e.into_inner());
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Watches the tool: it waits for the end, the limit, or a cancel, then sends `Finished` once.
struct Supervisor {
    shared: Arc<Shared>,
    provider: Provider,
    limit: Duration,
    tx: Sender<SignInEvent>,
    /// One message per reader thread that reached the end of its pipe.
    done: Receiver<()>,
    readers: usize,
}

impl Supervisor {
    fn run(self) {
        let deadline = Instant::now() + self.limit;
        loop {
            {
                let mut child = self.shared.child.lock().unwrap_or_else(|e| e.into_inner());
                if !matches!(child.try_wait(), Ok(None)) {
                    break;
                }
                if Instant::now() >= deadline || self.shared.cancelled.load(Ordering::SeqCst) {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // Let the readers finish, so a `Url` event comes before `Finished`. A helper process of
        // the tool may keep a pipe open, so the wait is short.
        for _ in 0..self.readers {
            if self.done.recv_timeout(Duration::from_millis(500)).is_err() {
                break;
            }
        }
        if self.shared.dropped.load(Ordering::SeqCst) {
            return;
        }
        let status = cli_auth_status(&self.provider, STATUS_TIMEOUT)
            .unwrap_or(AuthStatus::Unknown { detail: "this provider has no sign-in".into() });
        let _ = self.tx.send(SignInEvent::Finished(status));
    }
}

/// Read a pipe line by line on a helper thread. The first URL becomes a `Url` event.
fn read_lines<R: Read + Send + 'static>(pipe: R, tx: Sender<SignInEvent>, sent_url: Arc<AtomicBool>, done: Sender<()>) {
    std::thread::spawn(move || {
        for raw in BufReader::new(pipe).split(b'\n') {
            let Ok(raw) = raw else { break };
            let line = String::from_utf8_lossy(&raw);
            if let Some(url) = find_url(&line)
                && !sent_url.swap(true, Ordering::SeqCst)
            {
                let _ = tx.send(SignInEvent::Url(url));
            }
        }
        let _ = done.send(());
    });
}

/// The first token of a line that starts with `https://` or `http://`.
fn find_url(line: &str) -> Option<String> {
    let clean = strip_escapes(line);
    clean
        .split_whitespace()
        .find(|token| token.starts_with("https://") || token.starts_with("http://"))
        .map(|token| token.trim_end_matches(['.', ',', ';', ')', '>', '"', '\'']).to_string())
}

/// Remove terminal color codes (`ESC [ ... letter`), which a tool may print around a URL.
fn strip_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(status: AuthStatus) -> Option<String> {
        match status {
            AuthStatus::SignedIn { account } => account,
            other => panic!("expected SignedIn, got {other:?}"),
        }
    }

    #[test]
    fn claude_signed_out() {
        let out = r#"{"loggedIn": false, "authMethod": "none", "apiProvider": "firstParty", "analyticsDisabled": false, "projectsDirectory": "/Users/x/.claude/projects", "configDirectory": "/Users/x/.claude"}"#;
        assert_eq!(
            parse_claude_status(out),
            AuthStatus::SignedOut { detail: "claude auth status: loggedIn false, configDirectory /Users/x/.claude".into() }
        );
    }

    #[test]
    fn claude_signed_in_labels() {
        let full = r#"{"loggedIn": true, "authMethod": "claude.ai", "email": "a@b.com", "orgId": "o", "orgName": "O", "subscriptionType": "max"}"#;
        assert_eq!(account(parse_claude_status(full)).as_deref(), Some("Max plan · a@b.com"));
        let email = r#"{"loggedIn": true, "authMethod": "claude.ai", "email": "a@b.com"}"#;
        assert_eq!(account(parse_claude_status(email)).as_deref(), Some("a@b.com"));
        let plan = r#"{"loggedIn": true, "subscriptionType": "pro"}"#;
        assert_eq!(account(parse_claude_status(plan)).as_deref(), Some("Pro plan"));
        let key = r#"{"loggedIn": true, "authMethod": "api_key"}"#;
        assert_eq!(account(parse_claude_status(key)).as_deref(), Some("API key"));
        assert_eq!(account(parse_claude_status(r#"{"loggedIn": true}"#)), None);
    }

    #[test]
    fn claude_unreadable_output_is_unknown() {
        assert!(matches!(parse_claude_status("not json"), AuthStatus::Unknown { .. }));
        assert!(matches!(parse_claude_status(""), AuthStatus::Unknown { .. }));
        assert!(matches!(parse_claude_status(r#"{"email": "a@b.com"}"#), AuthStatus::Unknown { .. }));
    }

    #[test]
    fn home_folder_is_expanded() {
        let home = Path::new("/home/u");
        assert_eq!(expand_with("~/.claude-work", Some(home)), Path::new("/home/u/.claude-work"));
        assert_eq!(expand_with("~", Some(home)), home);
        assert_eq!(expand_with("/abs/dir", Some(home)), Path::new("/abs/dir"));
        assert_eq!(expand_with("~other/x", Some(home)), Path::new("~other/x"));
        assert_eq!(expand_with("~/x", None), Path::new("~/x"));
    }

    #[test]
    fn config_env_names_the_variable_of_the_tool() {
        assert_eq!(config_env(Tool::Claude, Some("/a")), Some(("CLAUDE_CONFIG_DIR", "/a".into())));
        assert_eq!(config_env(Tool::Claude, None), None);
        assert_eq!(config_env(Tool::Claude, Some("  ")), None);
    }

    #[test]
    fn finds_the_first_url() {
        assert_eq!(
            find_url("If the browser didn't open, visit: https://example.test/auth?x=1").as_deref(),
            Some("https://example.test/auth?x=1")
        );
        assert_eq!(find_url("\u{1b}[34mhttp://localhost:1455/auth\u{1b}[0m").as_deref(), Some("http://localhost:1455/auth"));
        assert_eq!(find_url("Open https://a.test/x.").as_deref(), Some("https://a.test/x"));
        assert_eq!(find_url("no link here"), None);
        assert_eq!(find_url("ftp://a.test"), None);
    }
}
