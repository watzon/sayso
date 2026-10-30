//! The client of the cleanup runtime (`sayso-cleanup`).
//!
//! One request at a time: a request goes out only after the reply to the one
//! before. llama.cpp cannot stop one decode from outside, so a request that
//! passes its deadline kills the process. The next request starts it again, and
//! a late reply cannot reach it. See `docs/cleanup-models.md`, "Runtime and protocol".

use sayso_core::cpu::{self, Level};
use parking_lot::Mutex;
use sayso_core::cleanup::protocol::{ErrorCode, PROTOCOL_VERSION, Reply, ReplyLine, Request, RequestLine, Stop};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

/// The start of the message of [`RuntimeError::UnsupportedCpu`]. The settings
/// look for it in the text of a failed test ([`is_unsupported_cpu`]).
const UNSUPPORTED_CPU: &str = "the processor of this computer cannot run the cleanup runtime";

/// The name of the runtime program, without a build suffix or `.exe`.
const PROGRAM: &str = "sayso-cleanup";

/// A load can take this long. The first start on a Mac compiles the Metal
/// shaders, which took about 4 s in the spike.
pub const LOAD_LIMIT: Duration = Duration::from_secs(15);

/// The runtime stops after this long with no request, to free its memory.
pub const IDLE_LIMIT: Duration = Duration::from_secs(10 * 60);

/// A request with no deadline of its own (`hello`, `count`, `unload`) gets this.
const SHORT_LIMIT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeError {
    #[error("the cleanup runtime is not installed: {0}")]
    NotInstalled(String),
    #[error("the cleanup runtime did not answer in time")]
    Timeout,
    #[error("the cleanup runtime stopped: {0}")]
    Stopped(String),
    #[error("the cleanup runtime failed: {code:?}: {message}")]
    Failed { code: ErrorCode, message: String },
    #[error("the cleanup runtime sent an answer that does not fit the request")]
    Unexpected,
    #[error("the cleanup model cannot load now: it is being deleted, or AI is off")]
    Closed,
    /// The build lacks instructions of this processor: Sayso refused to start
    /// it, or it stopped with an illegal instruction.
    #[error("{UNSUPPORTED_CPU}: {0}")]
    UnsupportedCpu(String),
}

/// True when `message` (the text of an error of a cleanup) is a
/// [`RuntimeError::UnsupportedCpu`].
pub fn is_unsupported_cpu(message: &str) -> bool {
    message.contains(UNSUPPORTED_CPU)
}

/// A completion of the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub text: String,
    pub stop: Stop,
    pub tokens_in: u32,
    pub tokens_out: u32,
}

struct Process {
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<ReplyLine>,
    /// The model id and the file that are loaded.
    loaded: Option<(String, PathBuf)>,
}

impl Process {
    fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct Runtime {
    path: PathBuf,
    /// The process, and the gate of loads. Every load and every stop takes
    /// this lock, so a gate change made under it holds for every load after it.
    process: Mutex<Option<Process>>,
    gate: Mutex<Gate>,
    /// False while no cleanup model is in use (AI is off): nothing loads.
    enabled: std::sync::atomic::AtomicBool,
    next_id: AtomicU64,
    last_use: Mutex<Instant>,
    idle_watch: Mutex<bool>,
}

/// What may load. Read only while the process lock is held.
#[derive(Default)]
struct Gate {
    /// Models that are being deleted.
    blocked: std::collections::HashSet<String>,
}

impl Runtime {
    pub fn new(path: PathBuf) -> Arc<Self> {
        Arc::new(Runtime {
            path,
            process: Mutex::new(None),
            gate: Mutex::new(Gate::default()),
            enabled: std::sync::atomic::AtomicBool::new(true),
            next_id: AtomicU64::new(1),
            last_use: Mutex::new(Instant::now()),
            idle_watch: Mutex::new(false),
        })
    }

