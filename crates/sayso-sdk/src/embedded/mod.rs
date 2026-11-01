//! A session in the process of the app, with an engine layer of the app.
//!
//! [`crate::Speech`] is the API for most apps. This one is for an app that has
//! to wrap the engine, as Consort does with [`crate::deepgram::DeepgramFinal`],
//! and that keeps its models itself. It never talks to Sayso. The calls block.
//! Run them on a thread that may wait.

mod engine;
mod mic;

pub use engine::{EngineModels, ModelInfo, ProgramEngine};
pub use mic::{CaptureMicrophone, LevelFn};
pub use sayso_capture::mic::{Device, devices};
pub use sayso_capture::permission;
pub use sayso_protocol::{
    DownloadProgress, ModelState, PartialEvent, Role, Segment, SegmentEvent, Source,
};
pub use sayso_speech::live::{AudioFeed, CapturedAudio, FeedItem, Microphone, OpenError};
pub use sayso_speech::{EngineError, EngineEvent, ModelId, SpeechEngine, SpeechPart, Turn, VadRun};

use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::Mutex;
use sayso_protocol::ErrorCode;
use sayso_speech::live::{LiveDeps, LiveError, LiveSessions, Owner, RecordingFolder};
use sayso_speech::{EventSink, ModelRetention, NoGate, SessionSpec};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// How the SDK does the work. `Sayso` and `Auto` come later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Mode {
    /// The SDK starts its own engine and never talks to Sayso.
    Local,
}

/// The app that uses the SDK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub name: String,
}

impl App {
    pub fn new(name: impl Into<String>) -> App {
        App { name: name.into() }
    }
}

/// Where the engine comes from.
#[derive(Clone)]
pub enum EngineSource {
    /// The engine that the app ships (`sayso_engine_sdk::find_engine`). It starts at the first request that needs it.
    Program(Arc<ProgramEngine>),
    /// An engine of the app. Tests use this for a fake engine.
    Custom {
        engine: Arc<dyn SpeechEngine>,
        models: Arc<dyn EngineModels>,
    },
}

impl EngineSource {
    /// The engine at `program`, with the shared model store of this user
    /// ([`crate::model_store`]), and `models_dir` for an engine without the
    /// store and for a model without a descriptor.
    pub fn program(program: impl Into<PathBuf>, models_dir: impl Into<PathBuf>) -> EngineSource {
        EngineSource::Program(Arc::new(ProgramEngine::new(
            program.into(),
            models_dir.into(),
            Some(crate::model_store()),
        )))
    }
}

