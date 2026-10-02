//! `EngineClient`: spawns the SaysoEngine sidecar and implements `SttBackend`.
//!
//! Threads, per client:
//! - **supervisor**: reads the sidecar's stdout. When the pipe closes it reports
//!   `Crashed`, restarts the sidecar with backoff, and starts a recovery thread.
//! - **writer** (one per sidecar process): owns the sidecar's stdin and drains an
//!   unbounded queue, so callers (including the audio thread) never wait on the pipe.
//! - **stderr** (one per sidecar process): forwards the sidecar's stderr to `log`.
//! - **recovery** (after a restart): says hello, refreshes statuses, reloads the
//!   models that were loaded before the crash, then reports `Restarted`.

use crate::protocol::{self, Message, pcm_base64, request_line};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded, unbounded};
use parking_lot::Mutex;
use sayso_core::models::{self, ModelId, ModelInfo};
use sayso_core::stt::{EngineEvent, ModelStatus, SessionOptions, Transcript, encode_wav, to_pcm16};
use sayso_platform::{PlatformError, Result, SttBackend};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// Audio frames that may wait in the writer queue before new frames are dropped.
/// 500 frames of 20 ms are 10 seconds. The live preview can lose audio. The final
/// pass reads the full recording from a file.
const MAX_QUEUED_AUDIO: usize = 500;

/// Timing knobs. The defaults suit the real sidecar. Tests shorten them.
#[derive(Debug, Clone)]
pub struct Options {
    /// Wait for quick requests: hello, list, delete, stream start.
    pub request_timeout: Duration,
    /// Wait for `load` (first load compiles for the Neural Engine, 60 s or more
    /// for Whisper) and for `transcribe`.
    pub long_timeout: Duration,
    /// First restart delay after a crash. It doubles each time.
    pub backoff_initial: Duration,
    /// Longest restart delay.
    pub backoff_max: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            request_timeout: Duration::from_secs(15),
            long_timeout: Duration::from_secs(600),
            backoff_initial: Duration::from_millis(500),
            backoff_max: Duration::from_secs(5),
        }
    }
}

fn failed(message: impl Into<String>) -> PlatformError {
    PlatformError::Failed(message.into())
}

/// What the writer thread sends to the sidecar. Audio is encoded on the writer
/// thread, so `push_audio` only converts samples and queues them.
enum Outgoing {
    Line(String),
    Audio { session: u64, pcm: Vec<i16> },
}

/// What to do with the reply of a request.
enum Pending {
    /// A caller blocks on this channel.
    Waiter(Sender<std::result::Result<Message, String>>),
    /// Nobody waits. A failure is logged and, for a model, shown as `Failed`.
    Detached {
        what: &'static str,
        model: Option<ModelId>,
    },
}

/// A live sidecar process, as seen by senders.
struct Conn {
    tx: Sender<Outgoing>,
    audio_queued: Arc<AtomicUsize>,
}

/// Connection and pending requests share one lock, so a request is either
/// registered before a crash drains the map or refused after it.
struct Core {
    conn: Option<Conn>,
    pending: HashMap<String, Pending>,
}

struct Inner {
    engine_path: PathBuf,
    models_dir: PathBuf,
    cache_dir: PathBuf,
    options: Options,
    core: Mutex<Core>,
    subscribers: Mutex<Vec<Sender<EngineEvent>>>,
    statuses: Mutex<HashMap<ModelId, ModelStatus>>,
    /// Models that a caller loaded on purpose. The recovery thread loads them again.
    desired_loaded: Mutex<Vec<ModelId>>,
    child: Mutex<Option<Child>>,
    next_id: AtomicU64,
    next_file: AtomicU64,
    generation: AtomicU64,
    restart_attempts: AtomicU32,
    shutting_down: AtomicBool,
    supervisor_alive: AtomicBool,
    dropped_audio: AtomicU64,
}

/// Stops the sidecar when the last `EngineClient` handle is dropped.
struct Guard(Arc<Inner>);