    /// `SAYSO_CLEANUP_PATH`, else the best build beside the app that this
    /// processor runs ([`cpu::best_in`]), else the development build in
    /// `native/cleanup/target/release`. A package with one build (before the
    /// x86_64 builds, or not x86_64) gives `sayso-cleanup`.
    pub fn default_path() -> PathBuf {
        if let Some(path) = std::env::var_os("SAYSO_CLEANUP_PATH").filter(|p| !p.is_empty()) {
            return PathBuf::from(path);
        }
        if let Some(path) = std::env::current_exe().ok().and_then(|exe| cpu::best_in(exe.parent()?, PROGRAM)) {
            return path;
        }
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../native/cleanup/target/release").join(Level::Avx2.file_name(PROGRAM))
    }

    /// True while the runtime process runs.
    pub fn is_running(&self) -> bool {
        self.process.lock().is_some()
    }

    /// Load `model_id` from `path` unless it is loaded. Gives the load time, or
    /// zero when it was loaded. `wait_until` bounds the wait for an earlier
    /// request (a load at the start of the dictation) to end.
    pub fn ensure_loaded(self: &Arc<Self>, model_id: &str, path: &Path, n_ctx: u32, wait_until: Option<Instant>) -> Result<u64, RuntimeError> {
        let mut guard = self.lock(wait_until)?;
        self.load_locked(&mut guard, model_id, path, n_ctx)
    }

    /// Run `f` with the process locked, with `model_id` loaded. A whole
    /// cleanup (counts and completions of all parts) runs in one call, so no
    /// other request can come between its requests.
    pub fn with_model<T>(
        self: &Arc<Self>,
        model_id: &str,
        path: &Path,
        n_ctx: u32,
        wait_until: Option<Instant>,
        f: impl FnOnce(&mut Session<'_>) -> Result<T, RuntimeError>,
    ) -> Result<(T, u64), RuntimeError> {
        let mut guard = self.lock(wait_until)?;
        let load_ms = self.load_locked(&mut guard, model_id, path, n_ctx)?;
        let mut session = Session { runtime: self, process: &mut guard, model_id: model_id.to_string(), path: path.to_path_buf(), n_ctx };
        let out = f(&mut session)?;
        Ok((out, load_ms))
    }

    /// Stop the process: before a model file is deleted, and when AI is off.
    pub fn stop(&self) {
        if let Some(process) = self.process.lock().take() {
            process.kill();
        }
    }

    /// Let models load, or not. Call it on any thread; it does not wait. A
    /// load that starts after `set_enabled(false)` is refused, also one that
    /// waits for the lock now. Stop the process after it with [`Self::stop`].
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::SeqCst);
    }

    /// Stop the process and let `model_id` not load until [`Self::unblock`],
    /// before its files are deleted.
    pub fn block(&self, model_id: &str) {
        let mut process = self.process.lock();
        self.gate.lock().blocked.insert(model_id.to_string());
        if let Some(p) = process.take() {
            p.kill();
        }
    }

    pub fn unblock(&self, model_id: &str) {
        let _process = self.process.lock();
        self.gate.lock().blocked.remove(model_id);
    }

