//! Requests from a connected app to the host (`docs/extensions.md`, "Requests").

use crate::codec::tagged_kind;
use crate::{Capability, Role, Source};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A request. Each request with an `id` gets one reply: `ok` or `error`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Hello(Hello),
    ListModels(ListModels),
    DownloadModel(DownloadModel),
    TranscribeFile(TranscribeFile),
    CancelJob(CancelJob),
    SessionStart(SessionStart),
    SessionAudio(SessionAudio),
    SessionStop(SessionStop),
    Enhance(Enhance),
    Subscribe(Subscribe),
    /// The host replies `ok`, then closes the connection.
    Bye,
}

tagged_kind!(Request {
    Hello => "hello",
    ListModels => "list_models",
    DownloadModel => "download_model",
    TranscribeFile => "transcribe_file",
    CancelJob => "cancel_job",
    SessionStart => "session_start",
    SessionAudio => "session_audio",
    SessionStop => "session_stop",
    Enhance => "enhance",
    Subscribe => "subscribe",
    Bye => "bye",
});

/// The capabilities that a live session with these sources needs, for its
/// start and while it runs.
pub fn session_capabilities(sources: &[Source]) -> Vec<Capability> {
    let mut needed = vec![Capability::SessionLive];
    if sources.contains(&Source::Mic) {
        needed.push(Capability::AudioMic);
    }
    if sources.contains(&Source::System) {
        needed.push(Capability::AudioSystem);
    }
    needed
}

impl Request {
    /// The capabilities that the user must grant before the host runs this
    /// request (the "Capability" column of "Requests"). An app can always end
    /// its own work, so `cancel_job` and `session_stop` need none.
    pub fn capabilities(&self) -> Vec<Capability> {
        match self {
            Request::Hello(_) | Request::Bye | Request::CancelJob(_) | Request::SessionStop(_) => vec![],
            Request::ListModels(_) => vec![Capability::ModelsRead],
            Request::DownloadModel(_) => vec![Capability::ModelsDownload],
            Request::TranscribeFile(_) => vec![Capability::TranscribeFile],
            Request::SessionStart(start) => session_capabilities(&start.sources),
            Request::SessionAudio(_) => vec![Capability::SessionLive],
            Request::Enhance(_) => vec![Capability::AiEnhance],
            Request::Subscribe(_) => vec![Capability::DictationObserve],
        }
    }
}

/// The first request of a client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    /// The name and version of the SDK, for example `sayso-extension 0.13.0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk: Option<String>,
    pub protocol: u32,
    /// The optional parts of the protocol that the client knows.
    #[serde(default)]
    pub features: Vec<String>,
    /// What the app says about itself. The host does not check it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<AppName>,
    /// The capabilities that the app asks for. The host asks the user for the
    /// ones that have no answer yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<Capability>,
}

/// `app` of `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppName {
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ListModels {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
}

/// The host asks the user before the download.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadModel {
    pub model: String,
    /// Why the app needs the model, for the question to the user.
    pub reason: String,
}

/// Start a job. `ok {job}` comes at once.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TranscribeFile {
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vad_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diarize: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diarization_model: Option<String>,
    /// The number of speakers, when the app knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speakers: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CancelJob {
    pub job: u64,
}

/// Start a live session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionStart {
    pub sources: Vec<Source>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vad_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diarize: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diarization_model: Option<String>,
    /// A folder for one WAV file of each source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_to: Option<PathBuf>,
}

/// Audio for the source `client`. It has no `id` and gets no reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionAudio {
    pub session: u64,
    /// Base64 of 16 kHz mono little-endian 16-bit PCM (see [`crate::pcm`]).
    pub pcm: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionStop {
    pub session: u64,
}

/// `text` with an optional `style`, or a `prompt`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Enhance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subscribe {
    /// Event types, for example `dictation_done`.
    pub events: Vec<String>,
}
