//! Live sessions (`docs/extensions.md`, "Requests" `session_start`,
//! `session_audio`, `session_stop`, and "Scheduling", "Capture", "Lifecycle",
//! "Privacy"): audio that is transcribed for an app while it arrives.
//!
//! A session has one or more sources (`mic`, `system`, `client`), and each source is a
//! pipeline of its own (`source`): the audio goes to the recording (a WAV
//! file), to a preview stream of the engine when the session has a streaming
//! model (`partial` events), and to a VAD (a VAD session of the engine, or
//! Basic VAD in the host). Each speech that ends is a part, and the final
//! model transcribes it with segments (`segment` events, with times from the
//! start of the session). [`LiveSessions::stop`] ends the audio, waits for the
//! last parts, runs diarization on the recording of each source, and sends
//! `speakers` events.
//!
//! The parts of this module do not know the connection: the events go to an
//! [`EventSink`] (the same type as for jobs), the engine is a
//! [`SpeechEngine`], the microphone a [`Microphone`], the system audio a
//! [`SystemAudio`], dictation first is a [`Gate`] (the [`DictationGate`](crate::DictationGate) of jobs), and the
//! recordings go where [`Recordings`] says. Sayso gives them in
//! `sayso-host` (`live_requests.rs`).

mod echo;
mod silence;
mod source;
mod wav;

#[cfg(test)]
mod tests;

use crate::jobs::{Gate, VadRun, speakers};
use crate::{EngineError, EventSink, ModelId, ModelRetention, SpeechEngine};
use crossbeam_channel::{Receiver, Sender, unbounded};
use echo::EchoGate;
use parking_lot::Mutex;
use sayso_protocol::{ErrorCode, Event, Reply, Source, SpeakersEvent};
use silence::SilenceWatch;
use source::{EchoRole, SessionClock, SourcePlan, SourceResult, SourceRun, SourceVad};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use wav::WavWriter;

/// The first engine session id of live sessions.
///
/// A dictation numbers its engine sessions from 1 up, one for each
/// dictation (`sayso_core::dictation::Machine`). Live sessions take their ids from
/// 2^40 up, so the two never meet: Sayso would need 2^40 dictations. 2^40 is
/// under 2^53, so an engine that reads JSON numbers as doubles keeps the ids exact.
pub const ENGINE_SESSION_BASE: u64 = 1 << 40;

/// The microphone. In Sayso, the microphone that the user chose in
/// Settings, shared with dictation.
pub trait Microphone: Send + Sync {
    /// Start to get the audio of the microphone. The frames (16 kHz mono)
    /// come in the order of capture, from now on, and never wait for the
    /// receiver. Dropping the feed stops them.
    fn open(&self) -> Result<AudioFeed, OpenError>;
}

/// The system audio: the sound of other apps and calls (`sayso_capture::live`).
pub trait SystemAudio: Send + Sync {
    /// Start to get the system audio, as [`Microphone::open`] does. It can
    /// wait for the answer of the user to a question of the system. `app_pid`
    /// is the process of the app that asked, for the capture to leave out.
    fn open(&self, app_pid: Option<u32>) -> Result<AudioFeed, OpenError>;
}

/// The audio of one captured source.
pub struct AudioFeed {
    pub items: Receiver<FeedItem>,
    /// Dropping it ends the capture; the channel then closes after the last frame.
    pub lease: Box<dyn Send>,
    /// What the capture leaves out, for the reply of `session_start`.
    pub excludes: Vec<String>,
}

/// What a capture gives, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum FeedItem {
    Audio(CapturedAudio),
    /// The capture lost this much audio, for example at a change of the device.
    Gap {
        ms: u64,
    },
    /// The capture ended before the session. No more items come.
    Ended(SourceEnd),
}

/// 16 kHz mono samples.
#[derive(Debug, Clone, PartialEq)]
pub struct CapturedAudio {
    pub samples: Vec<f32>,
    /// The time of the first sample on the capture clock, which the sources of
    /// one session share. None: this source has no such clock, and its times
    /// are its samples.
    pub time: Option<Duration>,
}

impl CapturedAudio {
    /// Samples without a time on the capture clock.
    pub fn untimed(samples: Vec<f32>) -> CapturedAudio {
        CapturedAudio {
            samples,
            time: None,
        }
    }
}