    fn lock(self: &Arc<Self>, wait_until: Option<Instant>) -> Result<parking_lot::MutexGuard<'_, Option<Process>>, RuntimeError> {
        let guard = match wait_until {
            Some(until) => self.process.try_lock_until(until).ok_or(RuntimeError::Timeout)?,
            None => self.process.lock(),
        };
        *self.last_use.lock() = Instant::now();
        self.watch_idle();
        Ok(guard)
    }

    fn load_locked(&self, process: &mut Option<Process>, model_id: &str, path: &Path, n_ctx: u32) -> Result<u64, RuntimeError> {
        {
            let gate = self.gate.lock();
            if !self.enabled.load(Ordering::SeqCst) || gate.blocked.contains(model_id) {
                return Err(RuntimeError::Closed);
            }
        }
        if process.as_ref().and_then(|p| p.loaded.as_ref()).is_some_and(|(id, p)| id == model_id && p == path) {
            return Ok(0);
        }
        // The runtime unloads the old model before it loads the new one, so
        // after a failed load no model is loaded.
        if let Some(p) = process.as_mut() {
            p.loaded = None;
        }
        let request = Request::Load { model_id: model_id.into(), path: path.to_string_lossy().into_owned(), n_ctx };
        match self.send(process, request, Some(Instant::now() + LOAD_LIMIT))? {
            Reply::Loaded { load_ms, .. } => {
                if let Some(p) = process.as_mut() {
                    p.loaded = Some((model_id.to_string(), path.to_path_buf()));
                }
                Ok(load_ms)
            }
            _ => Err(RuntimeError::Unexpected),
        }
    }

    /// Send one request and wait for its reply. Starts the process when it
    /// does not run. Kills it when the deadline passes or the reply is bad.
    fn send(&self, process: &mut Option<Process>, request: Request, deadline: Option<Instant>) -> Result<Reply, RuntimeError> {
        enum Outcome {
            Reply(Reply),
            Timeout,
            Dead,
        }
        if process.is_none() {
            *process = Some(self.start()?);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
        let line = serde_json::to_string(&RequestLine::new(id.clone(), request)).expect("a request always serializes");
        let outcome = {
            let p = process.as_mut().expect("started above");
            if writeln!(p.stdin, "{line}").and_then(|()| p.stdin.flush()).is_err() {
                Outcome::Dead
            } else {
                loop {
                    let next = match deadline {
                        Some(deadline) => p.replies.recv_timeout(deadline.saturating_duration_since(Instant::now())),
                        None => p.replies.recv().map_err(|_| RecvTimeoutError::Disconnected),
                    };
                    match next {
                        Ok(reply) if reply.id == id => break Outcome::Reply(reply.reply),
                        // A reply to an earlier request, or a line with no id: not ours.
                        Ok(_) => continue,
                        Err(RecvTimeoutError::Timeout) => break Outcome::Timeout,
                        Err(RecvTimeoutError::Disconnected) => break Outcome::Dead,
                    }
                }
            }
        };
        match outcome {
            Outcome::Reply(Reply::Error { code, message }) => {
                if matches!(code, ErrorCode::WrongModel | ErrorCode::NotLoaded)
                    && let Some(p) = process.as_mut()
                {
                    p.loaded = None;
                }
                Err(RuntimeError::Failed { code, message })
            }
            Outcome::Reply(reply) => Ok(reply),
            Outcome::Timeout => {
                if let Some(process) = process.take() {
                    process.kill();
                }
                Err(RuntimeError::Timeout)
            }
            Outcome::Dead => Err(self.stopped(process.take())),
        }
    }

    fn start(&self) -> Result<Process, RuntimeError> {
        // A build of the packages that this processor cannot run would stop
        // at the first load. A path from SAYSO_CLEANUP_PATH can be any build,
        // so it starts.
        let from_env = std::env::var_os("SAYSO_CLEANUP_PATH").is_some_and(|p| !p.is_empty());
        if let Some(build) = Level::of_file(PROGRAM, &self.path).filter(|b| !from_env && !b.runs_here()) {
            return Err(RuntimeError::UnsupportedCpu(format!("{} needs {}", self.path.display(), build.needs())));
        }
        let mut child = Command::new(&self.path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| RuntimeError::NotInstalled(format!("{}: {e}", self.path.display())))?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let (tx, replies) = channel();
        std::thread::Builder::new()
            .name("cleanup-runtime-out".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    match serde_json::from_str::<ReplyLine>(&line) {
                        Ok(reply) if reply.v == PROTOCOL_VERSION => {
                            if tx.send(reply).is_err() {
                                break;
                            }
                        }
                        Ok(reply) => log::warn!("cleanup runtime speaks protocol {}, not {PROTOCOL_VERSION}", reply.v),
                        Err(e) => log::warn!("cleanup runtime sent a line that is not a reply: {e}"),
                    }
                }
            })
            .map_err(|e| RuntimeError::Stopped(e.to_string()))?;
        std::thread::Builder::new()
            .name("cleanup-runtime-err".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    log::debug!(target: "sayso_cleanup", "{line}");
                }
            })
            .map_err(|e| RuntimeError::Stopped(e.to_string()))?;
        log::info!("started the cleanup runtime {}", self.path.display());
        Ok(Process { child, stdin, replies, loaded: None })
    }

    /// One thread per runtime stops the process after [`IDLE_LIMIT`] with no request.
    fn watch_idle(self: &Arc<Self>) {
        let mut started = self.idle_watch.lock();
        if *started {
            return;
        }
        *started = true;
        let weak: Weak<Runtime> = Arc::downgrade(self);
        let _ = std::thread::Builder::new().name("cleanup-runtime-idle".into()).spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(30));
                let Some(runtime) = weak.upgrade() else { break };
                let idle = runtime.last_use.lock().elapsed() >= IDLE_LIMIT;
                // `try_lock`: a request that runs now is a use.
                if idle && let Some(mut guard) = runtime.process.try_lock() && let Some(process) = guard.take() {
                    log::info!("stopped the cleanup runtime after {} minutes with no use", IDLE_LIMIT.as_secs() / 60);
                    process.kill();
                }
            }
        });
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(process) = self.process.get_mut().take() {
            process.kill();
        }
    }
}

