//! The messages of the engine protocol (`docs/engine-protocol.md`): what a
//! client sends to a speech engine, and what the engine sends back.
//!
//! These use the same envelope as the Host API (see [`crate::encode`] and
//! [`crate::decode`]): `"v":1`, a string `type`, and an optional string `id`.
//! The types in [`crate::ModelStateEvent`], [`crate::DownloadProgress`],
//! [`crate::Segment`], and [`crate::SpeechPart`] are the same on both layers.
//!
//! ```
//! use sayso_protocol::engine::{Request, Transcribe};
//!
//! let request = Request::Transcribe(Transcribe { model: "parakeet-tdt-v3".into(), path: "/tmp/talk.wav".into(), language: "auto".into(), vocabulary: vec![], segments: Some(true) });
//! let line = sayso_protocol::encode(Some("t1"), &request);
//! assert!(line.contains(r#""type":"transcribe""#));
//! ```

use crate::codec::{Kind, tagged_kind};
use crate::{DownloadProgress, ErrorReply, Fields, ModelStateEvent, Role, Segment, SpeechPart, TimestampLevel};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A request to the engine. Each request with an `id` gets one reply.
/// `StreamAudio` has no `id` and no reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Hello,
    ListModels(ListModels),
    Download(Download),
    Delete(ModelRef),
    Load(ModelRef),
    Unload(Unload),
    Prepare(Prepare),
    Transcribe(Transcribe),
    StreamStart(StreamStart),
    StreamAudio(StreamAudio),
    StreamEnd(StreamEnd),
    Vad(Vad),
    VadStart(VadStart),
    Diarize(Diarize),
    LanguageModel,
    Generate(Generate),
    /// The engine replies `ok`, then exits.
    Shutdown,
}

tagged_kind!(Request {
    Hello => "hello",
    ListModels => "list_models",
    Download => "download",
    Delete => "delete",
    Load => "load",
    Unload => "unload",
    Prepare => "prepare",
    Transcribe => "transcribe",
    StreamStart => "stream_start",
    StreamAudio => "stream_audio",
    StreamEnd => "stream_end",
    Vad => "vad",
    VadStart => "vad_start",
    Diarize => "diarize",
    LanguageModel => "language_model",
    Generate => "generate",
    Shutdown => "shutdown",
});

/// A model of a `list_models` request: its id, and the `engine` object when
/// the client has one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListedModel {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<Fields>,
}

/// Without `models`, the engine checks the models of its built-in catalog.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ListModels {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<ListedModel>>,
}

/// A model for `delete` and `load`. Without `engine`, the engine takes the
/// `engine` object of the id from its catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRef {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<Fields>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Download {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<Fields>,
    /// The size of the model files, for an engine that reports only a fraction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Unload {
    pub model: String,
    /// Keep the live preview part in memory (feature `keep_preview`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_preview: Option<bool>,
}

/// Load the optional parts of a loaded model (feature `prepare`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prepare {
    pub model: String,
    #[serde(default)]
    pub preview: bool,
    #[serde(default)]
    pub vocabulary: Vec<String>,
}

/// The final pass on a WAV file: 16 kHz, mono, 16-bit PCM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcribe {
    pub model: String,
    pub path: PathBuf,
    /// A language code such as `en`, or `auto`.
    #[serde(default = "auto")]
    pub language: String,
    /// Words that the model should prefer.
    #[serde(default)]
    pub vocabulary: Vec<String>,
    /// Ask for `segments` in the reply (feature `segments`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments: Option<bool>,
}

fn auto() -> String {
    "auto".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamStart {
    /// An integer that the client chooses.
    pub session: u64,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// Audio for a stream or a VAD session. It has no `id` and gets no reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamAudio {
    pub session: u64,
    /// Base64 of 16 kHz mono little-endian 16-bit PCM (see [`crate::pcm`]).
    pub pcm: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamEnd {
    pub session: u64,
}

/// Find the speech in a WAV file (feature `vad`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vad {
    pub model: String,
    pub path: PathBuf,
}

/// Open a VAD session on live audio (feature `vad`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VadStart {
    pub session: u64,
    pub model: String,
}

/// Find who speaks when in a WAV file (feature `diarization`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diarize {
    pub model: String,
    pub path: PathBuf,
    /// The number of speakers, when the client knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speakers: Option<u32>,
}

/// Run the language model of the system (feature `language_model`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Generate {
    pub instructions: String,
    pub prompt: String,
    pub timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
}

