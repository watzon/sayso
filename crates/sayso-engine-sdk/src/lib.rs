//! Start a Sayso speech engine from your own app and talk to it over stdio.
//!
//! The engine is a separate program: `SaysoEngine` on macOS, `sayso-engine` on
//! Linux and Windows. You get it from the release files of Sayso, and you do
//! not need Sayso itself. This crate starts the engine, sends it requests
//! with the engine protocol (`docs/engine-protocol.md`), and gives you the
//! replies and events. The calls block, and the events come on a channel, so
//! you need no async runtime. You can call one [`Engine`] from many threads.
//!
//! ```no_run
//! use sayso_engine_sdk::{Engine, EngineOptions, TranscribeOptions, find_engine};
//!
//! // The folder where you unpacked sayso-engine-<version>-<system>.tar.gz (or the zip).
//! let program = find_engine("/opt/myapp/sayso-engine")?;
//! let engine = Engine::start(EngineOptions::new(program, "/home/me/.myapp/models"))?;
//! println!("engine {} with {:?}", engine.version(), engine.features());
//!
//! // The built-in catalog gives the ids of the models.
//! engine.download_with("parakeet-tdt-v3", None, |progress| println!("{:.0}%", progress.fraction * 100.0))?;
//! engine.load("parakeet-tdt-v3")?;
//! let text = engine.transcribe("parakeet-tdt-v3", "talk.wav", &TranscribeOptions::default())?;
//! println!("{}", text.text);
//! # Ok::<(), sayso_engine_sdk::Error>(())
//! ```
//!
//! # Rules to know
//!
//! - The engine needs a folder for its models. [`EngineOptions`] sets it in
//!   `SAYSO_MODELS_DIR`. Do not let two engines use one folder at the same time.
//! - A `transcribe`, `vad`, or `diarize` call takes a path to a WAV file: 16 kHz,
//!   mono, 16-bit PCM. [`wav`] writes such a file from samples, and
//!   [`Engine::transcribe_i16`] and [`Engine::transcribe_f32`] do it for you.
//! - The engine exits when its stdin closes. Dropping the [`Engine`] closes
//!   stdin, waits a short time, and then kills the process.
//! - The engine writes free text to stderr. By default it goes to the stderr
//!   of your app. See [`Stderr`] to send it to a callback or to discard it.
//! - Check [`Engine::features`] before you use an optional request. A request
//!   that the engine does not know gets an [`Error::Engine`] reply.
//!
//! The engine has the GPL-3.0-only license. This crate has the MIT license and
//! never links the engine, so your app is not a derived work of it.

mod calls;
mod connection;
mod release;
pub mod wav;

pub use calls::{Catalog, Loaded, Model, StreamEnded, Transcript, TranscribeOptions};
pub use release::find_engine;
pub use sayso_protocol as protocol;
pub use sayso_protocol::Fields;

use crossbeam_channel::Receiver;
use sayso_protocol::engine::{Hello, Reply, Request};
use sayso_protocol::{ErrorCode, ErrorReply, PROTOCOL};
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use connection::Connection;

/// Something that the engine sends without a call waiting for it, and the end
/// of the engine.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// An event of the engine protocol. `id` is the id of the request that
    /// caused it, when a request caused it. Every event comes here, also the
    /// ones that a call gave to its own callback.
    Engine { id: Option<String>, event: protocol::engine::Event },
    /// The engine ended without a [`Engine::shutdown`] or a drop. Calls fail
    /// with [`Error::Exited`].
    Exited,
}

#[derive(Debug)]
pub enum Error {
    /// The engine did not start, or a pipe failed.
    Io(io::Error),
    /// The engine replied `error`. [`Error::code`] and [`Error::reason`] have its codes.
    Engine(ErrorReply),
    /// No reply came in time. The engine can still do the work. A late reply is dropped.
    Timeout { request: &'static str, after: Duration },
    /// The engine process ended.
    Exited,
    /// The engine sent something that this SDK cannot use, or it speaks another protocol.
    Protocol(String),
}

impl Error {
    /// The `code` of an `error` reply of the engine, for example
    /// [`ErrorCode::UnknownModel`].
    pub fn code(&self) -> Option<&ErrorCode> {
        match self {
            Error::Engine(error) => error.code.as_ref(),
            _ => None,
        }
    }

    /// The `reason` of an `error` reply of the engine.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Error::Engine(error) => error.reason.as_deref(),
            _ => None,
        }
    }

    /// The English `message` of an `error` reply of the engine.
    pub fn engine_message(&self) -> Option<&str> {
        match self {
            Error::Engine(error) => Some(&error.message),
            _ => None,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "cannot talk to the engine: {e}"),
            Error::Engine(e) => write!(f, "the engine replied: {e}"),
            Error::Timeout { request, after } => write!(f, "the engine gave no reply to {request} in {after:?}"),
            Error::Exited => f.write_str("the engine has ended"),
            Error::Protocol(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            Error::Engine(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        Error::Io(e)
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Where the stderr of the engine goes. It is free text for people and not
/// part of the protocol.
#[derive(Clone, Default)]
pub enum Stderr {
    /// The engine shares the stderr of your app. This is the default.
    #[default]
    Inherit,
    /// Discard it.
    Null,
    /// Call this function for each line, on a thread of the SDK.
    Lines(Arc<dyn Fn(&str) + Send + Sync>),
}

impl Stderr {
    /// Call `log` for each line of stderr.
    pub fn lines(log: impl Fn(&str) + Send + Sync + 'static) -> Stderr {
        Stderr::Lines(Arc::new(log))
    }
}

impl std::fmt::Debug for Stderr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Stderr::Inherit => "Inherit",
            Stderr::Null => "Null",
            Stderr::Lines(_) => "Lines(..)",
        })
    }
}