impl Runtime {
    /// The error for a process that ended. An illegal instruction is a build
    /// that this processor cannot run.
    fn stopped(&self, process: Option<Process>) -> RuntimeError {
        let Some(mut process) = process else { return RuntimeError::Stopped("no process".into()) };
        let status = match process.child.wait() {
            Ok(status) => status,
            Err(e) => return RuntimeError::Stopped(e.to_string()),
        };
        if cpu::illegal_instruction(&status) {
            let needs = Level::of_file(PROGRAM, &self.path).map_or_else(|| "an instruction that it lacks".to_string(), |b| b.needs().to_string());
            log::error!("the cleanup runtime {} stopped with an illegal instruction ({status})", self.path.display());
            return RuntimeError::UnsupportedCpu(format!("{} needs {needs} ({status})", self.path.display()));
        }
        RuntimeError::Stopped(format!("the process ended ({status})"))
    }
}

/// The requests of one cleanup, with the model loaded and the process locked.
pub struct Session<'a> {
    runtime: &'a Runtime,
    process: &'a mut Option<Process>,
    model_id: String,
    path: PathBuf,
    n_ctx: u32,
}

impl Session<'_> {
    pub fn count(&mut self, text: &str) -> Result<u32, RuntimeError> {
        self.count_until(text, None)
    }

    /// Count, and wait no longer than `until` (and no longer than a short limit).
    pub fn count_until(&mut self, text: &str, until: Option<Instant>) -> Result<u32, RuntimeError> {
        let short = Instant::now() + SHORT_LIMIT;
        let deadline = until.map_or(short, |until| until.min(short));
        let request = Request::Count { model_id: self.model_id.clone(), text: text.into() };
        match self.send(request, Some(deadline))? {
            Reply::Count { tokens } => Ok(tokens),
            _ => Err(RuntimeError::Unexpected),
        }
    }

    /// `deadline` None waits as long as the answer takes.
    pub fn complete(&mut self, prompt: &str, end: &[String], max_tokens: u32, deadline: Option<Instant>) -> Result<Completion, RuntimeError> {
        let request = Request::Complete { model_id: self.model_id.clone(), prompt: prompt.into(), end: end.to_vec(), max_tokens };
        match self.send(request, deadline)? {
            Reply::Completion { text, stop, tokens_in, tokens_out, .. } => Ok(Completion { text, stop, tokens_in, tokens_out }),
            _ => Err(RuntimeError::Unexpected),
        }
    }

    /// Send a request for the model of the session. When the runtime has
    /// another model or none (a restart after a timeout, a failed load), load
    /// the model again and send the request one more time.
    fn send(&mut self, request: Request, deadline: Option<Instant>) -> Result<Reply, RuntimeError> {
        match self.runtime.send(self.process, request.clone(), deadline) {
            Err(RuntimeError::Failed { code: ErrorCode::WrongModel | ErrorCode::NotLoaded, .. }) => {
                self.runtime.load_locked(self.process, &self.model_id, &self.path, self.n_ctx)?;
                self.runtime.send(self.process, request, deadline)
            }
            other => other,
        }
    }
}