impl Drop for Guard {
    fn drop(&mut self) {
        let inner = &self.0;
        inner.shutting_down.store(true, Ordering::SeqCst);
        {
            let mut core = inner.core.lock();
            if let Some(conn) = core.conn.take() {
                // Queued lines still reach the sidecar. When the sender drops, the writer
                // closes stdin, and the sidecar exits on end of input.
                let _ = conn
                    .tx
                    .send(Outgoing::Line(request_line("shutdown", None, json!({}))));
            }
        }
        let deadline = Instant::now() + Duration::from_millis(1500);
        while inner.supervisor_alive.load(Ordering::SeqCst) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        inner.kill_child();
    }
}

/// The speech engine as seen by the app. Cheap to clone. All clones share one sidecar.
#[derive(Clone)]
pub struct EngineClient {
    inner: Arc<Inner>,
    _guard: Arc<Guard>,
}

impl EngineClient {
    /// Start the sidecar and wait until it answers. `models_dir` is passed to the
    /// sidecar as `SAYSO_MODELS_DIR`. `cache_dir` holds the temporary WAV files.
    pub fn spawn(engine_path: PathBuf, models_dir: PathBuf, cache_dir: PathBuf) -> Result<Self> {
        Self::spawn_with(engine_path, models_dir, cache_dir, Options::default())
    }

    /// Like [`EngineClient::spawn`], with explicit timing options.
    pub fn spawn_with(
        engine_path: PathBuf,
        models_dir: PathBuf,
        cache_dir: PathBuf,
        options: Options,
    ) -> Result<Self> {
        std::fs::create_dir_all(&models_dir)
            .and_then(|()| std::fs::create_dir_all(&cache_dir))
            .map_err(|e| failed(format!("cannot create engine directories: {e}")))?;
        let inner = Arc::new(Inner {
            engine_path,
            models_dir,
            cache_dir,
            options,
            core: Mutex::new(Core {
                conn: None,
                pending: HashMap::new(),
            }),
            subscribers: Mutex::new(Vec::new()),
            statuses: Mutex::new(HashMap::new()),
            desired_loaded: Mutex::new(Vec::new()),
            child: Mutex::new(None),
            next_id: AtomicU64::new(0),
            next_file: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            restart_attempts: AtomicU32::new(0),
            shutting_down: AtomicBool::new(false),
            supervisor_alive: AtomicBool::new(true),
            dropped_audio: AtomicU64::new(0),
        });
        let stdout = inner.launch().map_err(|e| {
            failed(format!(
                "cannot start engine {}: {e}",
                inner.engine_path.display()
            ))
        })?;
        {
            let inner = Arc::clone(&inner);
            thread::Builder::new()
                .name("sayso-engine-supervisor".into())
                .spawn(move || supervise(&inner, stdout))
                .map_err(|e| failed(format!("cannot start engine thread: {e}")))?;
        }
        let client = EngineClient {
            inner: Arc::clone(&inner),
            _guard: Arc::new(Guard(inner)),
        };
        client
            .inner
            .call("hello", json!({}), client.inner.options.request_timeout)?;
        client.inner.refresh_statuses()?;
        Ok(client)
    }

    /// Find the sidecar binary. In order: env `SAYSO_ENGINE_PATH`, `SaysoEngine` next to
    /// the running executable (app bundle), then the development build in this workspace.
    /// If none exists, the development path comes back so the spawn error names it.
    pub fn default_engine_path() -> PathBuf {
        if let Some(path) = std::env::var_os("SAYSO_ENGINE_PATH").filter(|p| !p.is_empty()) {
            return PathBuf::from(path);
        }
        if let Some(beside) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("SaysoEngine")))
            .filter(|path| path.exists())
        {
            return beside;
        }
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../native/macos/SaysoEngine/.build/release/SaysoEngine")
    }

    /// Number of live preview audio frames dropped because the sidecar fell behind.
    pub fn dropped_audio_frames(&self) -> u64 {
        self.inner.dropped_audio.load(Ordering::Relaxed)
    }
}

