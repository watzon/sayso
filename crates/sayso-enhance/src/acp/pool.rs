//! The running agents: one started, initialized agent per provider. Each
//! request opens a new session on it. An agent that stays up stops after
//! [`super::IDLE_LIMIT`] without use. A dead agent is started again.

use super::client::{self, AcpClient, Deadline};
use super::rpc::Connection;
use crate::cli::{self, Invocation, looks_like_login_problem, tail};
use parking_lot::Mutex;
use sayso_core::enhance::EnhanceError;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;

/// How to start an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaunchSpec {
    pub command: String,
    pub args: Vec<String>,
    /// Extra environment variables, also the secret ones.
    pub env: Vec<(String, String)>,
}

impl LaunchSpec {
    /// A hash of the command, the arguments and the environment. A provider
    /// whose fingerprint changed gets a new agent.
    fn fingerprint(&self) -> String {
        let mut hash = Sha256::new();
        for part in std::iter::once(&self.command).chain(&self.args) {
            hash.update(part.as_bytes());
            hash.update([0]);
        }
        for (name, value) in &self.env {
            hash.update(name.as_bytes());
            hash.update([1]);
            hash.update(value.as_bytes());
            hash.update([0]);
        }
        format!("{:x}", hash.finalize())
    }

    /// The name of the command, for messages until the agent gives its own name.
    pub fn label(&self) -> String {
        Path::new(&self.command).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.command.clone())
    }
}

/// The last part of a pipe, for error messages.
pub(crate) struct Tail {
    text: Mutex<String>,
    done: AtomicBool,
}

impl Tail {
    pub fn new() -> Arc<Tail> {
        Arc::new(Tail { text: Mutex::new(String::new()), done: AtomicBool::new(false) })
    }

    /// A tail that gets no more text.
    pub fn finished() -> Arc<Tail> {
        let tail = Tail::new();
        tail.done.store(true, Ordering::SeqCst);
        tail
    }

    /// Read a pipe on a helper thread and keep its last 4 KB.
    fn read<R: Read + Send + 'static>(pipe: Option<R>) -> Arc<Tail> {
        let tail = Tail::new();
        let Some(mut pipe) = pipe else { return Tail::finished() };
        let writer = tail.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut text = writer.text.lock();
                text.push_str(&String::from_utf8_lossy(&buf[..n]));
                if text.len() > 4096 {
                    let cut = text.ceil_char_boundary(text.len() - 4096);
                    text.drain(..cut);
                }
            }
            writer.done.store(true, Ordering::SeqCst);
        });
        tail
    }

    /// The last lines. Waits a short while for the end of the pipe, so the
    /// lines of an agent that just stopped are there.
    pub fn lines(&self) -> String {
        for _ in 0..15 {
            if self.done.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        tail(&self.text.lock())
    }
}

/// The pipes of a started agent.
pub(crate) struct Transport {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    /// The process. None for an agent in this process (tests).
    pub child: Option<Child>,
    pub stderr: Arc<Tail>,
    /// An empty folder for the sessions. It lives as long as the agent.
    pub workdir: tempfile::TempDir,
}

/// Starts an agent. The pool of the app starts processes; tests start agents in threads.
pub(crate) type Spawner = Box<dyn Fn(&LaunchSpec) -> Result<Transport, EnhanceError> + Send + Sync>;

/// A started, initialized agent.
pub(crate) struct Agent {
    pub client: AcpClient,
    child: Mutex<Option<Child>>,
    stderr: Arc<Tail>,
    workdir: tempfile::TempDir,
    last_used: Mutex<Instant>,
    users: AtomicUsize,
}

impl Agent {
    /// The folder for new sessions.
    pub fn cwd(&self) -> &Path {
        self.workdir.path()
    }

