//! What a live session needs from the speech engine, as a small trait of its
//! own. [`super::backend::BackendEngine`] maps it onto
//! `sayso_platform::SttBackend` and the `Bus` of the host; the tests use a fake.

use crossbeam_channel::Receiver;
use sayso_core::models::ModelId;
use sayso_core::stt::{EngineEvent, Turn};
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

impl From<sayso_platform::PlatformError> for EngineError {
    fn from(error: sayso_platform::PlatformError) -> EngineError {
        EngineError { code: error.code().map(ErrorCode::parse), message: error.to_string() }
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// The engine calls of a live session (`docs/engine-protocol.md`, "Streams",
/// "VAD", "Segments", "Diarization"). The host calls them from its own
/// threads. A session uses the events `Partial`, `SpeechStart`, and `SpeechEnd`.
pub trait LiveEngine: Send + Sync {
    /// Load a model into memory. Blocks. A model that is in memory returns at once.
    fn load(&self, model: &ModelId) -> Result<(), EngineError>;
    /// Open a preview stream of a streaming model. Its text arrives as
    /// `EngineEvent::Partial`. A full engine gives the code `too_many_streams`.
    fn start_stream(&self, session: u64, model: &ModelId, language: &str) -> Result<(), EngineError>;
    /// Open a VAD session with a VAD model. Its speech arrives as
    /// `EngineEvent::SpeechStart` and `SpeechEnd`. A full engine gives the
    /// code `too_many_streams`.
    fn start_vad(&self, session: u64, model: &ModelId) -> Result<(), EngineError>;
    /// Send audio (16 kHz mono, -1 to 1) to a preview stream or a VAD session.
    /// Must not wait for the engine.
    fn push_audio(&self, session: u64, samples: &[f32]) -> Result<(), EngineError>;
    /// Close a preview stream. Need not wait for the reply.
    fn end_stream(&self, session: u64);
    /// Close a VAD session, and wait for the reply of the engine. The last
    /// `SpeechEnd` of the session must be on the receivers of
    /// [`LiveEngine::events`] when this returns.
    fn end_vad(&self, session: u64) -> Result<(), EngineError>;
    /// The final pass on one part of the audio, with segments. The times of
    /// the segments are from the first sample of `samples`.
    fn transcribe_segments(&self, samples: &[f32], model: &ModelId, language: &str) -> Result<Vec<Segment>, EngineError>;
    /// Who speaks when in a whole recording, with speakers from 0.
    fn diarize(&self, model: &ModelId, samples: &[f32]) -> Result<Vec<Turn>, EngineError>;
    /// A new receiver of the events from now on.
    fn events(&self) -> Receiver<EngineEvent>;
}