pub struct Options {
    pub mode: Mode,
    pub app: App,
    pub engine: EngineSource,
    /// A folder for the temporary recordings of live sessions. The SDK makes it.
    pub temp_dir: PathBuf,
    /// None: [`CaptureMicrophone`] with the default device.
    pub microphone: Option<Arc<dyn Microphone>>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Engine(EngineError),
    #[error("{message}")]
    Session {
        code: Option<ErrorCode>,
        message: String,
    },
    #[error("the file system refused: {0}")]
    Io(#[from] std::io::Error),
}

impl Error {
    /// The code of the engine or the session, when the error has one.
    pub fn code(&self) -> Option<&ErrorCode> {
        match self {
            Error::Engine(e) => e.code.as_ref(),
            Error::Session { code, .. } => code.as_ref(),
            Error::Io(_) => None,
        }
    }
}

impl From<LiveError> for Error {
    fn from(error: LiveError) -> Error {
        Error::Session {
            code: error.code,
            message: error.message,
        }
    }
}

impl From<EngineError> for Error {
    fn from(error: EngineError) -> Error {
        Error::Engine(error)
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// What comes on [`Speech::events`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Event {
    /// The text of a preview stream so far.
    Partial(PartialEvent),
    /// The final text of a part of the audio.
    Segment(SegmentEvent),
    /// The silence limit of a session stopped its audio. The last parts still
    /// come as [`Event::Segment`]. End the session with [`Speech::stop_session`].
    SilenceLimit { session: u64, silence_ms: u64 },
}

/// One live session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionOptions {
    pub sources: Vec<Source>,
    /// The model of the final pass of each part.
    pub final_model: ModelId,
    /// The streaming model of the `partial` events. None: no preview.
    pub preview_model: Option<ModelId>,
    pub vad: VadRun,
    /// A code such as `en`, or `auto`.
    pub language: String,
    /// Stop the audio after this long with no speech. None: no limit.
    pub silence_limit: Option<Duration>,
}

impl SessionOptions {
    /// A session from the microphone with one model for the final pass, Basic
    /// VAD, no preview, and no silence limit.
    pub fn mic(final_model: impl Into<String>) -> SessionOptions {
        SessionOptions {
            sources: vec![Source::Mic],
            final_model: ModelId::new(final_model),
            preview_model: None,
            vad: VadRun::Basic,
            language: "auto".into(),
            silence_limit: None,
        }
    }
}

/// The one owner of the sessions of this SDK. Sayso uses this for the connection of an app.
const LOCAL_CONNECTION: u64 = 1;

/// Recordings older than this come from an app that crashed.
const STALE_RECORDING: Duration = Duration::from_secs(24 * 60 * 60);

pub struct Speech {
    mode: Mode,
    app: App,
    models: Arc<dyn EngineModels>,
    live: LiveSessions,
    events: Receiver<Event>,
    sink: Arc<ChannelSink>,
}

impl Speech {
    /// Get ready. In local mode this starts no engine and loads no model.
    pub fn connect(options: Options) -> Result<Speech> {
        Speech::connect_with(options, |engine| engine)
    }