/// How long a call waits for its reply. Change them with [`Engine::set_timeouts`]
/// or in [`EngineOptions`], or use [`Engine::call`] with a time for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// `hello`, when the engine starts.
    pub startup: Duration,
    /// Quick requests: `list_models`, `delete`, `unload`, the stream requests, and `language_model`.
    pub control: Duration,
    /// `download`.
    pub download: Duration,
    /// `load` and `prepare`.
    pub load: Duration,
    /// `transcribe`, `vad`, and `diarize` on a file.
    pub process: Duration,
}

impl Default for Timeouts {
    fn default() -> Timeouts {
        Timeouts {
            startup: Duration::from_secs(30),
            control: Duration::from_secs(30),
            download: Duration::from_secs(60 * 60),
            load: Duration::from_secs(5 * 60),
            process: Duration::from_secs(10 * 60),
        }
    }
}

/// How to start the engine.
#[derive(Debug, Clone)]
pub struct EngineOptions {
    /// The engine program. [`find_engine`] finds it in an unpacked release file.
    pub program: PathBuf,
    /// The folder for the model files, in `SAYSO_MODELS_DIR`. The engine makes
    /// the folder when it is not there.
    pub models_dir: PathBuf,
    /// More arguments for the program. The engines need none.
    pub args: Vec<OsString>,
    /// More environment variables for the program.
    pub env: Vec<(OsString, OsString)>,
    pub stderr: Stderr,
    pub timeouts: Timeouts,
}

impl EngineOptions {
    pub fn new(program: impl Into<PathBuf>, models_dir: impl Into<PathBuf>) -> EngineOptions {
        EngineOptions { program: program.into(), models_dir: models_dir.into(), args: Vec::new(), env: Vec::new(), stderr: Stderr::default(), timeouts: Timeouts::default() }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> EngineOptions {
        self.args.push(arg.into());
        self
    }

    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> EngineOptions {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn stderr(mut self, stderr: Stderr) -> EngineOptions {
        self.stderr = stderr;
        self
    }
}

/// A running engine. All calls take `&self`, so threads can share it (in an
/// `Arc`) and run requests at the same time. Dropping it stops the engine.
pub struct Engine {
    connection: Connection,
    hello: Hello,
    timeouts: Mutex<Timeouts>,
    events: Receiver<Event>,
    next_session: std::sync::atomic::AtomicU64,
}

impl Engine {
    /// Start the engine, send `hello`, and check that it speaks protocol 1.
    pub fn start(options: EngineOptions) -> Result<Engine> {
        let (connection, events) = Connection::spawn(&options)?;
        let reply = connection.call(&Request::Hello, "hello", options.timeouts.startup, None)?;
        let Reply::Hello(hello) = reply else {
            return Err(Error::Protocol("the engine did not reply to hello with a hello".into()));
        };
        if hello.protocol != PROTOCOL {
            return Err(Error::Protocol(format!("the engine speaks protocol {}, and this SDK speaks {PROTOCOL}", hello.protocol)));
        }
        Ok(Engine { connection, hello, timeouts: Mutex::new(options.timeouts), events, next_session: std::sync::atomic::AtomicU64::new(1) })
    }

    /// The version of the engine. It is the version of the Sayso release that the engine comes with.
    pub fn version(&self) -> &str {
        &self.hello.version
    }

    /// The optional parts of the protocol that this engine has (see "Version and features" of the spec).
    pub fn features(&self) -> &[String] {
        &self.hello.features
    }

    pub fn has_feature(&self, feature: &str) -> bool {
        self.hello.features.iter().any(|f| f == feature)
    }

    /// The events of the engine. Events wait in the channel until you read
    /// them, so read them (or ignore them in a thread) when you use streams.
    pub fn events(&self) -> Receiver<Event> {
        self.events.clone()
    }

    pub fn timeouts(&self) -> Timeouts {
        *self.timeouts.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn set_timeouts(&self, timeouts: Timeouts) {
        *self.timeouts.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = timeouts;
    }

    /// A session number that no other call of this method returned. A stream
    /// or VAD session needs a number, and you can choose your own instead.
    pub fn new_session(&self) -> u64 {
        self.next_session.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Send any request that expects a reply, and wait for the reply.
    /// `Request::StreamAudio` has no reply: use [`Engine::stream_audio_pcm`].
    pub fn call(&self, request: &Request, timeout: Duration) -> Result<Reply> {
        self.connection.call(request, protocol::Kind::kind(request), timeout, None)
    }

    /// The process id of the engine.
    pub fn process_id(&self) -> u32 {
        self.connection.process_id()
    }

    /// True until the engine ends.
    pub fn is_running(&self) -> bool {
        self.connection.is_running()
    }

    /// Send `shutdown`, wait for the engine to exit, and kill it when it does
    /// not exit in 5 seconds. Dropping the engine does the same without the
    /// request.
    pub fn shutdown(self) -> Result<std::process::ExitStatus> {
        self.connection.mark_closing();
        let timeout = self.timeouts().control;
        match self.connection.call(&Request::Shutdown, "shutdown", timeout, None) {
            Ok(_) | Err(Error::Exited) => {}
            Err(e) => return Err(e),
        }
        Ok(self.connection.finish(Duration::from_secs(5))?)
    }
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine").field("hello", &self.hello).finish_non_exhaustive()
    }
}
