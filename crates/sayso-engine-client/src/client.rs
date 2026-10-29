//! `EngineClient`: spawns the SaysoEngine sidecar and implements `SttBackend`.
//!
//! Threads, per client:
//! - **start**: says hello and refreshes statuses after the first launch, then
//!   reports `Started`, or `StartFailed` and stops the sidecar.
//! - **supervisor**: reads the sidecar's stdout. When the pipe closes it reports
//!   `Crashed`, restarts the sidecar with backoff, and starts a recovery thread.
//! - **writer** (one per sidecar process): owns the sidecar's stdin and drains one
//!   queue (see `writer`), so callers (including the audio thread) never wait on the pipe.
//! - **stderr** (one per sidecar process): forwards the sidecar's stderr to `log`.
//! - **recovery** (after a restart): says hello, refreshes statuses, reloads the
//!   models that were loaded before the crash, then reports `Restarted`.

use crate::protocol::{self, Message, pcm_base64, request_line};
use crate::writer::{AudioPush, Outgoing, WriterQueue};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded, unbounded};
use parking_lot::Mutex;
use sayso_core::enhance::{EnhanceError, Generated, LanguageModel, Unavailable};
use sayso_core::models::{self, ModelId, ModelInfo};
use sayso_core::stt::{
    Diarization, EngineEvent, EngineReason, ModelStatus, SAMPLE_RATE, SessionOptions, SpeechPart, TimedTranscript, Transcript,
    encode_wav, to_pcm16,
};
use sayso_platform::{EngineStart, PartReadiness, PlatformError, Result, SttBackend, unsupported_feature};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use sayso_core::cpu::{self, Level};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// The sidecar's name beside the app executable, without a build suffix or
/// `.exe`: the Swift engine on macOS, and `sayso-engine` (`crates/sayso-engine`)
/// on Windows and Linux. Windows has a build of it for each level of the
/// processor ([`sayso_core::cpu`]), because whisper.cpp compiles its kernels for
/// one instruction set.
const ENGINE_PROGRAM: &str = if cfg!(target_os = "macos") {
    "SaysoEngine"
} else {
    "sayso-engine"
};

/// The start of the message of a request that failed because the engine
/// stopped with an illegal instruction.
const UNSUPPORTED_CPU: &str = "the processor of this computer cannot run this model";

/// The requests that run a model. When the engine stops with an illegal
/// instruction during one of them, the model is one that this build cannot
/// run on this processor.
const RUNS_A_MODEL: &[&str] = &["load", "transcribe", "stream_start", "vad", "vad_start", "diarize"];

/// The development build, relative to this crate. None where the workspace
/// builds the engine next to the app binary.
const DEV_ENGINE_PATH: Option<&str> = if cfg!(target_os = "macos") {
    Some("../../native/macos/SaysoEngine/.build/release/SaysoEngine")
} else {
    None
};

/// Timing knobs. The defaults suit the real sidecar. Tests shorten them.
#[derive(Debug, Clone)]
pub struct Options {
    /// Wait for quick requests: hello, list, delete, stream start.
    pub request_timeout: Duration,
    /// Wait for `load` (first load compiles for the Neural Engine, 60 s or more
    /// for Whisper) and for `transcribe`.
    ///
    /// The requests that read a whole recording and can run long (`transcribe_segments`,
    /// `detect_speech`, and `diarize`) wait for `long_timeout` plus
    /// `audio_timeout_factor` times the length of the audio. A long recording
    /// then gets more time, and a short one still fails after a bounded wait.
    pub long_timeout: Duration,
    /// How many times the length of the audio the long requests wait, on top of
    /// `long_timeout`. 2 means: 10 minutes of audio add 20 minutes.
    pub audio_timeout_factor: u32,
    /// First restart delay after a crash. It doubles each time.
    pub backoff_initial: Duration,
    /// Longest restart delay.
    pub backoff_max: Duration,
    /// Extra environment for the sidecar, also after a restart.
    pub env: Vec<(String, String)>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            request_timeout: Duration::from_secs(15),
            long_timeout: Duration::from_secs(600),
            audio_timeout_factor: 2,
            backoff_initial: Duration::from_millis(500),
            backoff_max: Duration::from_secs(5),
            env: Vec::new(),
        }
    }
}