impl SttBackend for EngineClient {
    fn catalog(&self) -> Vec<ModelInfo> {
        models::catalog()
    }

    fn status(&self, model: &ModelId) -> ModelStatus {
        self.inner
            .statuses
            .lock()
            .get(model)
            .cloned()
            .unwrap_or(ModelStatus::NotDownloaded)
    }

    fn download(&self, model: &ModelId) -> Result<()> {
        let info = find_model(model)?;
        let fields = json!({
            "model": info.id,
            "engine": info.engine,
            "size_bytes": info.size_bytes,
        });
        self.inner.send(
            "download",
            fields,
            Pending::Detached {
                what: "download",
                model: Some(model.clone()),
            },
        )?;
        Ok(())
    }

    fn delete(&self, model: &ModelId) -> Result<()> {
        let info = find_model(model)?;
        let fields = json!({"model": info.id, "engine": info.engine});
        self.inner
            .call("delete", fields, self.inner.options.request_timeout)?;
        self.inner.desired_loaded.lock().retain(|m| m != model);
        Ok(())
    }

    fn load(&self, model: &ModelId) -> Result<()> {
        self.inner.load_model(model)?;
        let mut desired = self.inner.desired_loaded.lock();
        if !desired.contains(model) {
            desired.push(model.clone());
        }
        Ok(())
    }

    fn start_stream(&self, session: u64, options: &SessionOptions) -> Result<()> {
        let Some(preview) = &options.preview_model else {
            return Ok(()); // Live preview is off.
        };
        let fields = json!({
            "session": session,
            "model": preview,
            "language": language_for(preview, &options.language),
        });
        self.inner
            .call("stream_start", fields, self.inner.options.request_timeout)?;
        Ok(())
    }

    fn push_audio(&self, session: u64, samples: &[f32]) -> Result<()> {
        let pcm = to_pcm16(samples);
        let core = self.inner.core.lock();
        let Some(conn) = core.conn.as_ref() else {
            return Err(failed("engine is not running"));
        };
        if conn.audio_queued.load(Ordering::Relaxed) >= MAX_QUEUED_AUDIO {
            self.inner.dropped_audio.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        conn.audio_queued.fetch_add(1, Ordering::Relaxed);
        conn.tx
            .send(Outgoing::Audio { session, pcm })
            .map_err(|_| failed("engine is not running"))
    }

    fn end_stream(&self, session: u64) {
        let sent = self.inner.send(
            "stream_end",
            json!({"session": session}),
            Pending::Detached {
                what: "stream_end",
                model: None,
            },
        );
        if let Err(e) = sent {
            log::warn!("cannot end stream {session}: {e}");
        }
    }

    fn transcribe(&self, samples: &[f32], options: &SessionOptions) -> Result<Transcript> {
        let n = self.inner.next_file.fetch_add(1, Ordering::Relaxed);
        let path = self
            .inner
            .cache_dir
            .join(format!("transcribe-{}-{n}.wav", std::process::id()));
        std::fs::write(&path, encode_wav(&to_pcm16(samples)))
            .map_err(|e| failed(format!("cannot write {}: {e}", path.display())))?;
        let _cleanup = RemoveOnDrop(path.clone());
        let fields = json!({
            "model": options.final_model,
            "path": path.to_string_lossy(),
            "language": language_for(&options.final_model, &options.language),
            "vocabulary": options.vocabulary,
        });
        let reply = self
            .inner
            .call("transcribe", fields, self.inner.options.long_timeout)?;
        Ok(Transcript {
            text: reply.str("text").unwrap_or_default().to_string(),
            model: options.final_model.clone(),
            elapsed_ms: reply.u64("elapsed_ms").unwrap_or(0),
        })
    }

    fn subscribe(&self) -> Receiver<EngineEvent> {
        let (tx, rx) = unbounded();
        self.inner.subscribers.lock().push(tx);
        rx
    }
}

fn find_model(model: &ModelId) -> Result<ModelInfo> {
    models::find(model).ok_or_else(|| failed(format!("unknown model {model}")))
}

/// The language to send for a model: the setting, or the nearest value the model
/// supports. A model that is not in the catalog gets the setting as it is.
fn language_for(model: &ModelId, setting: &str) -> String {
    models::find(model).map_or_else(|| setting.to_string(), |info| info.language_for(setting))
}

/// Deletes a temporary file when it goes out of scope.
struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// ---------------------------------------------------------------------------
// Requests and replies
// ---------------------------------------------------------------------------

impl Inner {
    /// Queue a request. The reply goes to `pending`. Fails at once when the sidecar is down.
    fn send(&self, kind: &str, fields: Value, pending: Pending) -> Result<String> {
        let id = format!("r{}", self.next_id.fetch_add(1, Ordering::Relaxed) + 1);
        let line = request_line(kind, Some(&id), fields);
        let mut core = self.core.lock();
        let tx = match &core.conn {
            Some(conn) => conn.tx.clone(),
            None => return Err(failed("engine is not running")),
        };
        core.pending.insert(id.clone(), pending);
        if tx.send(Outgoing::Line(line)).is_err() {
            core.pending.remove(&id);
            return Err(failed("engine is not running"));
        }
        Ok(id)
    }