    /// As [`Speech::connect`], and put a layer around the engine of the
    /// sessions, for example [`deepgram::DeepgramFinal`].
    pub fn connect_with(
        options: Options,
        layer: impl FnOnce(Arc<dyn SpeechEngine>) -> Arc<dyn SpeechEngine>,
    ) -> Result<Speech> {
        let Options {
            mode,
            app,
            engine,
            temp_dir,
            microphone,
        } = options;
        std::fs::create_dir_all(&temp_dir)?;
        remove_stale_recordings(&temp_dir);
        let (speech, models): (Arc<dyn SpeechEngine>, Arc<dyn EngineModels>) = match engine {
            EngineSource::Program(program) => (program.clone(), program),
            EngineSource::Custom { engine, models } => (engine, models),
        };
        let microphone = microphone.unwrap_or_else(|| Arc::new(CaptureMicrophone::default()));
        let live = LiveSessions::new(LiveDeps {
            engine: layer(speech),
            microphone: Some(microphone),
            system_audio: None,
            gate: Arc::new(NoGate),
            models: Arc::new(NoRetention),
            recordings: Arc::new(RecordingFolder::new(temp_dir)),
        });
        let (tx, events) = unbounded();
        Ok(Speech {
            mode,
            app,
            models,
            live,
            events,
            sink: Arc::new(ChannelSink {
                tx,
                partials: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// The mode of the work now.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The models of the engine and their state on disk. Starts the engine.
    pub fn models(&self) -> Result<Vec<ModelInfo>> {
        Ok(self.models.catalog()?)
    }

    /// Download a model at once, and call `on_progress` on this thread. The
    /// app asks its own user first. `size_bytes` helps an engine that knows only a fraction.
    pub fn download(
        &self,
        model: &str,
        size_bytes: Option<u64>,
        mut on_progress: impl FnMut(&DownloadProgress),
    ) -> Result<()> {
        Ok(self.models.download(model, size_bytes, &mut on_progress)?)
    }

    /// Delete the files of a model.
    pub fn delete(&self, model: &str) -> Result<()> {
        Ok(self.models.delete(model)?)
    }

    /// Read a model into memory. Blocks while it loads.
    pub fn load(&self, model: &str) -> Result<()> {
        Ok(self.models.load(model)?)
    }

    /// Take a model out of memory.
    pub fn unload(&self, model: &str) -> Result<()> {
        Ok(self.models.unload(model)?)
    }

    /// The events of the sessions. There is one queue, so read it from one place.
    pub fn events(&self) -> Receiver<Event> {
        self.events.clone()
    }

    /// Start a live session. Blocks while its models load. At most one session runs at a time.
    pub fn start_session(&self, options: SessionOptions) -> Result<u64> {
        let spec = SessionSpec {
            sources: options.sources,
            final_model: options.final_model,
            preview_model: options.preview_model,
            vad: options.vad,
            diarization: None,
            language: options.language,
            record_to: None,
            silence_limit: options.silence_limit,
        };
        let owner = Owner {
            connection: LOCAL_CONNECTION,
            app_id: self.app.name.clone(),
            pid: None,
        };
        Ok(self.live.start(owner, spec, self.sink.clone())?.session)
    }

    /// Audio of the source `client`: 16 kHz mono samples from -1 to 1.
    pub fn session_audio(&self, session: u64, samples: &[f32]) -> Result<()> {
        let pcm = sayso_protocol::pcm::encode_f32(samples);
        Ok(self.live.audio(LOCAL_CONNECTION, session, &pcm)?)
    }

    /// End the audio, wait for the last parts, and give the text of the session.
    /// The `segment` events come before this returns.
    pub fn stop_session(&self, session: u64) -> Result<String> {
        Ok(self.live.stop(LOCAL_CONNECTION, session)?)
    }

    /// Stop the session and forget the parts that wait. Returns at once.
    pub fn cancel_session(&self) {
        self.live.connection_closed(LOCAL_CONNECTION);
    }
}

/// The events of sessions go to the channel of the app.
struct ChannelSink {
    tx: Sender<Event>,
    /// The last partial of each stream, so a repeat does not reach the app.
    partials: Mutex<HashMap<u64, (String, String)>>,
}

impl EventSink for ChannelSink {
    fn event(&self, event: sayso_protocol::Event) {
        let event = match event {
            sayso_protocol::Event::Partial(partial) => {
                let text = (partial.committed.clone(), partial.tentative.clone());
                if self
                    .partials
                    .lock()
                    .insert(partial.session, text.clone())
                    .as_ref()
                    == Some(&text)
                {
                    return;
                }
                Event::Partial(partial)
            }
            sayso_protocol::Event::Segment(segment) => Event::Segment(segment),
            sayso_protocol::Event::SilenceLimit {
                session,
                silence_ms,
            } => Event::SilenceLimit {
                session,
                silence_ms,
            },
            _ => return,
        };
        // The app may have dropped its receiver. Then nobody wants the event.
        let _ = self.tx.send(event);
    }
}

/// Local mode holds no models for other work: the app loads and unloads them.
struct NoRetention;

impl ModelRetention for NoRetention {
    fn hold(&self, _models: Vec<ModelId>) -> Box<dyn Send> {
        Box::new(())
    }
}

/// A crash leaves its recording behind. A recording is private, so it does not stay.
fn remove_stale_recordings(folder: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("sayso-live-") {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|at| SystemTime::now().duration_since(at).ok())
            .is_some_and(|age| age > STALE_RECORDING);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeated_partial_does_not_reach_the_app() {
        let (tx, rx) = unbounded();
        let sink = ChannelSink {
            tx,
            partials: Mutex::new(HashMap::new()),
        };
        let partial = |text: &str| {
            sayso_protocol::Event::Partial(PartialEvent {
                session: 1,
                source: Some(Source::Mic),
                committed: String::new(),
                tentative: text.into(),
            })
        };
        sink.event(partial("hello"));
        sink.event(partial("hello"));
        sink.event(partial("hello there"));
        assert_eq!(rx.try_iter().count(), 2);
    }

    #[test]
    fn a_recent_recording_stays() {
        let folder = tempfile::tempdir().unwrap();
        let recent = folder.path().join("sayso-live-1-1-mic.wav");
        std::fs::write(&recent, b"x").unwrap();
        remove_stale_recordings(folder.path());
        assert!(recent.exists());
    }
}
