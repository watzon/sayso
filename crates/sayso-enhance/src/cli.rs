//! Run a command line tool with a hard deadline. Shared by the Claude and
//! Codex providers.

use sayso_core::enhance::EnhanceError;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;

/// One tool run.
pub(crate) struct Invocation<'a> {
    pub program: &'a Path,
    pub args: Vec<OsString>,
    /// Extra environment variables. The rest of the environment is inherited,
    /// because the tools need `HOME` to find their own login.
    pub envs: Vec<(&'a str, &'a str)>,
    /// A directory with nothing in it. The tool must not see a project.
    pub cwd: &'a Path,
    /// Text for standard input. None gives the tool an empty input.
    pub stdin: Option<String>,
    pub timeout: Duration,
}

pub(crate) struct Output {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// Read a pipe on a helper thread, so a full pipe never blocks the tool. The
/// thread sends each chunk as it arrives, so text read so far is never lost.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> Receiver<Vec<u8>> {
    let (tx, rx) = channel();
    if let Some(mut pipe) = pipe {
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
    }
    rx
}

/// Collect what a drained pipe holds. Stops at the end of the pipe, or when no
/// data arrives for a short while: a helper process of the tool may keep the
/// pipe open after the tool itself has exited.
fn collect(rx: Receiver<Vec<u8>>) -> String {
    let mut bytes = Vec::new();
    while let Ok(chunk) = rx.recv_timeout(Duration::from_millis(300)) {
        bytes.extend(chunk);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Start the tool. `piped_stdin` gives it a pipe, else it reads an empty input.
/// `quiet_stderr` sends its error output to the null device, else a pipe.
fn spawn(inv: &Invocation, piped_stdin: bool, quiet_stderr: bool) -> Result<Child, EnhanceError> {
    let mut attempts = 0;
    loop {
        let mut command = new_command(inv.program);
        command
            .args(&inv.args)
            .envs(inv.envs.iter().copied())
            .current_dir(inv.cwd)
            .stdin(if piped_stdin { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(if quiet_stderr { Stdio::null() } else { Stdio::piped() });
        match command.spawn() {
            Ok(child) => return Ok(child),
            // ETXTBSY (26): another thread still holds a freshly written script open for
            // writing. It clears within milliseconds, so try again a few times.
            Err(e) if e.raw_os_error() == Some(26) && attempts < 20 => {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                let program = inv.program.display();
                return Err(match e.kind() {
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => {
                        EnhanceError::NotConfigured(format!("could not start {program}: {e}"))
                    }
                    _ => EnhanceError::Cli(format!("could not start {program}: {e}")),
                });
            }
        }
    }
}

#[cfg(not(windows))]
fn new_command(program: &Path) -> Command {
    Command::new(program)
}

/// No console window, and npm `.cmd` shims run through node (see `win_shim`).
#[cfg(windows)]
fn new_command(program: &Path) -> Command {
    crate::win_shim::command(program)
}

/// Run the tool and collect its output. When the deadline passes, the tool is
/// killed and the result is [`EnhanceError::Timeout`].
pub(crate) fn run(inv: &Invocation) -> Result<Output, EnhanceError> {
    let mut child = spawn(inv, inv.stdin.is_some(), false)?;
    if let (Some(mut pipe), Some(text)) = (child.stdin.take(), inv.stdin.clone()) {
        // A thread, because the tool may answer before it reads all input.
        std::thread::spawn(move || {
            let _ = pipe.write_all(text.as_bytes());
        });
    }
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());

    let status = match child.wait_timeout(inv.timeout) {
        Ok(Some(status)) => status,
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(EnhanceError::Timeout(inv.timeout.as_millis() as u64));
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(EnhanceError::Cli(format!("could not wait for {}: {e}", inv.program.display())));
        }
    };
    Ok(Output { status, stdout: collect(stdout), stderr: collect(stderr) })
}

/// True when an error message from a tool says the user is not signed in.
pub(crate) fn looks_like_login_problem(message: &str) -> bool {
    let lower = message.to_lowercase();
    ["/login", "not logged in", "log in", "sign in", "signed in", "api key", "unauthorized", "authenticat"]
        .iter()
        .any(|needle| lower.contains(needle))
}

/// A running tool that talks in lines: write a line, read lines until one
/// matches. Every read stops at the deadline. Dropping the session kills the
/// tool and waits for it, so no path leaves a process behind.
pub(crate) struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    deadline: Instant,
    timeout: Duration,
}

impl Session {
    /// Start the tool. The deadline is `inv.timeout` from now. `inv.stdin` is
    /// ignored: use [`Session::send`]. Error output of the tool is discarded.
    pub fn start(inv: &Invocation) -> Result<Self, EnhanceError> {
        let mut child = spawn(inv, true, true)?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let (tx, lines) = channel();
        if let Some(stdout) = stdout {
            std::thread::spawn(move || {
                for raw in BufReader::new(stdout).split(b'\n') {
                    let Ok(raw) = raw else { break };
                    if tx.send(String::from_utf8_lossy(&raw).into_owned()).is_err() {
                        break;
                    }
                }
            });
        }
        Ok(Self { child, stdin, lines, deadline: Instant::now() + inv.timeout, timeout: inv.timeout })
    }

    /// Write one line to the tool. The input stays open afterwards.
    pub fn send(&mut self, line: &str) -> Result<(), EnhanceError> {
        let pipe = self.stdin.as_mut().ok_or_else(|| EnhanceError::Cli("the tool has no input".into()))?;
        pipe.write_all(line.as_bytes())
            .and_then(|()| pipe.write_all(b"\n"))
            .and_then(|()| pipe.flush())
            .map_err(|e| EnhanceError::Cli(format!("could not write to the tool: {e}")))
    }

    /// Read lines until `pick` returns a value. Lines it rejects are skipped.
    pub fn read_until<T>(&mut self, mut pick: impl FnMut(&str) -> Option<T>) -> Result<T, EnhanceError> {
        loop {
            let left = self.deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    if let Some(found) = pick(&line) {
                        return Ok(found);
                    }
                }
                Err(RecvTimeoutError::Timeout) => return Err(EnhanceError::Timeout(self.timeout.as_millis() as u64)),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(EnhanceError::Cli("the tool closed its output before it answered".into()));
                }
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The last few lines of tool output, for an error message.
pub(crate) fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().rev().take(4).collect();
    let joined = lines.into_iter().rev().collect::<Vec<_>>().join(" | ");
    joined.chars().take(400).collect()
}