/// Why a capture ended before the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceEnd {
    /// The user took the permission of the system back.
    Permission,
    /// The capture failed and did not come back. The text is English.
    Failed(String),
}

/// Why a capture did not start. The text is English.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// The system refused the permission.
    Permission(String),
    /// This system cannot capture it.
    Unsupported(String),
    Failed(String),
}

impl OpenError {
    fn live(self, source: &Source) -> LiveError {
        let (code, message) = match self {
            OpenError::Permission(message) => (Some(ErrorCode::Permission), message),
            OpenError::Unsupported(message) => (Some(ErrorCode::Unsupported), message),
            OpenError::Failed(message) => (None, message),
        };
        LiveError::new(
            code,
            format!("the source {source} did not start: {message}"),
        )
    }
}

/// Tells the app which live session runs, for the menu bar.
pub trait LiveObserver: Send + Sync {
    /// A session started (`Some`) or ended (`None`).
    fn changed(&self, live: Option<&LiveInfo>);
}

/// The live session that runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveInfo {
    pub session: u64,
    pub app_id: String,
    pub sources: Vec<Source>,
    /// True while it captures the microphone. Sayso shows that it records.
    pub captures_mic: bool,
    /// True after `session_stop`, while the last parts and diarization run.
    pub stopping: bool,
}

/// A live session after the model checks. The wiring finds the models from
/// the request and the settings of the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSpec {
    pub sources: Vec<Source>,
    /// The model of the final pass of each part.
    pub final_model: ModelId,
    /// The streaming model of the `partial` events. None: no preview.
    pub preview_model: Option<ModelId>,
    pub vad: VadRun,
    /// None: no diarization.
    pub diarization: Option<ModelId>,
    /// A code such as `en`, or `auto`.
    pub language: String,
    /// A folder for the recordings; else temporary files.
    pub record_to: Option<PathBuf>,
    /// Stop the audio after this long with no speech in any source. None: no limit.
    pub silence_limit: Option<Duration>,
}

impl SessionSpec {
    /// The engine models of the session.
    pub fn models(&self) -> Vec<ModelId> {
        let mut models = vec![self.final_model.clone()];
        models.extend(self.preview_model.clone());
        if let VadRun::Model(vad) = &self.vad {
            models.push(vad.clone());
        }
        models.extend(self.diarization.clone());
        models.dedup();
        models
    }
}

/// Who owns a session: the connection that started it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    pub connection: u64,
    pub app_id: String,
    /// The process of the app, when it is not this process. The capture of the
    /// system audio leaves it out.
    pub pid: Option<u32>,
}

/// A session that started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    pub session: u64,
    /// What the capture of the system audio leaves out.
    pub excludes: Vec<String>,
}

/// What the sessions need from the app.
pub struct LiveDeps {
    pub engine: Arc<dyn SpeechEngine>,
    /// None when this app cannot capture the microphone.
    pub microphone: Option<Arc<dyn Microphone>>,
    /// None when this app cannot capture the system audio.
    pub system_audio: Option<Arc<dyn SystemAudio>>,
    pub gate: Arc<dyn Gate>,
    /// The models that a session uses stay in memory while it runs (the
    /// same use as jobs, for the memory policy of the app).
    pub models: Arc<dyn ModelRetention>,
    /// Where the recordings go.
    pub recordings: Arc<dyn Recordings>,
}

/// Where the recordings of a live session go.
pub trait Recordings: Send + Sync {
    /// The path of the recording of each source of `session`, in the order of
    /// `sources`. Makes the folder. A session without `record_to` deletes its
    /// recordings at its end.
    fn paths(
        &self,
        session: u64,
        record_to: Option<&Path>,
        sources: &[Source],
    ) -> Result<Vec<PathBuf>, LiveError>;
}

/// The recordings of Sayso: `<record_to>/<source>.wav`, else a file in a
/// folder for temporary recordings.
pub struct RecordingFolder {
    temp_dir: PathBuf,
}

impl RecordingFolder {
    /// `temp_dir` is the folder of the temporary recordings, for a session without `record_to`.
    pub fn new(temp_dir: PathBuf) -> RecordingFolder {
        RecordingFolder { temp_dir }
    }
}