    /// Send a request and wait for its reply.
    fn call(&self, kind: &str, fields: Value, timeout: Duration) -> Result<Message> {
        let (tx, rx) = bounded(1);
        let id = self.send(kind, fields, Pending::Waiter(tx))?;
        match rx.recv_timeout(timeout) {
            Ok(Ok(message)) => Ok(message),
            Ok(Err(message)) => Err(failed(message)),
            Err(RecvTimeoutError::Timeout) => {
                self.core.lock().pending.remove(&id);
                Err(failed(format!(
                    "engine did not answer {kind} within {timeout:?}"
                )))
            }
            Err(RecvTimeoutError::Disconnected) => Err(failed("engine stopped")),
        }
    }

    fn load_model(&self, model: &ModelId) -> Result<()> {
        let info = find_model(model)?;
        let fields = json!({"model": info.id, "engine": info.engine});
        self.call("load", fields, self.options.long_timeout)?;
        Ok(())
    }

    /// Ask the sidecar for the disk state of every catalog model. No network.
    fn refresh_statuses(&self) -> Result<()> {
        let list: Vec<Value> = models::catalog()
            .iter()
            .map(|m| json!({"id": m.id, "engine": m.engine}))
            .collect();
        self.call(
            "list_models",
            json!({"models": list}),
            self.options.request_timeout,
        )?;
        Ok(())
    }

    /// Route one message from the sidecar.
    fn handle(&self, message: Message) {
        match message.kind.as_str() {
            "model_state" | "download_progress" => {
                if let Some((model, status)) = protocol::model_status(&message) {
                    self.set_status(model, status);
                }
            }
            "partial" => {
                if let Some(session) = message.u64("session") {
                    self.emit(EngineEvent::Partial {
                        session,
                        committed: message.str("committed").unwrap_or_default().to_string(),
                        tentative: message.str("tentative").unwrap_or_default().to_string(),
                    });
                }
            }
            "log" => {
                let level = message.str("level").unwrap_or("info").to_string();
                let text = message.str("message").unwrap_or_default().to_string();
                match level.as_str() {
                    "error" => log::error!(target: "sayso_engine", "{text}"),
                    "warn" => log::warn!(target: "sayso_engine", "{text}"),
                    "debug" => log::debug!(target: "sayso_engine", "{text}"),
                    _ => log::info!(target: "sayso_engine", "{text}"),
                }
                self.emit(EngineEvent::Log {
                    level,
                    message: text,
                });
            }
            _ if message.is_reply() => self.complete(message),
            other => log::debug!("ignoring engine message of type {other}"),
        }
    }

