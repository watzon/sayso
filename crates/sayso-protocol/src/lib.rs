//! The messages of the Sayso Host API (`docs/extensions.md`, "Host API") and
//! the data types that it shares with the engine protocol
//! (`docs/engine-protocol.md`).
//!
//! Each message is one JSON object on one line, with `"v":1`, a string
//! `type`, and an optional string `id`. [`encode`] writes a message and
//! [`decode`] reads one. A reader ignores fields that it does not know, and a
//! message type that it does not know comes back as [`Decoded::Unknown`], so
//! an old reader does not fail on a newer peer.
//!
//! The messages of the engine protocol are in [`engine`].
//!
//! This crate does no I/O. The socket path rules in [`socket`] ask the system
//! only for the user (uid, temporary folder, SID).

mod codec;
pub mod engine;
mod event;
mod names;
pub mod pcm;
mod reply;
mod request;
pub mod socket;
mod types;

pub use codec::{Decoded, DecodeError, Kind, Message, PROTOCOL, decode, encode};
pub use event::{DictationDone, DownloadProgress, Event, ModelStateEvent, PartialEvent, SegmentEvent, SpeakersEvent};
pub use names::{Capability, ErrorCode, ModelState, Role, Source, TimestampLevel};
pub use reply::{EnhanceReply, parse_ok, ErrorReply, HelloReply, HostMessage, JobReply, ListModelsReply, Reply, SessionReply, SubscribeReply, TextReply};
pub use request::{CancelJob, DownloadModel, Enhance, Hello, ListModels, Request, SessionAudio, SessionStart, SessionStop, Subscribe, TranscribeFile};
pub use types::{ModelInfo, Segment, SpeechPart, Turn, Word};

/// A JSON object: the fields of a message.
pub type Fields = serde_json::Map<String, serde_json::Value>;
