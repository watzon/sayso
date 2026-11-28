//! Use the speech features of Sayso from your own app (`docs/sdk.md`).
//!
//! The app calls one API. When Sayso runs, the SDK uses Sayso: its engine, its
//! loaded models, its microphone, and its scheduler. When Sayso does not run, the SDK
//! starts the engine that the app ships, and does the work in the process of the app.
//! The calls block, and the events come on a channel, so you need no async runtime.
//!
//! ```no_run
//! # #[cfg(feature = "local")]
//! # fn example() -> Result<(), sayso_sdk::Error> {
//! use sayso_sdk::protocol::{Capability, TranscribeFile};
//! use sayso_sdk::{App, Engine, Event, Mode, Options, Speech};
//!
//! let options = Options::new(Mode::Auto, App::new("Notetaker"))
//!     .capabilities([Capability::TranscribeFile])
//!     .engine(Engine::from_release("resources/engine"));
//! let speech = Speech::connect(options)?;
//! println!("mode: {:?}", speech.mode());
//! let job = speech.transcribe_file(TranscribeFile { path: "/tmp/talk.wav".into(), ..Default::default() })?;
//! for event in speech.events() {
//!     match event {
//!         Event::Message(sayso_sdk::protocol::Event::JobDone { job: done, text }) if done == job => {
//!             println!("{text}");
//!             break;
//!         }
//!         Event::Message(sayso_sdk::protocol::Event::JobFailed { job: failed, message, .. }) if failed == job => {
//!             eprintln!("{message}");
//!             break;
//!         }
//!         _ => {}
//!     }
//! }
//! # Ok(())
//! # }
//! # fn main() {}
//! ```
//!
//! Without the feature `local`, only Sayso mode exists:
//!
//! ```no_run
//! use sayso_sdk::protocol::{Capability, ModelState};
//! use sayso_sdk::{App, Mode, Options, Speech};
//!
//! let options = Options::new(Mode::Sayso { start: false }, App::new("Notetaker")).capabilities([Capability::ModelsRead]);
//! let speech = Speech::connect(options)?;
//! let ready = speech.list_models(None)?.into_iter().filter(|m| m.state == ModelState::Ready).count();
//! println!("{ready} models are loaded in Sayso");
//! # Ok::<(), sayso_sdk::Error>(())
//! ```
//!
//! # Rules to know
//!
//! - Both modes use the messages of [`protocol`] and the same end rules: each job ends with
//!   one `job_done` or `job_failed`, each live session with one end, and each request with
//!   one reply. A session ends with the reply of [`Speech::stop_session`], or else with the
//!   event `session_ended`.
//! - The order and the contents of events can differ between modes
//!   (`docs/sdk.md`, "What differs between modes").
//! - The ids of jobs and sessions come from the SDK, and no id comes back after a change of mode.
//! - `Mode::Auto` changes mode only when the app has no active work. A change never moves work.
//! - The SDK never sends a request again by itself. After a lost connection, a request that
//!   had no reply gets the code `connection_lost`.
//! - [`Speech::start_session`] with the source `system` can wait for the answer of the user to a
//!   question of macOS, in local mode. Call it on a thread that can wait. The app needs
//!   `NSAudioCaptureUsageDescription` in its `Info.plist`.
//! - [`Speech::connect`] waits for the answer of the user when Sayso asks about the app for
//!   the first time. Call it on a thread that can wait.

mod api;
pub mod audio;
#[cfg(feature = "local")]
pub mod capture;
#[cfg(feature = "cleanup")]
pub mod cleanup;
pub mod client;
#[cfg(feature = "deepgram")]
pub mod deepgram;
#[cfg(feature = "local")]
pub mod embedded;
pub mod engine;
pub mod error;
#[cfg(feature = "local")]
mod local;
#[cfg(feature = "local")]
mod local_engine;
#[cfg(feature = "local")]
mod local_models;
pub mod paths;
pub mod protocol;
#[cfg(feature = "local")]
pub mod speech;
pub mod start;
mod transport;
mod wire;

pub use api::Speech;
#[cfg(feature = "capture")]
pub use capture::permission;
pub use engine::find_engine;
pub use error::Error;
pub use paths::model_store;
pub use protocol::Fields;

use protocol::{Capability, ErrorReply};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// The mode that an app asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Use Sayso, and fail when that is not possible. With `start`, start Sayso when it does
    /// not run. Sayso has hotkeys and a menu bar icon, so only do this when the user asked for it.
    Sayso { start: bool },
    /// Use the engine of the app. Never talk to Sayso. Needs the feature `local`.
    #[cfg(feature = "local")]
    Local,
    /// Use Sayso when it runs, else the engine of the app. Never starts Sayso. Needs the feature `local`.
    #[cfg(feature = "local")]
    Auto,
}