    /// Finish the request that a reply belongs to.
    fn complete(&self, message: Message) {
        let is_error = message.kind == "error";
        let text = message
            .str("message")
            .unwrap_or("unknown engine error")
            .to_string();
        let Some(pending) = message
            .id
            .as_ref()
            .and_then(|id| self.core.lock().pending.remove(id))
        else {
            if is_error {
                log::warn!("engine error: {text}");
            }
            return;
        };
        match pending {
            Pending::Waiter(tx) => {
                let _ = tx.send(if is_error { Err(text) } else { Ok(message) });
            }
            Pending::Detached { what, model } => {
                if is_error {
                    log::warn!("engine {what} failed: {text}");
                    if let Some(model) = model {
                        self.set_status(model, ModelStatus::Failed { message: text });
                    }
                }
            }
        }
    }

    /// Update the status cache. Subscribers hear about real changes only.
    fn set_status(&self, model: ModelId, status: ModelStatus) {
        let changed = {
            let mut statuses = self.statuses.lock();
            if statuses.get(&model) == Some(&status) {
                false
            } else {
                statuses.insert(model.clone(), status.clone());
                true
            }
        };
        if changed {
            self.emit(EngineEvent::ModelStatus { model, status });
        }
    }

    /// Send an event to every subscriber. Subscribers that dropped their receiver go away.
    fn emit(&self, event: EngineEvent) {
        self.subscribers
            .lock()
            .retain(|tx| tx.send(event.clone()).is_ok());
    }

