//! Events from the host. An event that a request caused has the `id` of that
//! request. Other events can come at any time.

use crate::codec::tagged_kind;
use crate::{ErrorCode, ModelState, Segment, Source, Turn};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    ModelState(ModelStateEvent),
    DownloadProgress(DownloadProgress),
    JobProgress {
        job: u64,
        /// From 0 to 1. A job that waits in the queue sends 0.
        fraction: f64,
    },
    Segment(SegmentEvent),
    Speakers(SpeakersEvent),
    JobDone {
        job: u64,
        text: String,
    },
    JobFailed {
        job: u64,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<ErrorCode>,
    },
    Partial(PartialEvent),
    /// The user stopped the live session in Sayso, not the app. The last
    /// `segment` and `speakers` events come before it, and it is the end of
    /// the session: there is no reply to `session_stop`. The SDK also gives
    /// this event, with a `code`, when the connection ends during a session.
    SessionEnded {
        session: u64,
        /// The text of all segments in the order of time.
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<ErrorCode>,
    },
    DictationDone(DictationDone),
    /// Sayso quits or restarts, and closes the connection.
    HostClosing,
}

tagged_kind!(Event {
    ModelState => "model_state",
    DownloadProgress => "download_progress",
    JobProgress => "job_progress",
    Segment => "segment",
    Speakers => "speakers",
    JobDone => "job_done",
    JobFailed => "job_failed",
    Partial => "partial",
    SessionEnded => "session_ended",
    DictationDone => "dictation_done",
    HostClosing => "host_closing",
});

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelStateEvent {
    pub model: String,
    pub state: ModelState,
    /// The time of the load, with the state `ready`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_ms: Option<u64>,
    /// Only the live preview part is in memory, with the state `downloaded`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<bool>,
    /// Why, in English, with the state `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub model: String,
    pub fraction: f64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// A segment of a job or of one source of a live session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SegmentEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    #[serde(flatten)]
    pub segment: Segment,
}

/// The speakers of a job, or of one source of a live session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeakersEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    pub turns: Vec<Turn>,
}

/// The text of one source of a live session so far.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartialEvent {
    pub session: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    /// Text that does not change any more.
    pub committed: String,
    /// The newest text, which can still change.
    pub tentative: String,
}

/// A dictation after its insert.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DictationDone {
    /// The inserted text.
    pub text: String,
    /// The text of the speech model, before replacements and the style.
    pub transcript: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// When the insert happened, in RFC 3339.
    pub time: String,
}
