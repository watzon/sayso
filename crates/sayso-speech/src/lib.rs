//! Jobs and live sessions over a Sayso speech engine (`docs/sdk.md`, "Crates").
//!
//! Sayso (`sayso-host`) and the local mode of the SDK run the same code. The
//! parts that differ come through traits:
//!
//! - [`SpeechEngine`]: the calls to the engine.
//! - [`ModelResolver`]: the models of a request, and the checks.
//! - [`ModelRetention`]: keeps the models of work in memory while it runs.
//! - [`Gate`]: dictation first, then live sessions, then jobs.
//! - [`EventSink`]: where the events go.
//! - [`live::Recordings`]: where the recordings of live sessions go.
//!
//! A job or a live session stops between two engine calls when it is
//! cancelled ([`jobs::JobQueue::cancel`], [`live::LiveSessions::connection_closed`]).
//!
//! The events are the messages of `sayso-protocol`, so both modes send the
//! same messages. The other types are in this crate ([`ModelId`],
//! [`Timestamps`], [`Turn`], [`EngineEvent`], [`EngineError`]), and Sayso
//! converts its own types to them.

pub mod basic_vad;
mod engine;
pub mod jobs;
pub mod live;
pub mod resample;

pub use engine::{EngineError, EngineEvent, SpeechEngine};
pub use jobs::{DictationGate, Gate, JobSpec, NoGate, VadRun};
pub use live::SessionSpec;
/// A part of the audio with speech, from VAD.
pub use sayso_protocol::SpeechPart;

use sayso_protocol::{ErrorCode, Event, SessionStart, TranscribeFile};

/// The id of a model in the catalog of the engine, such as `parakeet-tdt-v3`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelId(pub String);

impl ModelId {
    pub fn new(s: impl Into<String>) -> Self {
        ModelId(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ModelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The times that a speech model gives with its text (the timestamp level).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timestamps {
    /// Segments with the time of each word.
    Word,
    /// The model's own segments, without word times.
    Segment,
    /// No times: one segment for each piece of audio that the engine
    /// transcribes alone.
    None,
}

/// A turn of one speaker, from the diarization of the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turn {
    pub start_ms: u64,
    pub end_ms: u64,
    /// From 0, in the order in which the speakers first speak.
    pub speaker: u32,
}

/// Where the events of jobs and live sessions go: the connection of the app.
/// `event` must not block, because the queue can call it while it holds its lock.
pub trait EventSink: Send + Sync {
    fn event(&self, event: Event);
}

/// Keeps the models of work in memory while it runs. Jobs and live sessions
/// hold their models before they load them, so nobody unloads a model
/// between the two.
pub trait ModelRetention: Send + Sync {
    /// Hold `models` until the result drops.
    fn hold(&self, models: Vec<ModelId>) -> Box<dyn Send>;
}

/// A request that is refused: the `error` reply of `transcribe_file` or
/// `session_start`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: ErrorCode,
    /// English.
    pub message: String,
}

impl Refusal {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Refusal {
        Refusal { code, message: message.into() }
    }
}

/// Chooses the models of a request, and checks them (`docs/extensions.md`,
/// "Requests", and the error codes in "Code"): a model of this system
/// (`unknown_model`) of the right task (`wrong_role`), on disk
/// (`model_not_downloaded`), that the engine can run (`unsupported`). A field
/// that is missing gets the default model of its role.
pub trait ModelResolver {
    /// The job of `transcribe_file`.
    fn job(&self, request: &TranscribeFile) -> Result<JobSpec, Refusal>;
    /// The live session of `session_start`. The sources and `record_to` are
    /// checked when the session starts.
    fn session(&self, request: &SessionStart) -> Result<SessionSpec, Refusal>;
}
