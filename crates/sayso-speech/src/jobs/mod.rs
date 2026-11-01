//! Jobs: the transcription of a file in the background (`transcribe_file`
//! and `cancel_job` in `docs/extensions.md`, "Requests" and "Scheduling").
//!
//! The parts, each of them without the connection:
//!
//! - A [`ModelResolver`](crate::ModelResolver) checks the models of a request
//!   and gives a [`JobSpec`].
//! - [`JobQueue`] runs one job at a time, first in, first out, and cancels.
//! - [`JobRunner`] runs a job on a [`SpeechEngine`](crate::SpeechEngine):
//!   [`audio`] reads the file, [`plan`] finds the speech and the chunks,
//!   and [`speakers`] gives the speakers.
//! - A [`Gate`] ([`DictationGate`]) holds a job between two engine calls
//!   while a dictation runs or a live session transcribes a part.
//! - A [`ModelRetention`](crate::ModelRetention) keeps the models of a job in memory.
//!
//! The events of a job go to an [`EventSink`](crate::EventSink); the connection gives one.

pub mod audio;
mod gate;
pub mod plan;
mod queue;
mod runner;
pub mod speakers;

pub use gate::{DictationGate, Gate, LivePart, NoGate, STALE_AFTER};
pub use plan::VadRun;
pub use queue::{CANCELLED, GRANT_GONE, JobQueue, JobRun, RunJob};
pub use runner::{JobRunner, TRANSCRIPTION_SHARE};

use crate::{ModelId, Timestamps};
use std::path::PathBuf;

/// A job after the model checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobSpec {
    /// The file. Absolute.
    pub path: PathBuf,
    /// The speech model.
    pub model: ModelId,
    /// The timestamp level of `model` on this engine.
    pub timestamps: Timestamps,
    pub vad: VadRun,
    /// None without diarization.
    pub diarization: Option<ModelId>,
    /// A code such as `en`, or `auto`.
    pub language: String,
    /// The number of speakers, when the app knows it.
    pub speakers: Option<u32>,
}

impl JobSpec {
    /// The engine models of the job, in the order of their use.
    pub fn models(&self) -> Vec<ModelId> {
        let mut models = Vec::new();
        if let VadRun::Model(vad) = &self.vad {
            models.push(vad.clone());
        }
        models.push(self.model.clone());
        models.extend(self.diarization.clone());
        models
    }
}