/// A reply of the engine. Each request with an `id` gets exactly one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Hello(Hello),
    /// The fields depend on the request. [`crate::parse_ok`] reads them as a
    /// typed body such as [`ListModelsReply`].
    Ok(Fields),
    Final(Final),
    Error(ErrorReply),
}

tagged_kind!(Reply {
    Hello => "hello",
    Ok => "ok",
    Final => "final",
    Error => "error",
});

/// The reply to `hello`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    /// The version of the engine.
    #[serde(default)]
    pub version: String,
    pub protocol: u32,
    /// The optional parts of the protocol that this engine has. An engine
    /// that sends none is older than the spec.
    #[serde(default)]
    pub features: Vec<String>,
}

/// The reply to `transcribe`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Final {
    pub text: String,
    #[serde(default)]
    pub elapsed_ms: u64,
    /// With `segments: true` and the feature `segments`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments: Option<Vec<Segment>>,
    /// The language that the model found, such as `de` (feature
    /// `detected_language`). Only when the request had `language` `auto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// A model of the built-in catalog, in the reply to `list_models` without `models`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogModel {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// The `engine` object of the model.
    #[serde(default)]
    pub engine: Fields,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub roles: Vec<Role>,
    #[serde(default = "no_timestamps")]
    pub timestamps: TimestampLevel,
    /// The model finds the language of the speech (feature `detected_language`).
    #[serde(default)]
    pub detects_language: bool,
    #[serde(default)]
    pub recommended: bool,
}

fn no_timestamps() -> TimestampLevel {
    TimestampLevel::None
}

/// An id that left the catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Retired {
    pub id: String,
    #[serde(default)]
    pub message: String,
}

/// `ok` to `list_models`. Both lists are empty when the request had `models`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ListModelsReply {
    #[serde(default)]
    pub models: Vec<CatalogModel>,
    #[serde(default)]
    pub retired: Vec<Retired>,
}

/// `ok` to `vad`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VadReply {
    #[serde(default)]
    pub speech: Vec<SpeechPart>,
}

/// A turn of one speaker. In the engine protocol the speaker is an integer
/// from 0, in the order in which the speakers first speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeakerTurn {
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker: u32,
}

/// `ok` to `diarize`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiarizeReply {
    #[serde(default)]
    pub speakers: u32,
    #[serde(default)]
    pub turns: Vec<SpeakerTurn>,
}

/// `ok` to `stream_end`. A VAD session has no `text`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamEndReply {
    pub session: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// `ok` to `language_model`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LanguageModelReply {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// A reason code (see "Reason codes" in the spec).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `ok` to `generate`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerateReply {
    pub text: String,
    #[serde(default)]
    pub model: String,
}

/// An event of the engine. An event that a request caused has the `id` of
/// that request and comes before its reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    ModelState(ModelStateEvent),
    DownloadProgress(DownloadProgress),
    PartState(PartState),
    Partial(Partial),
    SpeechStart(SpeechStart),
    SpeechEnd(SpeechEnd),
    Log(Log),
}

tagged_kind!(Event {
    ModelState => "model_state",
    DownloadProgress => "download_progress",
    PartState => "part_state",
    Partial => "partial",
    SpeechStart => "speech_start",
    SpeechEnd => "speech_end",
    Log => "log",
});

/// The state of an optional part of a model, after `prepare`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartState {
    pub model: String,
    /// `preview` or `dictionary`.
    pub part: String,
    /// `preparing`, `ready`, or `failed`.
    pub state: String,
}

/// The text of a stream so far. It has all the text of the stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Partial {
    pub session: u64,
    /// Text that does not change any more.
    pub committed: String,
    /// The newest text, which can still change.
    pub tentative: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeechStart {
    pub session: u64,
    pub start_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeechEnd {
    pub session: u64,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Log {
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub message: String,
}

/// Any message from the engine: a reply or an event.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum FromEngine {
    Reply(Reply),
    Event(Event),
}

impl Kind for FromEngine {
    fn knows(kind: &str) -> bool {
        Reply::knows(kind) || Event::knows(kind)
    }
    fn kind(&self) -> &'static str {
        match self {
            FromEngine::Reply(reply) => reply.kind(),
            FromEngine::Event(event) => event.kind(),
        }
    }
    fn from_fields(fields: Fields) -> serde_json::Result<Self> {
        let is_reply = fields.get("type").and_then(|kind| kind.as_str()).is_some_and(Reply::knows);
        if is_reply { Reply::from_fields(fields).map(FromEngine::Reply) } else { Event::from_fields(fields).map(FromEngine::Event) }
    }
}