/// Where the first hello is. The supervisor restarts the sidecar only after
/// the first hello came back.
const START_PENDING: u8 = 0;
const START_DONE: u8 = 1;
const START_FAILED: u8 = 2;

fn failed(message: impl Into<String>) -> PlatformError {
    PlatformError::Failed(message.into())
}

fn unsupported_cpu_status(message: &str) -> ModelStatus {
    ModelStatus::Failed {
        message: message.to_string(),
        reason: Some(EngineReason::UnsupportedCpu),
    }
}

/// Why a request failed. `code` is the `code` field of an `error` reply, or
/// `timeout` when the engine did not answer in time.
struct Failure {
    message: String,
    code: Option<String>,
}

/// What to do with the reply of a request.
enum Pending {
    /// A caller blocks on this channel.
    Waiter(Sender<std::result::Result<Message, Failure>>),
    /// Nobody waits. A failure is logged and, for a model, shown as `Failed`.
    Detached {
        what: &'static str,
        model: Option<ModelId>,
    },
}

/// A live sidecar process, as seen by senders.
struct Conn {
    tx: Arc<WriterQueue>,
}

impl Drop for Conn {
    /// The writer sends what is queued, closes stdin, and ends.
    fn drop(&mut self) {
        self.tx.close();
    }
}

/// Connection and pending requests share one lock, so a request is either
/// registered before a crash drains the map or refused after it.
struct Core {
    conn: Option<Conn>,
    pending: HashMap<String, Pending>,
}

struct Preparation {
    request: String,
    preview: bool,
    vocabulary: Vec<String>,
    parts: [PartReadiness; 2],
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
    /// Models that are not `Ready` but whose live preview is in memory
    /// (`model_state` with `preview: true`).
    preview_ready: Mutex<HashSet<ModelId>>,
    /// The `features` of the last `hello`.
    features: Mutex<Vec<String>>,
    /// Open VAD sessions. They use the stream paths, but their audio is never dropped.
    vad_sessions: Mutex<HashSet<u64>>,
    preparation: Mutex<HashMap<ModelId, Preparation>>,
    child: Mutex<Option<Child>>,
    next_id: AtomicU64,
    next_file: AtomicU64,
    generation: AtomicU64,
    restart_attempts: AtomicU32,
    shutting_down: AtomicBool,
    supervisor_alive: AtomicBool,
    dropped_audio: AtomicU64,
    /// `START_PENDING`, `START_DONE`, or `START_FAILED`.
    start: AtomicU8,
    /// The generation of the last sidecar process that stopped with an
    /// illegal instruction, and the message for it.
    illegal_stop: Mutex<Option<(u64, String)>>,
    /// Models that stopped the sidecar with an illegal instruction, with the
    /// message. A request that runs one fails at once, so the app does not
    /// crash the engine again and again (issue #37).
    cannot_run: Mutex<HashMap<ModelId, String>>,
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

    /// Like [`EngineClient::spawn`], with explicit options: timing, and environment for the sidecar.
    pub fn spawn_with(
        engine_path: PathBuf,
        models_dir: PathBuf,
        cache_dir: PathBuf,
        options: Options,
    ) -> Result<Self> {
        let (client, events) = Self::start_with(engine_path, models_dir, cache_dir, options)?;
        for event in events {
            match event {
                EngineEvent::Started => return Ok(client),
                EngineEvent::StartFailed { message } => return Err(failed(message)),
                _ => {}
            }
        }
        Err(failed("engine stopped"))
    }

    /// Start the sidecar and return without a wait for its answer. The hello
    /// and the status refresh run on a thread of their own. The receiver
    /// gets every event from the start on: `Started` when the engine answers,
    /// or `StartFailed` when it does not. After `StartFailed` the sidecar is
    /// stopped and does not restart.
    ///
    /// An error here means that the sidecar could not run at all, for
    /// example because the binary is missing.
    pub fn start(engine_path: PathBuf, models_dir: PathBuf, cache_dir: PathBuf) -> Result<(Self, Receiver<EngineEvent>)> {
        Self::start_with(engine_path, models_dir, cache_dir, Options::default())
    }