impl Recordings for RecordingFolder {
    fn paths(
        &self,
        session: u64,
        record_to: Option<&Path>,
        sources: &[Source],
    ) -> Result<Vec<PathBuf>, LiveError> {
        let dir = match record_to {
            Some(dir) => {
                if !dir.is_absolute() {
                    return Err(LiveError::new(
                        Some(ErrorCode::BadRequest),
                        format!("record_to must be an absolute path, not {}", dir.display()),
                    ));
                }
                dir.to_path_buf()
            }
            None => self.temp_dir.clone(),
        };
        std::fs::create_dir_all(&dir).map_err(|e| {
            LiveError::new(
                Some(ErrorCode::BadRequest),
                format!("cannot make the folder {}: {e}", dir.display()),
            )
        })?;
        Ok(sources
            .iter()
            .map(|source| match record_to {
                Some(_) => dir.join(recording_name(source)),
                None => dir.join(format!(
                    "sayso-live-{}-{session}-{}",
                    std::process::id(),
                    recording_name(source)
                )),
            })
            .collect())
    }
}

/// A request of a live session that fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveError {
    pub code: Option<ErrorCode>,
    pub message: String,
}

impl LiveError {
    pub fn new(code: Option<ErrorCode>, message: impl Into<String>) -> LiveError {
        LiveError {
            code,
            message: message.into(),
        }
    }

    fn engine(what: &str, error: EngineError) -> LiveError {
        LiveError {
            code: error.code,
            message: format!("{what}: {}", error.message),
        }
    }

    /// The `error` reply.
    pub fn reply(&self) -> Reply {
        Reply::error(self.code.clone(), self.message.clone())
    }
}

impl std::fmt::Display for LiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// The file name of the recording of a source in `record_to`.
pub fn recording_name(source: &Source) -> String {
    format!("{}.wav", source.as_str())
}

/// The live sessions: at most one at a time, for all apps.
#[derive(Clone)]
pub struct LiveSessions {
    inner: Arc<Inner>,
}

struct Inner {
    deps: LiveDeps,
    slot: Mutex<Slot>,
    /// After [`LiveSessions::close`], no session starts. Changed under the lock of the slot.
    closed: AtomicBool,
    next_session: AtomicU64,
    next_engine_session: AtomicU64,
    observer: Mutex<Option<Arc<dyn LiveObserver>>>,
}

enum Slot {
    Free,
    /// `session_start` opens the captures and the engine sessions, and can wait for
    /// the answer of the user. `cancelled`: its connection closed or Sayso quits,
    /// so the start ends when it can, with nothing left open.
    Starting {
        connection: u64,
        cancelled: bool,
    },
    Running(Box<Running>),
}

struct Running {
    info: LiveInfo,
    owner: Owner,
    /// The audio of the source `client`, while it is open.
    client: Option<Sender<FeedItem>>,
    excludes: Vec<String>,
    /// Held until the stop.
    stopper: Option<Stopper>,
}

/// What a stop needs, taken out of the slot so the stop does not hold its lock.
struct Stopper {
    sources: Vec<SourceRun>,
    /// The leases of the captures. Dropping them ends the audio of the sources `mic` and `system`.
    leases: Vec<Box<dyn Send>>,
    cancel: Arc<AtomicBool>,
    sink: Arc<dyn EventSink>,
    diarization: Option<ModelId>,
    /// The recordings to delete after the stop (no `record_to`).
    temporary: Vec<PathBuf>,
    /// All the recordings, for a start that was cancelled.
    recordings: Vec<PathBuf>,
    /// The echo rule, when the session has both `mic` and `system`.
    echo: Option<Arc<EchoGate>>,
    /// The models of the session, until it ends.
    _models: Box<dyn Send>,
}

impl LiveSessions {
    pub fn new(deps: LiveDeps) -> LiveSessions {
        LiveSessions {
            inner: Arc::new(Inner {
                deps,
                slot: Mutex::new(Slot::Free),
                closed: AtomicBool::new(false),
                next_session: AtomicU64::new(1),
                next_engine_session: AtomicU64::new(ENGINE_SESSION_BASE),
                observer: Mutex::new(None),
            }),
        }
    }

    /// Tell `observer` each time a session starts or ends.
    pub fn set_observer(&self, observer: Arc<dyn LiveObserver>) {
        *self.inner.observer.lock() = Some(observer);
    }

    /// The session that runs now, if any.
    pub fn current(&self) -> Option<LiveInfo> {
        match &*self.inner.slot.lock() {
            Slot::Running(running) => Some(running.info.clone()),
            _ => None,
        }
    }