    /// Close the input of the agent, give it a second to exit, then kill it.
    /// An `npx` agent runs in a child of `npx`: it exits when its input closes.
    fn stop(&self) {
        self.client.close();
        if let Some(mut child) = self.child.lock().take()
            && !matches!(child.wait_timeout(Duration::from_secs(1)), Ok(Some(_)))
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Add the error output of an agent that stopped to an error.
    fn explain(&self, error: EnhanceError) -> EnhanceError {
        if !self.client.is_closed() {
            return error;
        }
        explain(error, self.client.label(), &self.stderr)
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Add the error output to the error of an agent that stopped. Output that
/// talks about a login makes it a missing sign-in.
fn explain(error: EnhanceError, label: &str, stderr: &Tail) -> EnhanceError {
    let lines = stderr.lines();
    match error {
        _ if lines.is_empty() => error,
        _ if looks_like_login_problem(&lines) => EnhanceError::NotConfigured(format!("not signed in to {label}: {lines}")),
        EnhanceError::Cli(message) => EnhanceError::Cli(format!("{message}: {lines}")),
        other => other,
    }
}

/// Stop an agent on a helper thread, so the caller does not wait for it.
fn stop_later(agent: Arc<Agent>) {
    std::thread::spawn(move || drop(agent));
}

/// The agent of one provider.
struct Slot {
    fingerprint: String,
    agent: Arc<Agent>,
}

type Cell = Arc<Mutex<Option<Slot>>>;

/// The running agents, by provider id.
pub(crate) struct Pool {
    spawn: Spawner,
    idle_limit: Duration,
    cells: Arc<Mutex<HashMap<String, Cell>>>,
    reaper: AtomicBool,
}

impl Pool {
    pub fn new(spawn: Spawner, idle_limit: Duration) -> Self {
        Self { spawn, idle_limit, cells: Arc::new(Mutex::new(HashMap::new())), reaper: AtomicBool::new(false) }
    }

    /// Start an agent and initialize it. The pool does not keep it.
    pub fn start(&self, spec: &LaunchSpec, deadline: Deadline) -> Result<Agent, EnhanceError> {
        let Transport { reader, writer, mut child, stderr, workdir } = (self.spawn)(spec)?;
        let conn = Connection::new(reader, writer, client::handler());
        match AcpClient::initialize(conn, &spec.label(), deadline) {
            Ok(client) => {
                Ok(Agent { client, child: Mutex::new(child), stderr, workdir, last_used: Mutex::new(Instant::now()), users: AtomicUsize::new(0) })
            }
            Err(error) => {
                if let Some(child) = child.as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                Err(explain(error, &spec.label(), &stderr))
            }
        }
    }

    /// Run `f` with the agent of `key`. With `keep_running` the agent stays
    /// in the pool; else it is started for this call and stopped after it.
    /// An agent in the pool that died is started again, once per call. An
    /// agent that timed out is stopped: it may still work on the request.
    pub fn run<T>(
        &self,
        key: &str,
        spec: &LaunchSpec,
        keep_running: bool,
        deadline: Deadline,
        f: impl Fn(&Agent) -> Result<T, EnhanceError>,
    ) -> Result<T, EnhanceError> {
        if !keep_running {
            let agent = Arc::new(self.start(spec, deadline)?);
            let result = f(&agent).map_err(|e| agent.explain(e));
            stop_later(agent);
            return result;
        }
        let mut retried = false;
        loop {
            let (agent, fresh) = self.checkout(key, spec, deadline)?;
            let result = f(&agent);
            *agent.last_used.lock() = Instant::now();
            agent.users.fetch_sub(1, Ordering::SeqCst);
            if agent.client.is_unusable() {
                self.discard(key, &agent);
                // An agent that waited in the pool may have died since: start it again once.
                if result.is_err() && agent.client.is_closed() && !fresh && !retried {
                    retried = true;
                    continue;
                }
            }
            return result.map_err(|e| agent.explain(e));
        }
    }

    /// The agent of `key`, started when there is none, when it died, or when
    /// its launch changed. True when it was started for this call. The agent
    /// counts the caller as a user, so the reaper leaves it alone.
    fn checkout(&self, key: &str, spec: &LaunchSpec, deadline: Deadline) -> Result<(Arc<Agent>, bool), EnhanceError> {
        self.start_reaper();
        let cell = self.cells.lock().entry(key.to_string()).or_default().clone();
        // A second request for the same provider waits while the first starts the agent.
        let mut slot = cell.try_lock_until(deadline.at).ok_or(EnhanceError::Timeout(deadline.timeout_ms()))?;
        let fingerprint = spec.fingerprint();
        if let Some(current) = slot.as_ref() {
            if current.fingerprint == fingerprint && !current.agent.client.is_unusable() {
                current.agent.users.fetch_add(1, Ordering::SeqCst);
                return Ok((current.agent.clone(), false));
            }
            if let Some(old) = slot.take() {
                stop_later(old.agent);
            }
        }
        let agent = Arc::new(self.start(spec, deadline)?);
        agent.users.fetch_add(1, Ordering::SeqCst);
        *slot = Some(Slot { fingerprint, agent: agent.clone() });
        Ok((agent, true))
    }

    /// Remove an agent from the pool and stop it.
    fn discard(&self, key: &str, agent: &Arc<Agent>) {
        let cell = self.cells.lock().get(key).cloned();
        if let Some(cell) = cell {
            let mut slot = cell.lock();
            if slot.as_ref().is_some_and(|s| Arc::ptr_eq(&s.agent, agent)) {
                *slot = None;
            }
        }
        stop_later(agent.clone());
    }

    /// Stop the agent of `key`, when one runs.
    pub fn stop(&self, key: &str) {
        let cell = self.cells.lock().remove(key);
        if let Some(slot) = cell.and_then(|cell| cell.lock().take()) {
            stop_later(slot.agent);
        }
    }

    /// Start the thread that stops idle and dead agents, once.
    fn start_reaper(&self) {
        if self.reaper.swap(true, Ordering::SeqCst) {
            return;
        }
        let cells = Arc::downgrade(&self.cells);
        let idle_limit = self.idle_limit;
        let every = (idle_limit / 4).clamp(Duration::from_millis(10), Duration::from_secs(30));
        std::thread::spawn(move || reap(&cells, idle_limit, every));
    }
}

/// Stop the agents that nobody used for `idle_limit`, and the ones that died.
/// Ends when the pool is gone.
fn reap(cells: &Weak<Mutex<HashMap<String, Cell>>>, idle_limit: Duration, every: Duration) {
    loop {
        std::thread::sleep(every);
        let Some(cells) = cells.upgrade() else { return };
        let list: Vec<Cell> = cells.lock().values().cloned().collect();
        drop(cells);
        for cell in list {
            // A cell that is locked starts or hands out its agent right now.
            let Some(mut slot) = cell.try_lock() else { continue };
            let expired = slot.as_ref().is_some_and(|s| {
                let agent = &s.agent;
                agent.client.is_closed() || (agent.users.load(Ordering::SeqCst) == 0 && agent.last_used.lock().elapsed() >= idle_limit)
            });
            if expired && let Some(old) = slot.take() {
                drop(slot);
                drop(old);
            }
        }
    }
}

/// The program of a command: a path as it is (a leading `~` is the home
/// folder), a bare name searched like the other tools.
fn resolve_program(command: &str) -> Result<PathBuf, EnhanceError> {
    let command = command.trim();
    if command.contains('/') || command.contains('\\') {
        return Ok(crate::auth::expand_dir(command));
    }
    crate::detect::detect_command(command).ok_or_else(|| EnhanceError::NotConfigured(format!("the {command} command was not found")))
}

/// The `PATH` of the agent: the folder of its program first. An `npx` or
/// `node` script starts with `#!/usr/bin/env node`, and a Dock launch has a
/// short `PATH` without the folder of `node`.
fn path_with(program: &Path) -> Option<String> {
    let dir = program.parent().filter(|d| !d.as_os_str().is_empty())?;
    let mut dirs = vec![dir.to_path_buf()];
    dirs.extend(std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default());
    std::env::join_paths(dirs).ok().map(|p| p.to_string_lossy().into_owned())
}

/// Start the agent as a process, through the same helpers as the CLI tools
/// (Flatpak host, Windows shims).
pub(crate) fn spawn_process(spec: &LaunchSpec) -> Result<Transport, EnhanceError> {
    let program = resolve_program(&spec.command)?;
    let workdir = cli::workdir()?;
    #[cfg(not(windows))]
    let sandboxed = sayso_core::flatpak::app_id().is_some();
    #[cfg(windows)]
    let sandboxed = false;
    // In a Flatpak the `PATH` of this process is the one of the sandbox, not of the host.
    let path = (!sandboxed && !spec.env.iter().any(|(name, _)| name == "PATH")).then(|| path_with(&program)).flatten();
    let mut envs: Vec<(&str, &str)> = spec.env.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    if let Some(path) = &path {
        envs.push(("PATH", path));
    }
    let inv = Invocation {
        program: &program,
        args: spec.args.iter().map(Into::into).collect(),
        envs,
        cwd: workdir.path(),
        stdin: None,
        timeout: Duration::ZERO,
    };
    let mut child = cli::spawn(&inv, true, false)?;
    let (Some(writer), Some(reader)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        return Err(EnhanceError::Cli(format!("could not open the pipes of {}", spec.label())));
    };
    let stderr = Tail::read(child.stderr.take());
    Ok(Transport { reader: Box::new(reader), writer: Box::new(writer), child: Some(child), stderr, workdir })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(command: &str, env: &[(&str, &str)]) -> LaunchSpec {
        LaunchSpec {
            command: command.into(),
            args: vec!["acp".into()],
            env: env.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect(),
        }
    }

    #[test]
    fn the_fingerprint_follows_the_launch() {
        let a = spec("/bin/agent", &[("KEY", "1")]);
        assert_eq!(a.fingerprint(), spec("/bin/agent", &[("KEY", "1")]).fingerprint());
        assert_ne!(a.fingerprint(), spec("/bin/agent", &[("KEY", "2")]).fingerprint(), "a new secret value starts a new agent");
        assert_ne!(a.fingerprint(), spec("/bin/other", &[("KEY", "1")]).fingerprint());
        assert_eq!(a.label(), "agent");
    }

    #[test]
    #[cfg(unix)]
    fn a_missing_command_is_not_configured() {
        assert!(matches!(resolve_program("no-such-agent-here"), Err(EnhanceError::NotConfigured(m)) if m.contains("no-such-agent-here")));
        assert_eq!(resolve_program("/opt/x/agent").unwrap(), PathBuf::from("/opt/x/agent"));
        let path = path_with(Path::new("/opt/node/bin/npx")).unwrap();
        assert!(path.starts_with("/opt/node/bin:"), "{path}");
    }

    #[test]
    fn the_error_output_explains_a_stopped_agent() {
        let stderr = Tail::new();
        stderr.text.lock().push_str("starting\nError: unknown flag --acp\n");
        stderr.done.store(true, Ordering::SeqCst);
        let error = explain(EnhanceError::Cli("agent stopped: the agent closed its output".into()), "agent", &stderr);
        assert!(matches!(&error, EnhanceError::Cli(m) if m.ends_with("starting | Error: unknown flag --acp")), "{error:?}");
        stderr.text.lock().push_str("Please log in first\n");
        let error = explain(EnhanceError::Cli("agent stopped".into()), "agent", &stderr);
        assert!(matches!(&error, EnhanceError::NotConfigured(m) if m.starts_with("not signed in to agent")), "{error:?}");
        assert_eq!(explain(EnhanceError::Timeout(5), "agent", &Tail::finished()), EnhanceError::Timeout(5));
    }
}