/// The mode that a [`Speech`] uses now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveMode {
    Sayso,
    #[cfg(feature = "local")]
    Local,
}

/// What the app says about itself.
#[derive(Debug, Clone, Default)]
pub struct App {
    /// Sayso shows it as a claim of the app, after the name from the system.
    pub name: String,
}

impl App {
    pub fn new(name: impl Into<String>) -> App {
        App { name: name.into() }
    }
}

/// The engine that the app ships, for local mode (`docs/sdk.md`, decision 9). The SDK
/// does not use the engine inside an installed Sayso, because an update of Sayso would
/// then change the engine of the app.
#[cfg(feature = "local")]
#[derive(Debug, Clone)]
pub struct Engine {
    source: EngineSource,
    store: Option<PathBuf>,
    models_dir: Option<PathBuf>,
}

#[cfg(feature = "local")]
#[derive(Debug, Clone)]
enum EngineSource {
    Release(PathBuf),
    Program(PathBuf),
}

#[cfg(feature = "local")]
impl Engine {
    /// The folder where the app unpacked the release file of the engine.
    pub fn from_release(folder: impl Into<PathBuf>) -> Engine {
        Engine {
            source: EngineSource::Release(folder.into()),
            store: None,
            models_dir: None,
        }
    }

    /// The engine program itself.
    pub fn from_program(program: impl Into<PathBuf>) -> Engine {
        Engine {
            source: EngineSource::Program(program.into()),
            store: None,
            models_dir: None,
        }
    }

    /// Use another model store than [`model_store`].
    pub fn store(mut self, store: impl Into<PathBuf>) -> Engine {
        self.store = Some(store.into());
        self
    }

    /// The folder for the models of an engine without the store, and for a model without a
    /// descriptor. The default is a folder with the name of the app in the data folder of the system.
    pub fn models_dir(mut self, folder: impl Into<PathBuf>) -> Engine {
        self.models_dir = Some(folder.into());
        self
    }

    #[cfg(feature = "local")]
    fn program(&self) -> io::Result<PathBuf> {
        match &self.source {
            EngineSource::Release(folder) => crate::engine::find_engine(folder),
            EngineSource::Program(program) if program.is_file() => Ok(program.clone()),
            EngineSource::Program(program) => Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("there is no engine at {}", program.display()),
            )),
        }
    }
}

/// The models of the app, for a request without a model in local mode. A field that
/// is `None` takes the first suitable model on disk. In Sayso mode, a request without
/// a model gets the choice of the user, not these.
#[cfg(feature = "local")]
#[derive(Debug, Clone, Default)]
pub struct ModelChoices {
    pub final_model: Option<String>,
    pub preview_model: Option<String>,
    /// A model id, or `basic` for Basic VAD.
    pub vad_model: Option<String>,
    pub diarization_model: Option<String>,
    /// A code such as `en`. The default is `auto`.
    pub language: Option<String>,
}

/// The times of the SDK. The defaults are the times of `docs/sdk.md`.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// How often local mode looks for the socket of Sayso.
    pub watch: Duration,
    /// After a `hello` that failed, local mode tries again after this time, or when the socket changes.
    pub retry: Duration,
    /// How long [`Mode::Sayso`] with `start` waits for Sayso.
    pub start_wait: Duration,
    /// How long the first reply to `hello` can take, unless Sayso said that it asks the user.
    pub hello_limit: Duration,
    /// How long local mode keeps a model in memory after the last work that used it.
    pub keep_model: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing {
            watch: Duration::from_millis(200),
            retry: Duration::from_secs(30),
            start_wait: client::START_WAIT,
            hello_limit: Duration::from_secs(10),
            keep_model: Duration::from_secs(60),
        }
    }
}

/// The sources of local mode instead of the real capture: a test, or an app with its own capture.
#[cfg(feature = "local")]
#[derive(Clone)]
pub struct CaptureSources {
    pub microphone: Arc<dyn capture::Microphone>,
    pub system_audio: Arc<dyn capture::SystemAudio>,
}