    /// `session_start`. Gives the id of the session; its events go to `sink`.
    /// `busy` when a session of any app runs. Blocks while the models load, and
    /// while the user answers a question of the system about the capture.
    pub fn start(
        &self,
        owner: Owner,
        spec: SessionSpec,
        sink: Arc<dyn EventSink>,
    ) -> Result<Started, LiveError> {
        check_sources(
            &spec.sources,
            self.inner.deps.microphone.is_some(),
            self.inner.deps.system_audio.is_some(),
        )?;
        {
            let mut slot = self.inner.slot.lock();
            if self.inner.closed.load(Ordering::SeqCst) {
                return Err(start_cancelled());
            }
            if !matches!(*slot, Slot::Free) {
                return Err(LiveError::new(
                    Some(ErrorCode::Busy),
                    "another live session runs, and only one can run at a time",
                ));
            }
            *slot = Slot::Starting {
                connection: owner.connection,
                cancelled: false,
            };
        }
        // Each way out of here without a session frees the slot, also a panic.
        let _free = FreeAfterStart(&self.inner);
        let session = self.inner.next_session.fetch_add(1, Ordering::SeqCst);
        let mut running = self.open(session, &owner, &spec, sink)?;
        let info = running.info.clone();
        let excludes = running.excludes.clone();
        {
            let mut slot = self.inner.slot.lock();
            if !matches!(
                *slot,
                Slot::Starting {
                    cancelled: true,
                    ..
                }
            ) {
                *slot = Slot::Running(Box::new(running));
                drop(slot);
                log::info!(
                    "host: live session {session} of {} started with {:?}",
                    owner.app_id,
                    spec.sources
                );
                self.notify(Some(&info));
                return Ok(Started { session, excludes });
            }
        }
        // The app or Sayso went while this opened. The slot stays taken until the sources closed.
        if let Some(mut stopper) = running.stopper.take() {
            stopper.cancel.store(true, Ordering::SeqCst);
            stopper.temporary = std::mem::take(&mut stopper.recordings);
            let _ = self.finish(session, stopper, true);
        }
        Err(start_cancelled())
    }

    fn start_is_cancelled(&self) -> bool {
        matches!(
            *self.inner.slot.lock(),
            Slot::Starting {
                cancelled: true,
                ..
            }
        )
    }