    fn kill_child(&self) {
        if let Some(mut child) = self.child.lock().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    // -----------------------------------------------------------------------
    // Process management
    // -----------------------------------------------------------------------

    /// Start the sidecar, install its writer queue, and return its stdout for the reader.
    fn launch(&self) -> std::io::Result<ChildStdout> {
        let mut child = Command::new(&self.engine_path)
            .env("SAYSO_MODELS_DIR", &self.models_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");

        thread::Builder::new()
            .name("sayso-engine-stderr".into())
            .spawn(move || {
                for line in BufReader::new(stderr)
                    .lines()
                    .map_while(std::result::Result::ok)
                {
                    log::debug!(target: "sayso_engine", "{line}");
                }
            })?;

        let (tx, rx) = unbounded();
        let audio_queued = Arc::new(AtomicUsize::new(0));
        let queued = Arc::clone(&audio_queued);
        thread::Builder::new()
            .name("sayso-engine-writer".into())
            .spawn(move || write_loop(stdin, &rx, &queued))?;

        self.generation.fetch_add(1, Ordering::SeqCst);
        *self.child.lock() = Some(child);
        self.core.lock().conn = Some(Conn { tx, audio_queued });
        Ok(stdout)
    }

    /// The sidecar's stdout closed. Wait for the exit, close the connection, and fail
    /// every request that is still waiting.
    fn on_exit(&self) -> String {
        let child = self.child.lock().take();
        let description = match child {
            Some(mut child) => {
                let deadline = Instant::now() + Duration::from_secs(2);
                loop {
                    match child.try_wait() {
                        Ok(Some(status)) => break format!("engine exited ({status})"),
                        Ok(None) if Instant::now() < deadline => {
                            thread::sleep(Duration::from_millis(20));
                        }
                        _ => {
                            let _ = child.kill();
                            let _ = child.wait();
                            break "engine was killed after closing its output".to_string();
                        }
                    }
                }
            }
            None => "engine stopped".to_string(),
        };
        let drained: Vec<Pending> = {
            let mut core = self.core.lock();
            core.conn = None;
            core.pending.drain().map(|(_, pending)| pending).collect()
        };
        for pending in drained {
            match pending {
                Pending::Waiter(tx) => {
                    let _ = tx.send(Err(format!("{description} while a request was running")));
                }
                Pending::Detached { what, .. } => log::warn!("engine {what} lost: {description}"),
            }
        }
        description
    }

    /// With the process gone, nothing is loaded or downloading any more.
    fn reset_statuses_after_exit(&self) {
        let current: Vec<(ModelId, ModelStatus)> = self
            .statuses
            .lock()
            .iter()
            .map(|(m, s)| (m.clone(), s.clone()))
            .collect();
        for (model, status) in current {
            match status {
                ModelStatus::Ready | ModelStatus::Optimizing => {
                    self.set_status(model, ModelStatus::Downloaded);
                }
                ModelStatus::Downloading { .. } => {
                    self.set_status(model, ModelStatus::NotDownloaded)
                }
                _ => {}
            }
        }
    }

    fn next_backoff(&self) -> Duration {
        let attempt = self.restart_attempts.fetch_add(1, Ordering::SeqCst).min(16);
        self.options
            .backoff_initial
            .saturating_mul(1u32 << attempt)
            .min(self.options.backoff_max)
    }

    /// After a restart: hello, refresh statuses, load again what was loaded, `Restarted`.
    fn recover(&self, generation: u64) {
        let current = || self.generation.load(Ordering::SeqCst) == generation;
        if let Err(e) = self.call("hello", json!({}), self.options.request_timeout) {
            log::warn!("engine did not answer after restart: {e}");
            if current() {
                self.kill_child(); // The supervisor restarts it again.
            }
            return;
        }
        self.restart_attempts.store(0, Ordering::SeqCst);
        if let Err(e) = self.refresh_statuses() {
            log::warn!("cannot refresh model status after restart: {e}");
        }
        let models: Vec<ModelId> = self.desired_loaded.lock().clone();
        for model in models {
            if !current() || self.shutting_down.load(Ordering::SeqCst) {
                return;
            }
            if let Err(e) = self.load_model(&model) {
                log::warn!("cannot load {model} again after restart: {e}");
            }
        }
        if current() && !self.shutting_down.load(Ordering::SeqCst) {
            self.emit(EngineEvent::Restarted);
        }
    }
}

/// Read the sidecar's stdout until it closes. Restart it when it dies.
fn supervise(inner: &Arc<Inner>, first_stdout: ChildStdout) {
    let mut stdout = first_stdout;
    loop {
        for line in BufReader::new(stdout)
            .lines()
            .map_while(std::result::Result::ok)
        {
            if line.trim().is_empty() {
                continue;
            }
            match Message::parse(&line) {
                Ok(message) => inner.handle(message),
                Err(e) => log::warn!("unreadable engine line ({e}): {line}"),
            }
        }
        let description = inner.on_exit();
        if inner.shutting_down.load(Ordering::SeqCst) {
            break;
        }
        log::error!("{description}");
        inner.emit(EngineEvent::Crashed {
            message: description,
        });
        inner.reset_statuses_after_exit();

        stdout = loop {
            let delay = inner.next_backoff();
            sleep_unless_shutdown(inner, delay);
            if inner.shutting_down.load(Ordering::SeqCst) {
                inner.supervisor_alive.store(false, Ordering::SeqCst);
                return;
            }
            match inner.launch() {
                Ok(stdout) => break stdout,
                Err(e) => log::error!("cannot restart engine: {e}"),
            }
        };
        let generation = inner.generation.load(Ordering::SeqCst);
        let recovering = Arc::clone(inner);
        let spawned = thread::Builder::new()
            .name("sayso-engine-recovery".into())
            .spawn(move || recovering.recover(generation));
        if let Err(e) = spawned {
            log::error!("cannot start recovery thread: {e}");
        }
    }
    inner.supervisor_alive.store(false, Ordering::SeqCst);
}

/// Sleep, but wake early when the client shuts down.
fn sleep_unless_shutdown(inner: &Inner, total: Duration) {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline && !inner.shutting_down.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(10).min(total));
    }
}

/// Write queued messages to the sidecar. Ends when the queue closes or the pipe breaks.
fn write_loop(mut stdin: ChildStdin, queue: &Receiver<Outgoing>, audio_queued: &AtomicUsize) {
    for item in queue {
        let mut line = match item {
            Outgoing::Line(line) => line,
            Outgoing::Audio { session, pcm } => {
                audio_queued.fetch_sub(1, Ordering::Relaxed);
                request_line(
                    "stream_audio",
                    None,
                    json!({"session": session, "pcm": pcm_base64(&pcm)}),
                )
            }
        };
        line.push('\n');
        if stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .is_err()
        {
            break;
        }
    }
}
