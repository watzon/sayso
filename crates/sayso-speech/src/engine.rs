//! What jobs and live sessions need from the speech engine, as a small trait
//! of its own. Sayso maps it onto its engine client (`sayso-host`,
//! `BackendEngine`); the tests use a fake.

use crate::{ModelId, SpeechPart, Turn};
use crossbeam_channel::Receiver;
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

/// The events of the engine that a live session uses (`docs/engine-protocol.md`,
/// "Streams" and "VAD").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineEvent {
    /// The text of a preview stream so far.
    Partial { session: u64, committed: String, tentative: String },
    /// A VAD session heard speech start.
    SpeechStart { session: u64, start_ms: u64 },
    /// A VAD session heard speech end.
    SpeechEnd { session: u64, start_ms: u64, end_ms: u64 },
}

/// The engine calls of jobs and live sessions (`docs/engine-protocol.md`,
/// "Streams", "VAD", "Segments", "Diarization"). The calls come from the
/// threads of this crate. A live session uses the events `Partial`,
/// `SpeechStart`, and `SpeechEnd`.
pub trait SpeechEngine: Send + Sync {
    /// Load a model into memory. Blocks. A model that is in memory returns at once.
    fn load(&self, model: &ModelId) -> Result<(), EngineError>;
    /// The final pass on one part of the audio, as text only: for a model without times.
    fn transcribe(&self, samples: &[f32], model: &ModelId, language: &str) -> Result<String, EngineError>;
    /// The final pass on one part of the audio, with segments. The times of
    /// the segments are from the first sample of `samples`, and the texts are
    /// trimmed. An engine without segments gives one segment for all of `samples`.
    fn transcribe_segments(&self, samples: &[f32], model: &ModelId, language: &str) -> Result<Vec<Segment>, EngineError>;
    /// The speech in `samples` with a VAD model, with times from the first sample.
    fn detect_speech(&self, model: &ModelId, samples: &[f32]) -> Result<Vec<SpeechPart>, EngineError>;
    /// Who speaks when in `samples`, with speakers from 0. `speakers` is the
    /// number of speakers, when the app knows it.
    fn diarize(&self, model: &ModelId, samples: &[f32], speakers: Option<u32>) -> Result<Vec<Turn>, EngineError>;
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
    /// [`SpeechEngine::events`] when this returns.
    fn end_vad(&self, session: u64) -> Result<(), EngineError>;
    /// A new receiver of the events from now on.
    fn events(&self) -> Receiver<EngineEvent>;
}
