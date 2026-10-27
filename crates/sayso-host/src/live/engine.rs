//! What a live session needs from the speech engine, as a small trait of its
//! own. The wiring maps it onto `sayso_platform::SttBackend` and the `Bus` of
//! the host; the tests use a fake.

use crossbeam_channel::Receiver;
use sayso_core::models::ModelId;
use sayso_protocol::{ErrorCode, Segment};

/// A failed call to the engine. `code` is the `code` of the engine's `error`
/// reply when it has one, for example `too_many_streams`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError {
    pub code: Option<ErrorCode>,
    pub message: String,
}

impl EngineError {
    pub fn new(message: impl Into<String>) -> EngineError {
        EngineError { code: None, message: message.into() }
    }

    pub fn with_code(code: ErrorCode, message: impl Into<String>) -> EngineError {
        EngineError { code: Some(code), message: message.into() }
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// An event of the engine that a live session uses. The times are in
/// milliseconds from the first audio of the engine session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveEvent {
    /// The text of a preview stream so far.
    Partial { session: u64, committed: String, tentative: String },
    /// Speech starts in a VAD session.
    SpeechStart { session: u64, start_ms: u64 },
    /// The speech that started at `start_ms` ends in a VAD session.
    SpeechEnd { session: u64, start_ms: u64, end_ms: u64 },
}

/// A turn of the engine's `diarize` reply: the speaker is a number from 0, in
/// the order in which the speakers first speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawTurn {
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker: u32,
}

/// The engine calls of a live session (`docs/engine-protocol.md`, "Streams",
/// "VAD", "Segments", "Diarization"). The host calls them from its own
/// threads.
pub trait LiveEngine: Send + Sync {
    /// Load a model into memory. Blocks. A model that is in memory returns at once.
    fn load(&self, model: &ModelId) -> Result<(), EngineError>;
    /// Open a preview stream of a streaming model. Its text arrives as
    /// [`LiveEvent::Partial`]. A full engine gives the code `too_many_streams`.
    fn start_stream(&self, session: u64, model: &ModelId, language: &str) -> Result<(), EngineError>;
    /// Open a VAD session with a VAD model. Its speech arrives as
    /// [`LiveEvent::SpeechStart`] and [`LiveEvent::SpeechEnd`]. A full engine
    /// gives the code `too_many_streams`.
    fn start_vad(&self, session: u64, model: &ModelId) -> Result<(), EngineError>;
    /// Send audio (16 kHz mono, -1 to 1) to a preview stream or a VAD session.
    /// Must not wait for the engine. The audio of a VAD session must not be
    /// dropped; the audio of a preview stream can be, when the engine is behind.
    fn push_audio(&self, session: u64, samples: &[f32]) -> Result<(), EngineError>;
    /// Close a preview stream. Need not wait for the reply.
    fn end_stream(&self, session: u64);
    /// Close a VAD session, and wait for the reply of the engine. The engine
    /// sends the last `speech_end` before that reply, so it is on the
    /// receivers of [`LiveEngine::events`] when this returns, or soon after.
    fn end_vad(&self, session: u64) -> Result<(), EngineError>;
    /// The final pass on one part of the audio, with segments. The times of
    /// the segments are from the first sample of `samples`.
    fn transcribe_segments(&self, samples: &[f32], model: &ModelId, language: &str) -> Result<Vec<Segment>, EngineError>;
    /// Who speaks when in a whole recording.
    fn diarize(&self, model: &ModelId, samples: &[f32]) -> Result<Vec<RawTurn>, EngineError>;
    /// A new receiver of the events from now on.
    fn events(&self) -> Receiver<LiveEvent>;
}