    /// Like [`EngineClient::start`], with explicit timing options.
    pub fn start_with(
        engine_path: PathBuf,
        models_dir: PathBuf,
        cache_dir: PathBuf,
        options: Options,
    ) -> Result<(Self, Receiver<EngineEvent>)> {
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
            preview_ready: Mutex::new(HashSet::new()),
            features: Mutex::new(Vec::new()),
            vad_sessions: Mutex::new(HashSet::new()),
            preparation: Mutex::new(HashMap::new()),
            child: Mutex::new(None),
            next_id: AtomicU64::new(0),
            next_file: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            restart_attempts: AtomicU32::new(0),
            shutting_down: AtomicBool::new(false),
            supervisor_alive: AtomicBool::new(true),
            dropped_audio: AtomicU64::new(0),
            start: AtomicU8::new(START_PENDING),
            illegal_stop: Mutex::new(None),
            cannot_run: Mutex::new(HashMap::new()),
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
            _guard: Arc::new(Guard(Arc::clone(&inner))),
        };
        // Before the start thread, so no event is lost.
        let events = client.subscribe();
        thread::Builder::new()
            .name("sayso-engine-start".into())
            .spawn(move || inner.first_start())
            .map_err(|e| failed(format!("cannot start engine thread: {e}")))?;
        Ok((client, events))
    }

    /// Find the sidecar binary. In order: env `SAYSO_ENGINE_PATH`, the engine next to
    /// the running executable (app bundle, install folder, or cargo target folder), then
    /// the development build in this workspace. If none exists, the development path
    /// comes back so the spawn error names it.
    ///
    /// The engine is `SaysoEngine` (Swift) on macOS, and `sayso-engine` (Rust,
    /// `crates/sayso-engine`) on Windows and Linux. Next to the executable, the
    /// best build that this processor runs wins (`sayso-engine-avx.exe` and
    /// `sayso-engine-baseline.exe` on Windows, [`sayso_core::cpu`]).
    pub fn default_engine_path() -> PathBuf {
        if let Some(path) = std::env::var_os("SAYSO_ENGINE_PATH").filter(|p| !p.is_empty()) {
            return PathBuf::from(path);
        }
        let dir = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf));
        if let Some(best) = dir.as_deref().and_then(|dir| cpu::best_in(dir, ENGINE_PROGRAM)) {
            return best;
        }
        let file = Level::Avx2.file_name(ENGINE_PROGRAM);
        match DEV_ENGINE_PATH {
            Some(dev) => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(dev),
            // `cargo build -p sayso-engine` puts the engine next to the app binary.
            None => dir.map_or_else(|| PathBuf::from(&file), |dir| dir.join(&file)),
        }
    }

    /// Number of live preview audio frames dropped because the sidecar fell behind.
    pub fn dropped_audio_frames(&self) -> u64 {
        self.inner.dropped_audio.load(Ordering::Relaxed)
    }
}

/// How long the engine may take to end a `generate` request after its own
/// deadline, before the client stops the wait.
const GENERATE_GRACE: Duration = Duration::from_millis(500);

impl Failure {
    fn into_enhance_error(self, timeout: Duration) -> EnhanceError {
        match self.code.as_deref() {
            Some("timeout") => EnhanceError::Timeout(timeout.as_millis() as u64),
            Some("unavailable") => EnhanceError::NotConfigured(self.message),
            Some("refused") => EnhanceError::Refused(self.message),
            _ => EnhanceError::Cli(self.message),
        }
    }
}

impl LanguageModel for EngineClient {
    fn model_name(&self) -> std::result::Result<String, EnhanceError> {
        self.availability()
            .map_err(|unavailable| EnhanceError::NotConfigured(unavailable.message))
    }

    fn availability(&self) -> std::result::Result<String, Unavailable> {
        let timeout = self.inner.options.request_timeout;
        let reply = self
            .inner
            .request("language_model", json!({}), timeout)
            // An engine without the request answers `error`: it has no language model.
            .map_err(|failure| Unavailable {
                reason: None,
                message: failure.message,
            })?;
        if reply.body["available"] == true {
            Ok(reply.str("model").unwrap_or("language model").to_string())
        } else {
            Err(Unavailable {
                reason: protocol::reason(&reply),
                message: reply
                    .str("message")
                    .unwrap_or("the language model cannot run")
                    .to_string(),
            })
        }
    }