    /// Open the captures, the engine sessions, and the recordings, and start the sources.
    fn open(
        &self,
        session: u64,
        owner: &Owner,
        spec: &SessionSpec,
        sink: Arc<dyn EventSink>,
    ) -> Result<Running, LiveError> {
        let deps = &self.inner.deps;
        let engine = &deps.engine;
        let mut opened = Opened::new(engine.clone());
        let recordings =
            deps.recordings
                .paths(session, spec.record_to.as_deref(), &spec.sources)?;

        // Hold before the load, so the app does not unload a model between the two.
        let held = deps.models.hold(spec.models());
        let mut models: Vec<&ModelId> = vec![&spec.final_model];
        models.extend(spec.preview_model.as_ref());
        if let VadRun::Model(vad) = &spec.vad {
            models.push(vad);
        }
        for model in models {
            engine
                .load(model)
                .map_err(|e| LiveError::engine(&format!("cannot load {model}"), e))?;
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let clock = Arc::new(SessionClock::new());
        let echo = (spec.sources.contains(&Source::Mic) && spec.sources.contains(&Source::System))
            .then(|| Arc::new(EchoGate::new(session, sink.clone())));
        // The captures first, and the system audio first of them. It can wait for the answer of
        // the user, and the microphone must not run during that wait, when Sayso shows no
        // recording state yet. A refusal stops the start with no microphone and no recording.
        let mut excludes = Vec::new();
        let mut feeds: Vec<(Source, Receiver<FeedItem>)> = Vec::new();
        if spec.sources.contains(&Source::System) {
            let Some(system) = &deps.system_audio else {
                unreachable!("checked in check_sources")
            };
            let feed = system
                .open(owner.pid)
                .map_err(|e| e.live(&Source::System))?;
            opened.leases.push(feed.lease);
            excludes = feed.excludes;
            feeds.push((Source::System, feed.items));
            if self.start_is_cancelled() {
                return Err(start_cancelled());
            }
        }
        if spec.sources.contains(&Source::Mic) {
            let Some(microphone) = &deps.microphone else {
                unreachable!("checked in check_sources")
            };
            let feed = microphone.open().map_err(|e| e.live(&Source::Mic))?;
            opened.leases.push(feed.lease);
            feeds.push((Source::Mic, feed.items));
        }
        let silence = spec.silence_limit.map(|limit| {
            SilenceWatch::new(
                limit,
                spec.sources.len(),
                self.on_silence_limit(session, limit, sink.clone()),
            )
        });
        for (index, (source, path)) in spec.sources.iter().zip(&recordings).enumerate() {
            // Watch before the engine sessions open, so no event is lost.
            let events = engine.events();
            let preview = match &spec.preview_model {
                Some(model) => {
                    let id = self.engine_session();
                    engine
                        .start_stream(id, model, &spec.language)
                        .map_err(|e| LiveError::engine("cannot open the preview stream", e))?;
                    opened.streams.push(id);
                    Some(id)
                }
                None => None,
            };
            let vad = match &spec.vad {
                VadRun::Basic => SourceVad::Basic,
                VadRun::Model(model) => {
                    let id = self.engine_session();
                    engine
                        .start_vad(id, model)
                        .map_err(|e| LiveError::engine("cannot open the VAD session", e))?;
                    opened.vads.push(id);
                    SourceVad::Engine { session: id }
                }
            };
            let wav = WavWriter::create(path).map_err(|e| {
                LiveError::new(
                    Some(ErrorCode::BadRequest),
                    format!("cannot write {}: {e}", path.display()),
                )
            })?;
            opened.written.push(path.clone());
            let audio = match feeds.iter().position(|(captured, _)| captured == source) {
                Some(at) => feeds.swap_remove(at).1,
                None => {
                    let (tx, rx) = unbounded();
                    opened.client = Some(tx);
                    rx
                }
            };
            let role = echo.as_ref().and_then(|gate| match source {
                Source::Mic => Some(EchoRole::Mic(gate.clone())),
                Source::System => Some(EchoRole::System(gate.clone())),
                _ => None,
            });
            let plan = SourcePlan {
                session,
                source: source.clone(),
                final_model: spec.final_model.clone(),
                language: spec.language.clone(),
                preview,
                vad,
                wav,
                clock: clock.clone(),
                echo: role,
                silence: silence.clone().map(|watch| (watch, index)),
            };
            let run = SourceRun::start(
                plan,
                audio,
                events,
                engine.clone(),
                sink.clone(),
                deps.gate.clone(),
                cancel.clone(),
            )
            .map_err(|e| LiveError::new(None, format!("cannot start a thread: {e}")))?;
            // The source ends its engine sessions now.
            opened.streams.clear();
            opened.vads.clear();
            opened.sources.push(run);
        }
        let captures_mic = spec.sources.contains(&Source::Mic);
        let kept = opened.keep();
        Ok(Running {
            info: LiveInfo {
                session,
                app_id: owner.app_id.clone(),
                sources: spec.sources.clone(),
                captures_mic,
                stopping: false,
            },
            owner: owner.clone(),
            client: kept.client,
            excludes,
            stopper: Some(Stopper {
                sources: kept.sources,
                leases: kept.leases,
                cancel,
                sink,
                diarization: spec.diarization.clone(),
                temporary: if spec.record_to.is_none() {
                    recordings.clone()
                } else {
                    Vec::new()
                },
                recordings,
                echo,
                _models: held,
            }),
        })
    }

    /// What runs when the silence limit of `session` passes: end the audio of
    /// the sources, and send the event. Gives false while the session still
    /// starts, so the watch asks again at the next audio. `session_stop` ends
    /// the session as usual, and the last parts come before its reply.
    fn on_silence_limit(
        &self,
        session: u64,
        limit: Duration,
        sink: Arc<dyn EventSink>,
    ) -> Box<dyn Fn() -> bool + Send + Sync> {
        let inner = Arc::downgrade(&self.inner);
        Box::new(move || {
            let Some(inner) = inner.upgrade() else {
                return true;
            };
            let mut slot = inner.slot.lock();
            match &mut *slot {
                Slot::Starting { .. } => return false,
                Slot::Running(running) if running.info.session == session => {
                    running.client = None;
                    if let Some(stopper) = &mut running.stopper {
                        stopper.leases.clear();
                    }
                }
                _ => return true,
            }
            drop(slot);
            log::info!(
                "host: live session {session}: no speech for {} ms, so its audio stops",
                limit.as_millis()
            );
            sink.event(Event::SilenceLimit {
                session,
                silence_ms: limit.as_millis() as u64,
            });
            true
        })
    }

    fn engine_session(&self) -> u64 {
        self.inner
            .next_engine_session
            .fetch_add(1, Ordering::SeqCst)
    }

    /// `session_audio`: PCM for the source `client` (base64 of 16 kHz mono
    /// 16-bit samples).
    pub fn audio(&self, connection: u64, session: u64, pcm: &str) -> Result<(), LiveError> {
        let samples = sayso_protocol::pcm::decode_f32(pcm)
            .map_err(|e| LiveError::new(Some(ErrorCode::BadRequest), e.to_string()))?;
        let slot = self.inner.slot.lock();
        let running = owned(&slot, connection, session)?;
        let Some(client) = &running.client else {
            let message = if running.info.stopping {
                "the live session stops"
            } else {
                "the live session has no source client"
            };
            return Err(LiveError::new(Some(ErrorCode::BadRequest), message));
        };
        // The receiver lives until the stop closes this sender.
        let _ = client.send(FeedItem::Audio(CapturedAudio::untimed(samples)));
        Ok(())
    }

    /// `session_stop`: end the audio, wait for the last parts, diarize, and
    /// give the text of the session. The `segment` and `speakers` events go to
    /// the sink of the session before this returns. Blocks.
    pub fn stop(&self, connection: u64, session: u64) -> Result<String, LiveError> {
        let stopper = {
            let mut slot = self.inner.slot.lock();
            let Slot::Running(running) = &mut *slot else {
                return Err(unknown_session(session));
            };
            if running.info.session != session || running.owner.connection != connection {
                return Err(unknown_session(session));
            }
            let Some(stopper) = running.stopper.take() else {
                return Err(LiveError::new(
                    Some(ErrorCode::BadRequest),
                    "the live session stops already",
                ));
            };
            running.client = None;
            running.info.stopping = true;
            stopper
        };
        if let Some(info) = self.current() {
            self.notify(Some(&info));
        }
        let result = self.finish(session, stopper, false);
        self.free(session);
        result
    }

    /// The user stopped the session in the app: **Stop** in the popover, or a
    /// grant that the user took back. It ends as [`LiveSessions::stop`] does,
    /// with no reply to wait for: `Event::SessionEnded` is its end on the sink.
    /// Returns at once. False when `session` no longer runs or stops already.
    pub fn stop_for_user(&self, session: u64) -> bool {
        let stopper = {
            let mut slot = self.inner.slot.lock();
            let Slot::Running(running) = &mut *slot else {
                return false;
            };
            if running.info.session != session {
                return false;
            }
            let Some(stopper) = running.stopper.take() else {
                return false;
            };
            running.client = None;
            running.info.stopping = true;
            stopper
        };
        if let Some(info) = self.current() {
            self.notify(Some(&info));
        }
        let sink = stopper.sink.clone();
        let sessions = self.clone();
        let spawned = std::thread::Builder::new()
            .name("sayso-host-live-stop".into())
            .spawn(move || {
                let text = sessions
                    .finish(session, stopper, false)
                    .unwrap_or_else(|e| {
                        log::warn!(
                            "host: the stop of live session {session} failed: {}",
                            e.message
                        );
                        String::new()
                    });
                sessions.free(session);
                sink.event(Event::SessionEnded {
                    session,
                    text,
                    code: None,
                });
            });
        if let Err(e) = spawned {
            log::warn!("host: cannot stop live session {session}: {e}");
        }
        true
    }

    /// The connection closed: stop its session without diarization, and
    /// forget the parts that wait. Returns at once; the session ends on a
    /// thread of its own, and the slot is free after that.
    pub fn connection_closed(&self, connection: u64) {
        self.cancel(Some(connection));
    }

    /// Sayso quits: stop the session that runs as [`LiveSessions::connection_closed`] does.
    /// No session starts after this.
    pub fn close(&self) {
        self.cancel(None);
    }

    /// Cancel the session of `connection`, or any session. A session in its start cannot
    /// stop now, for example during a question of the system. It ends when the question ends.
    fn cancel(&self, connection: Option<u64>) {
        let taken = {
            let mut slot = self.inner.slot.lock();
            if connection.is_none() {
                self.inner.closed.store(true, Ordering::SeqCst);
            }
            match &mut *slot {
                Slot::Starting {
                    connection: owner,
                    cancelled,
                } if connection.is_none_or(|c| *owner == c) => {
                    *cancelled = true;
                    None
                }
                Slot::Running(running)
                    if connection.is_none_or(|c| running.owner.connection == c) =>
                {
                    running.client = None;
                    running.info.stopping = true;
                    running
                        .stopper
                        .take()
                        .map(|stopper| (running.info.session, stopper))
                }
                _ => None,
            }
        };
        let Some((session, stopper)) = taken else {
            return;
        };
        log::info!("host: live session {session} is cancelled: its connection or Sayso closes");
        stopper.cancel.store(true, Ordering::SeqCst);
        let sessions = self.clone();
        let spawned = std::thread::Builder::new()
            .name("sayso-host-live-stop".into())
            .spawn(move || {
                let _ = sessions.finish(session, stopper, true);
                sessions.free(session);
            });
        if let Err(e) = spawned {
            log::warn!("host: cannot stop live session {session}: {e}");
        }
    }

    fn finish(&self, session: u64, stopper: Stopper, cancelled: bool) -> Result<String, LiveError> {
        let Stopper {
            sources,
            leases,
            cancel: _,
            sink,
            diarization,
            temporary,
            recordings: _,
            echo,
            _models,
        } = stopper;
        // The audio of the captures ends here, and the client sender went with the slot.
        drop(leases);
        let mut results: Vec<SourceResult> =
            sources.into_iter().filter_map(SourceRun::join).collect();
        if let Some(echo) = echo {
            // The rule sends what still waits, and it has the segments that stayed.
            echo.system_ended();
            if let Some(mic) = results
                .iter_mut()
                .find(|result| result.source == Source::Mic)
            {
                mic.segments = echo.kept();
            }
        }
        let outcome = if cancelled {
            Ok(String::new())
        } else {
            self.speakers_and_text(session, &results, diarization.as_ref(), &*sink)
        };
        for path in temporary {
            if let Err(e) = std::fs::remove_file(&path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                log::warn!("host: cannot delete {}: {e}", path.display());
            }
        }
        log::info!("host: live session {session} ended");
        outcome
    }

    fn speakers_and_text(
        &self,
        session: u64,
        results: &[SourceResult],
        diarization: Option<&ModelId>,
        sink: &dyn EventSink,
    ) -> Result<String, LiveError> {
        if let Some(error) = results.iter().find_map(|r| r.wav_error.clone()) {
            return Err(LiveError::new(None, error));
        }
        if let Some(model) = diarization {
            let engine = &self.inner.deps.engine;
            for result in results {
                self.inner.deps.gate.wait_idle(&|| false);
                engine
                    .load(model)
                    .map_err(|e| LiveError::engine(&format!("cannot load {model}"), e))?;
                let samples =
                    wav::read(&result.wav).map_err(|e| LiveError::new(None, e.to_string()))?;
                let call = self.inner.deps.gate.engine_call();
                let found = engine
                    .diarize(model, &samples, None)
                    .map_err(|e| LiveError::engine("diarization failed", e))?;
                drop(call);
                // The rules of jobs: a session has no number of speakers, so small speakers fold.
                let found = speakers::fold_small_speakers(&found);
                let times: Vec<(u64, u64)> = result
                    .segments
                    .iter()
                    .map(|s| (s.start_ms, s.end_ms))
                    .collect();
                let turns = speakers::segment_turns(&times, &speakers::assign(&times, &found));
                sink.event(Event::Speakers(SpeakersEvent {
                    job: None,
                    session: Some(session),
                    source: Some(result.source.clone()),
                    turns,
                }));
            }
        }
        Ok(session_text(results))
    }

    fn free(&self, session: u64) {
        let mut slot = self.inner.slot.lock();
        if matches!(&*slot, Slot::Running(running) if running.info.session == session) {
            *slot = Slot::Free;
            drop(slot);
            self.notify(None);
        }
    }

    fn notify(&self, info: Option<&LiveInfo>) {
        let observer = self.inner.observer.lock().clone();
        if let Some(observer) = observer {
            observer.changed(info);
        }
    }
}

fn owned(slot: &Slot, connection: u64, session: u64) -> Result<&Running, LiveError> {
    match slot {
        Slot::Running(running)
            if running.info.session == session && running.owner.connection == connection =>
        {
            Ok(running)
        }
        _ => Err(unknown_session(session)),
    }
}

/// Frees the slot of a start that has no session, after each other lock of the start.
struct FreeAfterStart<'a>(&'a Inner);

