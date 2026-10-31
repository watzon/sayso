//! Live sessions (`docs/extensions.md`, "Requests" `session_start`,
//! `session_audio`, `session_stop`, and "Scheduling", "Capture", "Lifecycle",
//! "Privacy"): audio that the host transcribes for an extension while it
//! arrives.
//!
//! The sessions run in `sayso_speech::live` (MIT), so the local mode of the
//! SDK runs the same code. The host gives them the engine of the app
//! ([`BackendEngine`]), the shared microphone of the app ([`Microphone`]),
//! the system audio of `sayso-capture` ([`SystemAudio`]), dictation first (the [`DictationGate`](crate::jobs::DictationGate) of
//! jobs), the models of the user ([`resolve`]), and the folders of the
//! recordings ([`RecordingFolder`]). `live_requests.rs` gives them.

mod resolve;

pub use crate::backend::BackendEngine;
pub use resolve::resolve;
pub use sayso_speech::live::{
    AudioFeed, CapturedAudio, ENGINE_SESSION_BASE, FeedItem, LiveDeps, LiveError, LiveInfo, LiveObserver, LiveSessions, Microphone, OpenError, Owner, RecordingFolder, Recordings, SessionSpec, SourceEnd, Started,
    SystemAudio, recording_name,
};
pub use sayso_speech::{EngineError, EventSink, SpeechEngine};
