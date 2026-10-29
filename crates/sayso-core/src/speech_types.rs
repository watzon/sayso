//! The types of `sayso-speech` from the types of Sayso. The jobs and the live
//! sessions of the host run in `sayso-speech`, which has its own types
//! (`docs/sdk.md`, "Crates").

use crate::models::{ModelId, Timestamps, Vad};
use crate::stt::{EngineEvent, SpeechPart, Turn};

impl From<ModelId> for sayso_speech::ModelId {
    fn from(id: ModelId) -> Self {
        sayso_speech::ModelId(id.0)
    }
}

impl From<&sayso_speech::ModelId> for ModelId {
    fn from(id: &sayso_speech::ModelId) -> Self {
        ModelId(id.0.clone())
    }
}

impl From<sayso_speech::ModelId> for ModelId {
    fn from(id: sayso_speech::ModelId) -> Self {
        ModelId(id.0)
    }
}

impl From<Timestamps> for sayso_speech::Timestamps {
    fn from(timestamps: Timestamps) -> Self {
        match timestamps {
            Timestamps::Word => sayso_speech::Timestamps::Word,
            Timestamps::Segment => sayso_speech::Timestamps::Segment,
            Timestamps::None => sayso_speech::Timestamps::None,
        }
    }
}

impl From<Vad> for sayso_speech::VadRun {
    fn from(vad: Vad) -> Self {
        match vad {
            Vad::Basic => sayso_speech::VadRun::Basic,
            Vad::Model(id) => sayso_speech::VadRun::Model(id.into()),
        }
    }
}

impl From<SpeechPart> for sayso_speech::SpeechPart {
    fn from(part: SpeechPart) -> Self {
        sayso_speech::SpeechPart { start_ms: part.start_ms, end_ms: part.end_ms }
    }
}

impl From<Turn> for sayso_speech::Turn {
    fn from(turn: Turn) -> Self {
        sayso_speech::Turn { start_ms: turn.start_ms, end_ms: turn.end_ms, speaker: turn.speaker }
    }
}

impl EngineEvent {
    /// The event for a live session, when it is one: a partial, or the start
    /// or the end of speech.
    pub fn for_speech(&self) -> Option<sayso_speech::EngineEvent> {
        Some(match self {
            EngineEvent::Partial { session, committed, tentative } => {
                sayso_speech::EngineEvent::Partial { session: *session, committed: committed.clone(), tentative: tentative.clone() }
            }
            EngineEvent::SpeechStart { session, start_ms } => sayso_speech::EngineEvent::SpeechStart { session: *session, start_ms: *start_ms },
            EngineEvent::SpeechEnd { session, start_ms, end_ms } => {
                sayso_speech::EngineEvent::SpeechEnd { session: *session, start_ms: *start_ms, end_ms: *end_ms }
            }
            _ => return None,
        })
    }
}