impl Drop for FreeAfterStart<'_> {
    fn drop(&mut self) {
        let mut slot = self.0.slot.lock();
        if matches!(*slot, Slot::Starting { .. }) {
            *slot = Slot::Free;
        }
    }
}

fn start_cancelled() -> LiveError {
    LiveError::new(
        None,
        "the start of the live session was cancelled, because the app or the program that runs it closed",
    )
}

fn unknown_session(session: u64) -> LiveError {
    LiveError::new(
        Some(ErrorCode::BadRequest),
        format!("no live session {session} of this connection runs"),
    )
}

/// The sources that this version can capture: `mic` and `system` (when the
/// app can capture them) and `client`, each at most once.
fn check_sources(
    sources: &[Source],
    has_microphone: bool,
    has_system_audio: bool,
) -> Result<(), LiveError> {
    if sources.is_empty() {
        return Err(LiveError::new(
            Some(ErrorCode::BadRequest),
            "a live session needs at least one source",
        ));
    }
    for (i, source) in sources.iter().enumerate() {
        if sources[..i].contains(source) {
            return Err(LiveError::new(
                Some(ErrorCode::BadRequest),
                format!("the source {source} is in the list twice"),
            ));
        }
        match source {
            Source::Mic if has_microphone => {}
            Source::System if has_system_audio => {}
            Source::Client => {}
            Source::Mic | Source::System => {
                return Err(LiveError::new(
                    Some(ErrorCode::Unsupported),
                    format!("this program cannot capture the source {source}"),
                ));
            }
            Source::Other(other) => {
                return Err(LiveError::new(
                    Some(ErrorCode::BadRequest),
                    format!("unknown source {other}"),
                ));
            }
        }
    }
    Ok(())
}