/// What [`Speech::connect`] needs.
#[derive(Clone)]
pub struct Options {
    pub mode: Mode,
    pub app: App,
    /// What the app wants to use in Sayso. The approval question lists them.
    pub capabilities: Vec<Capability>,
    /// Needed for `Mode::Local` and `Mode::Auto`.
    #[cfg(feature = "local")]
    pub engine: Option<Engine>,
    #[cfg(feature = "local")]
    pub models: ModelChoices,
    /// The socket of Sayso. The default is the rule of [`protocol::socket`].
    pub socket: Option<PathBuf>,
    pub timing: Timing,
    /// How [`Mode::Sayso`] with `start` starts Sayso. The default is [`client::start_sayso`].
    pub start: Arc<dyn Fn() -> io::Result<()> + Send + Sync>,
    /// Called once when Sayso asks the user about this app and [`Speech::connect`] waits
    /// for the answer. It runs on the thread of the connection: do not block in it.
    pub on_approval_waiting: Option<Arc<dyn Fn() + Send + Sync>>,
    /// None: the microphone and the system audio of `sayso-capture`, in the process of the app.
    /// Sayso mode does not use this, because Sayso captures there.
    #[cfg(feature = "local")]
    pub capture: Option<CaptureSources>,
}

impl Options {
    pub fn new(mode: Mode, app: App) -> Options {
        Options {
            mode,
            app,
            capabilities: Vec::new(),
            #[cfg(feature = "local")]
            engine: None,
            #[cfg(feature = "local")]
            models: ModelChoices::default(),
            socket: None,
            timing: Timing::default(),
            start: Arc::new(client::start_sayso),
            on_approval_waiting: None,
            #[cfg(feature = "local")]
            capture: None,
        }
    }

    pub fn capabilities(mut self, capabilities: impl IntoIterator<Item = Capability>) -> Options {
        self.capabilities = capabilities.into_iter().collect();
        self
    }

    #[cfg(feature = "local")]
    pub fn engine(mut self, engine: Engine) -> Options {
        self.engine = Some(engine);
        self
    }

    #[cfg(feature = "local")]
    pub fn models(mut self, models: ModelChoices) -> Options {
        self.models = models;
        self
    }

    pub fn socket(mut self, socket: impl Into<PathBuf>) -> Options {
        self.socket = Some(socket.into());
        self
    }

    #[cfg(feature = "local")]
    pub fn capture(mut self, capture: CaptureSources) -> Options {
        self.capture = Some(capture);
        self
    }

    pub fn on_approval_waiting(mut self, callback: impl Fn() + Send + Sync + 'static) -> Options {
        self.on_approval_waiting = Some(Arc::new(callback));
        self
    }

    pub fn timing(mut self, timing: Timing) -> Options {
        self.timing = timing;
        self
    }
}

/// What the mode that the app has now can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Features {
    /// [`Speech::enhance`]: the providers and styles of the user. In Sayso mode only.
    pub enhance: bool,
    /// [`Speech::subscribe`]: `dictation_done`. In Sayso mode only.
    pub subscribe: bool,
    /// The user answers a question before a download. In Sayso mode only.
    pub download_question: bool,
    /// The source `system` of a live session can start. In Sayso mode, the user granted
    /// `audio.system`. In local mode, this system has it and did not refuse the app: the app
    /// can still get `permission` at the start, when the system asks the user then.
    pub system_audio: bool,
    /// In Sayso mode, what the user granted. In local mode, what the SDK can do without a grant.
    pub capabilities: Vec<Capability>,
    /// In Sayso mode, what the user refused.
    pub denied: Vec<Capability>,
    /// The optional parts of the Host API, in Sayso mode.
    pub host: Vec<String>,
    /// The features of the engine, in local mode after the engine started. Sayso and the
    /// app can ship different engine versions.
    pub engine: Option<Vec<String>>,
}

/// What the app gets on [`Speech::events`].
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A message of the Host API, with the ids of the SDK: `job_progress`, `segment`,
    /// `speakers`, `job_done`, `job_failed`, `partial`, `source_gap`, `source_ended`,
    /// `part_failed`, `silence_limit`, `session_ended`, `model_state`, `download_progress`,
    /// or `dictation_done`. The app does not see `approval_waiting`: use
    /// [`Options::on_approval_waiting`].
    Message(protocol::Event),
    /// The mode changed. No work of the app is active at this moment, except the work
    /// that the change ended just before this event.
    ModeChanged {
        mode: ActiveMode,
        features: Features,
    },
    /// An `error` that belongs to no request: audio that Sayso could not use, for example.
    Error(ErrorReply),
    /// [`Mode::Sayso`]: the connection ended. Requests fail until [`Speech::reconnect`].
    Disconnected { code: protocol::ErrorCode },
}
