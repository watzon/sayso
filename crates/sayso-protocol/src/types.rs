//! Data types that the Host API shares with the engine protocol.

use crate::{ModelState, Role, TimestampLevel};
use serde::{Deserialize, Deserializer, Serialize};

/// A word with its times, in milliseconds from the start of the audio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// A part of a transcript with its times. `words` is there when the model
/// has the timestamp level `word`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<Word>>,
}

/// A part of the audio with speech, from VAD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechPart {
    pub start_ms: u64,
    pub end_ms: u64,
}

/// A turn of one speaker, from diarization.
///
/// In the Host API a speaker is a label such as `"1"`, unique in one source
/// of one job or live session. The engine protocol sends an integer from 0;
/// this type reads both and writes a string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub start_ms: u64,
    pub end_ms: u64,
    #[serde(deserialize_with = "label")]
    pub speaker: String,
}

fn label<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Label {
        Text(String),
        Number(u64),
    }
    Ok(match Label::deserialize(deserializer)? {
        Label::Text(text) => text,
        Label::Number(number) => number.to_string(),
    })
}

/// A model in the reply to `list_models`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub roles: Vec<Role>,
    pub state: ModelState,
    /// Language codes such as `en`. Empty when the model has no list.
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default = "no_timestamps")]
    pub timestamps: TimestampLevel,
    #[serde(default)]
    pub size_bytes: u64,
    /// The roles for which the user chose this model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choice: Vec<Role>,
}

fn no_timestamps() -> TimestampLevel {
    TimestampLevel::None
}