/// What a start opened so far. A start that fails closes it all on drop: the
/// sources that run end, the engine sessions that no source owns yet close, and
/// the recordings that it made go.
struct Opened {
    engine: Arc<dyn SpeechEngine>,
    sources: Vec<SourceRun>,
    client: Option<Sender<FeedItem>>,
    streams: Vec<u64>,
    vads: Vec<u64>,
    leases: Vec<Box<dyn Send>>,
    written: Vec<PathBuf>,
    kept: bool,
}

/// What a session keeps of a start that worked.
struct Kept {
    sources: Vec<SourceRun>,
    client: Option<Sender<FeedItem>>,
    leases: Vec<Box<dyn Send>>,
}

impl Opened {
    fn new(engine: Arc<dyn SpeechEngine>) -> Opened {
        Opened {
            engine,
            sources: Vec::new(),
            client: None,
            streams: Vec::new(),
            vads: Vec::new(),
            leases: Vec::new(),
            written: Vec::new(),
            kept: false,
        }
    }

    fn keep(mut self) -> Kept {
        self.kept = true;
        Kept {
            sources: std::mem::take(&mut self.sources),
            client: self.client.take(),
            leases: std::mem::take(&mut self.leases),
        }
    }
}

impl Drop for Opened {
    fn drop(&mut self) {
        if self.kept {
            return;
        }
        // The audio ends, so the sources that run end too.
        self.client = None;
        self.leases.clear();
        for source in self.sources.drain(..) {
            let _ = source.join();
        }
        for &id in &self.streams {
            self.engine.end_stream(id);
        }
        for &id in &self.vads {
            let _ = self.engine.end_vad(id);
        }
        for path in &self.written {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The text of the session: the segments of all sources in the order of time.
fn session_text(results: &[SourceResult]) -> String {
    let mut segments: Vec<(u64, usize, &str)> = results
        .iter()
        .enumerate()
        .flat_map(|(i, r)| {
            r.segments
                .iter()
                .map(move |s| (s.start_ms, i, s.text.trim()))
        })
        .collect();
    segments.sort_by_key(|&(start, source, _)| (start, source));
    segments
        .into_iter()
        .map(|(_, _, text)| text)
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