    fn generate(
        &self,
        instructions: &str,
        prompt: &str,
        temperature: Option<f32>,
        timeout: Duration,
    ) -> std::result::Result<Generated, EnhanceError> {
        let mut fields = json!({
            "instructions": instructions,
            "prompt": prompt,
            "timeout_ms": timeout.as_millis() as u64,
        });
        if let Some(temperature) = temperature {
            fields["temperature"] = json!(temperature);
        }
        let reply = self
            .inner
            .request("generate", fields, timeout + GENERATE_GRACE)
            .map_err(|failure| failure.into_enhance_error(timeout))?;
        let text = reply
            .str("text")
            .ok_or_else(|| EnhanceError::InvalidOutput("the engine sent no text".into()))?;
        Ok(Generated {
            text: text.to_string(),
            model: reply.str("model").unwrap_or("language model").to_string(),
        })
    }
}

impl SttBackend for EngineClient {
    fn start_state(&self) -> EngineStart {
        match self.inner.start.load(Ordering::SeqCst) {
            START_DONE => EngineStart::Started,
            START_FAILED => EngineStart::Failed,
            _ => EngineStart::Starting,
        }
    }

    fn prepare(&self, model: &ModelId, preview: bool, vocabulary: &[String]) -> Result<()> {
        let mut preparations = self.inner.preparation.lock();
        if preparations.get(model).is_some_and(|p| {
            p.preview == preview && p.vocabulary == vocabulary
                && !p.parts.contains(&PartReadiness::Failed)
        }) {
            return Ok(());
        }
        let request = self.inner.send(
            "prepare",
            json!({"model": model, "preview": preview, "vocabulary": vocabulary}),
            Pending::Detached { what: "prepare", model: None },
        )?;
        preparations.insert(model.clone(), Preparation {
            request,
            preview,
            vocabulary: vocabulary.to_vec(),
            parts: [
                if preview { PartReadiness::Preparing } else { PartReadiness::Unprepared },
                if vocabulary.is_empty() { PartReadiness::Unprepared } else { PartReadiness::Preparing },
            ],
        });
        Ok(())
    }

    fn part_readiness(&self, model: &ModelId, preview: bool) -> PartReadiness {
        self.inner.preparation.lock().get(model).map_or(
            PartReadiness::Unprepared,
            |p| p.parts[if preview { 0 } else { 1 }],
        )
    }

    fn catalog(&self) -> Vec<ModelInfo> {
        models::catalog().to_vec()
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
        self.inner.preparation.lock().remove(model);
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

    fn unload(&self, model: &ModelId, keep_preview: bool) -> Result<()> {
        self.inner.preparation.lock().remove(model);
        self.inner.desired_loaded.lock().retain(|m| m != model);
        self.inner.send(
            "unload",
            json!({"model": model, "keep_preview": keep_preview}),
            Pending::Detached {
                what: "unload",
                model: None,
            },
        )?;
        // The cache must not say `Ready` while the engine drops the model.
        if self.status(model) == ModelStatus::Ready {
            self.inner
                .set_status(model.clone(), ModelStatus::Downloaded);
        }
        Ok(())
    }

    fn preview_ready(&self, model: &ModelId) -> bool {
        self.inner.preview_ready.lock().contains(model)
    }

    fn start_stream(&self, session: u64, options: &SessionOptions) -> Result<()> {
        let Some(preview) = &options.preview_model else {
            return Ok(()); // Live preview is off.
        };
        if let Some(conn) = self.inner.core.lock().conn.as_ref() {
            conn.tx.begin_stream(session);
        }
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
        let vad = self.inner.vad_sessions.lock().contains(&session);
        let core = self.inner.core.lock();
        let Some(conn) = core.conn.as_ref() else {
            return Err(failed("engine is not running"));
        };
        match conn.tx.push_audio(session, pcm) {
            Ok(AudioPush::Queued | AudioPush::Ended) => Ok(()),
            Ok(AudioPush::Full) => {
                self.inner.dropped_audio.fetch_add(1, Ordering::Relaxed);
                // A missing frame moves every later time of a VAD session, so the caller must know.
                if vad {
                    Err(failed("the engine falls behind: VAD audio was dropped"))
                } else {
                    Ok(())
                }
            }
            Err(_) => Err(failed("engine is not running")),
        }
    }

    /// End the live preview of `session`. The preview audio that waits in the writer
    /// queue is dropped, so the requests behind it do not wait for it. `stream_end`
    /// still follows every frame that the writer already took.
    ///
    /// A VAD session (see [`SttBackend::start_vad`]) keeps that audio: the engine
    /// needs every frame to find the times.
    fn end_stream(&self, session: u64) {
        let vad = self.inner.vad_sessions.lock().remove(&session);
        if let Some(conn) = self.inner.core.lock().conn.as_ref() {
            let dropped = if vad {
                conn.tx.finish_stream(session);
                0
            } else {
                conn.tx.end_stream(session)
            };
            if dropped > 0 {
                log::debug!(
                    "stream {session} ended with {dropped} audio frames not yet sent, dropped"
                );
            }
        }
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

    /// End a VAD session and wait for the reply to `stream_end`. The reader
    /// sends the events of a line before it finishes the next reply, so the
    /// last `SpeechEnd` is with the subscribers when this returns.
    fn end_vad(&self, session: u64) -> Result<()> {
        if !self.inner.vad_sessions.lock().remove(&session) {
            return Err(failed(format!("no VAD session {session} is open")));
        }
        if let Some(conn) = self.inner.core.lock().conn.as_ref() {
            conn.tx.finish_stream(session);
        }
        self.inner
            .call("stream_end", json!({"session": session}), self.inner.options.long_timeout)?;
        Ok(())
    }

    fn transcribe(&self, samples: &[f32], options: &SessionOptions) -> Result<Transcript> {
        let wav = self.inner.write_wav("transcribe", samples)?;
        let path = &wav.0;
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

    fn features(&self) -> Vec<String> {
        self.inner.features.lock().clone()
    }

    fn transcribe_segments(&self, samples: &[f32], options: &SessionOptions) -> Result<TimedTranscript> {
        self.inner.require("segments")?;
        let wav = self.inner.write_wav("transcribe", samples)?;
        let fields = json!({
            "model": options.final_model,
            "path": wav.0.to_string_lossy(),
            "language": language_for(&options.final_model, &options.language),
            "vocabulary": options.vocabulary,
            "segments": true,
        });
        let reply = self
            .inner
            .call("transcribe", fields, self.inner.long_timeout_for(samples))?;
        // An engine without `segments` ignores the field and sends no list.
        let segments = protocol::segments(&reply)
            .map_err(|e| failed(format!("the engine sent no usable segments: {e}")))?;
        Ok(TimedTranscript {
            text: reply.str("text").unwrap_or_default().to_string(),
            model: options.final_model.clone(),
            elapsed_ms: reply.u64("elapsed_ms").unwrap_or(0),
            segments,
        })
    }

    fn detect_speech(&self, model: &ModelId, samples: &[f32]) -> Result<Vec<SpeechPart>> {
        self.inner.require("vad")?;
        let wav = self.inner.write_wav("vad", samples)?;
        let fields = json!({"model": model, "path": wav.0.to_string_lossy()});
        let reply = self
            .inner
            .call("vad", fields, self.inner.long_timeout_for(samples))?;
        protocol::speech(&reply).map_err(|e| failed(format!("the engine sent no usable speech parts: {e}")))
    }

    fn start_vad(&self, session: u64, model: &ModelId) -> Result<()> {
        self.inner.require("vad")?;
        if let Some(conn) = self.inner.core.lock().conn.as_ref() {
            conn.tx.begin_stream(session);
        }
        self.inner.vad_sessions.lock().insert(session);
        let opened = self.inner.call(
            "vad_start",
            json!({"session": session, "model": model}),
            self.inner.options.request_timeout,
        );
        if opened.is_err() {
            self.inner.vad_sessions.lock().remove(&session);
        }
        opened.map(|_| ())
    }

    fn diarize(&self, model: &ModelId, samples: &[f32], speakers: Option<u32>) -> Result<Diarization> {
        self.inner.require("diarization")?;
        let wav = self.inner.write_wav("diarize", samples)?;
        let mut fields = json!({"model": model, "path": wav.0.to_string_lossy()});
        if let Some(speakers) = speakers {
            fields["speakers"] = json!(speakers);
        }
        let reply = self
            .inner
            .call("diarize", fields, self.inner.long_timeout_for(samples))?;
        protocol::diarization(&reply).map_err(|e| failed(format!("the engine sent no usable turns: {e}")))
    }

    fn subscribe(&self) -> Receiver<EngineEvent> {
        let (tx, rx) = unbounded();
        self.inner.subscribers.lock().push(tx);
        rx
    }
}

/// A local model of any task: a speech model, or a VAD or diarization model
/// that the app downloads and deletes.
fn find_model(model: &ModelId) -> Result<ModelInfo> {
    models::find_local(model).ok_or_else(|| failed(format!("unknown model {model}")))
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
    /// Fail at once when the engine said in its `hello` that it lacks `feature`.
    /// Before the first `hello`, and for an engine that lists no features, the
    /// request goes out, and the `error` reply of the engine is the answer.
    fn require(&self, feature: &str) -> Result<()> {
        let features = self.features.lock();
        if features.is_empty() || features.iter().any(|f| f == feature) {
            Ok(())
        } else {
            Err(unsupported_feature(feature))
        }
    }

    /// How long a request on `samples` waits: `long_timeout` plus the length of the
    /// audio times `audio_timeout_factor` (see [`Options`]).
    fn long_timeout_for(&self, samples: &[f32]) -> Duration {
        let audio = Duration::from_secs_f64(samples.len() as f64 / f64::from(SAMPLE_RATE));
        self.options
            .long_timeout
            .saturating_add(audio.saturating_mul(self.options.audio_timeout_factor))
    }

    /// Write `samples` as a WAV file in the cache folder. The file goes away when the result drops.
    fn write_wav(&self, what: &str, samples: &[f32]) -> Result<RemoveOnDrop> {
        let n = self.next_file.fetch_add(1, Ordering::Relaxed);
        let path = self
            .cache_dir
            .join(format!("{what}-{}-{n}.wav", std::process::id()));
        std::fs::write(&path, encode_wav(&to_pcm16(samples)))
            .map_err(|e| failed(format!("cannot write {}: {e}", path.display())))?;
        Ok(RemoveOnDrop(path))
    }

    /// Say hello, and keep the `features` of the engine.
    fn hello(&self) -> Result<()> {
        let reply = self.call("hello", json!({}), self.options.request_timeout)?;
        *self.features.lock() = protocol::features(&reply);
        Ok(())
    }

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
    /// The `code` of the engine stays in [`PlatformError::Engine`], for
    /// example `too_many_streams` from `stream_start` and `vad_start`.
    fn call(&self, kind: &str, fields: Value, timeout: Duration) -> Result<Message> {
        self.request(kind, fields, timeout).map_err(|failure| match failure.code {
            Some(code) => PlatformError::Engine { code, message: failure.message },
            None => failed(failure.message),
        })
    }

    /// Like [`Inner::call`], but a failure keeps its code.
    fn request(
        &self,
        kind: &str,
        fields: Value,
        timeout: Duration,
    ) -> std::result::Result<Message, Failure> {
        let plain = |message: String| Failure {
            message,
            code: None,
        };
        let model = RUNS_A_MODEL
            .contains(&kind)
            .then(|| fields.get("model").and_then(Value::as_str).map(ModelId::new))
            .flatten();
        // The lock ends here: `set_status` takes it again.
        let cannot_run = model.as_ref().and_then(|model| self.cannot_run.lock().get(model).cloned());
        if let Some(model) = &model
            && let Some(message) = cannot_run
        {
            self.set_status(model.clone(), unsupported_cpu_status(&message));
            return Err(plain(message));
        }
        let generation = self.generation.load(Ordering::SeqCst);
        let (tx, rx) = bounded(1);
        let id = self
            .send(kind, fields, Pending::Waiter(tx))
            .map_err(|e| plain(e.to_string()))?;
        let reply = match rx.recv_timeout(timeout) {
            Ok(reply) => reply,
            Err(RecvTimeoutError::Timeout) => {
                self.core.lock().pending.remove(&id);
                Err(Failure {
                    message: format!("engine did not answer {kind} within {timeout:?}"),
                    code: Some("timeout".into()),
                })
            }
            Err(RecvTimeoutError::Disconnected) => Err(plain("engine stopped".into())),
        };
        let illegal_stop = self.illegal_stop.lock().clone();
        if reply.is_err()
            && let Some(model) = model
            && let Some((stopped, message)) = illegal_stop
            && stopped == generation
        {
            self.cannot_run_model(model, &message);
            return Err(plain(message));
        }
        reply
    }

    /// `model` stopped the sidecar with an illegal instruction: this build of
    /// the engine cannot run it here. It is not loaded again after the
    /// restart, it shows as failed, and the next request for it fails at once.
    fn cannot_run_model(&self, model: ModelId, message: &str) {
        log::error!("{model}: {message}");
        self.desired_loaded.lock().retain(|m| *m != model);
        self.cannot_run.lock().insert(model.clone(), message.to_string());
        self.set_status(model, unsupported_cpu_status(message));
    }

    fn load_model(&self, model: &ModelId) -> Result<()> {
        let info = find_model(model)?;
        let fields = json!({"model": info.id, "engine": info.engine});
        self.call("load", fields, self.options.long_timeout)?;
        Ok(())
    }

    /// Ask the sidecar for the disk state of every local model: the speech,
    /// VAD, and diarization models. No network.
    fn refresh_statuses(&self) -> Result<()> {
        let list: Vec<Value> = models::local_models()
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
            "part_state" => {
                if let Some(model) = message.str("model") {
                    let mut preparations = self.preparation.lock();
                    if let Some(p) = preparations.get_mut(&ModelId::new(model))
                        && message.id.as_deref() == Some(p.request.as_str())
                    {
                        let index = match message.str("part") { Some("preview") => 0, Some("dictionary") => 1, _ => return };
                        p.parts[index] = match message.str("state") {
                            Some("preparing") => PartReadiness::Preparing,
                            Some("ready") => PartReadiness::Ready,
                            Some("failed") => PartReadiness::Failed,
                            _ => return,
                        };
                    }
                }
            }

            "model_state" | "download_progress" => {
                if let Some((model, status)) = protocol::model_status(&message) {
                    if message.kind == "model_state" {
                        let mut kept = self.preview_ready.lock();
                        if message.body["preview"] == true {
                            kept.insert(model.clone());
                        } else {
                            kept.remove(&model);
                        }
                    }
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
            "speech_start" => match (message.u64("session"), message.u64("start_ms")) {
                (Some(session), Some(start_ms)) => self.emit(EngineEvent::SpeechStart { session, start_ms }),
                _ => log::debug!("ignoring speech_start without session or start_ms"),
            },
            "speech_end" => {
                match (message.u64("session"), message.u64("start_ms"), message.u64("end_ms")) {
                    (Some(session), Some(start_ms), Some(end_ms)) => {
                        self.emit(EngineEvent::SpeechEnd { session, start_ms, end_ms });
                    }
                    _ => log::debug!("ignoring speech_end without session, start_ms, or end_ms"),
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
        // An older engine can reject prepare. Keep the model usable and permit a retry.
        if let Some(id) = message.id.as_ref() {
            for p in self.preparation.lock().values_mut().filter(|p| &p.request == id) {
                for state in &mut p.parts {
                    if *state == PartReadiness::Preparing {
                        *state = if message.kind == "error" { PartReadiness::Failed } else { PartReadiness::Ready };
                    }
                }
            }
        }

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
                let _ = tx.send(if is_error {
                    let code = message.str("code").map(str::to_string);
                    Err(Failure {
                        message: text,
                        code,
                    })
                } else {
                    Ok(message)
                });
            }
            Pending::Detached { what, model } => {
                if is_error {
                    log::warn!("engine {what} failed: {text}");
                    if let Some(model) = model {
                        // The engine sent `model_state failed` with the reason just before
                        // this reply. Keep that reason when the text is the same.
                        let reason = protocol::reason(&message).or_else(|| match self.statuses.lock().get(&model) {
                            Some(ModelStatus::Failed { message: known, reason }) if *known == text => *reason,
                            _ => None,
                        });
                        self.set_status(model, ModelStatus::Failed { message: text, reason });
                    }
                }
            }
        }
    }

    /// Update the status cache. Subscribers hear about real changes only.
    fn set_status(&self, model: ModelId, status: ModelStatus) {
        // A model that this build cannot run stays failed while its files are
        // there, also after the restart reports them again.
        let status = match self.cannot_run.lock().get(&model) {
            Some(message) if status.is_on_disk() => unsupported_cpu_status(message),
            _ => status,
        };
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
        let mut command = Command::new(&self.engine_path);
        #[cfg(windows)]
        {
            // CREATE_NO_WINDOW: the app is a GUI program, so a console child
            // would open a console window.
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command
            .env("SAYSO_MODELS_DIR", &self.models_dir)
            .envs(self.options.env.iter().map(|(k, v)| (k, v)))
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

        let tx = Arc::new(WriterQueue::default());
        let queue = Arc::clone(&tx);
        thread::Builder::new()
            .name("sayso-engine-writer".into())
            .spawn(move || write_loop(stdin, &queue))?;

        self.generation.fetch_add(1, Ordering::SeqCst);
        *self.child.lock() = Some(child);
        self.core.lock().conn = Some(Conn { tx });
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
                        Ok(Some(status)) if cpu::illegal_instruction(&status) => {
                            let needs = Level::of_file(ENGINE_PROGRAM, &self.engine_path)
                                .map_or("an instruction that it lacks", Level::needs);
                            let message = format!(
                                "{UNSUPPORTED_CPU}: the engine {} needs {needs} ({status})",
                                self.engine_path.display()
                            );
                            // Before the waiters fail, so that they find it.
                            *self.illegal_stop.lock() = Some((self.generation.load(Ordering::SeqCst), message));
                            break format!("engine stopped with an illegal instruction ({status})");
                        }
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
                    let _ = tx.send(Err(Failure {
                        message: format!("{description} while a request was running"),
                        code: None,
                    }));
                }
                Pending::Detached { what, .. } => log::warn!("engine {what} lost: {description}"),
            }
        }
        description
    }

    /// With the process gone, nothing is loaded or downloading any more.
    fn reset_statuses_after_exit(&self) {
        self.preview_ready.lock().clear();
        self.vad_sessions.lock().clear();
        self.preparation.lock().clear();
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

    /// After the first launch: hello and refresh statuses, then `Started`. When
    /// either fails, stop the sidecar for good and report `StartFailed`.
    fn first_start(&self) {
        let answered = self.hello().and_then(|()| self.refresh_statuses());
        match answered {
            Ok(()) => {
                self.start.store(START_DONE, Ordering::SeqCst);
                self.emit(EngineEvent::Started);
            }
            Err(e) => {
                // Before the state, so the supervisor sees it and does not restart.
                self.shutting_down.store(true, Ordering::SeqCst);
                self.start.store(START_FAILED, Ordering::SeqCst);
                self.kill_child();
                self.emit(EngineEvent::StartFailed { message: e.to_string() });
            }
        }
    }

    /// After a restart: hello, refresh statuses, load again what was loaded, `Restarted`.
    fn recover(&self, generation: u64) {
        let current = || self.generation.load(Ordering::SeqCst) == generation;
        if let Err(e) = self.hello() {
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
        // The engine stopped before its first hello came back. The start
        // thread gets the failure from `on_exit` and decides at once.
        while inner.start.load(Ordering::SeqCst) == START_PENDING {
            thread::sleep(Duration::from_millis(5));
        }
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
fn write_loop(mut stdin: ChildStdin, queue: &WriterQueue) {
    while let Some(item) = queue.next() {
        let mut line = match item {
            Outgoing::Line(line) => line,
            Outgoing::Audio { session, pcm } => request_line(
                "stream_audio",
                None,
                json!({"session": session, "pcm": pcm_base64(&pcm)}),
            ),
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
