//! Jobs: the transcription of a file in the background for an extension
//! (`transcribe_file` and `cancel_job` in `docs/extensions.md`, "Requests"
//! and "Scheduling").
//!
//! The parts, each of them without the connection:
//!
//! - [`resolve`] checks the models of a request and gives a [`JobSpec`].
//! - [`JobQueue`] runs one job at a time, first in, first out, and cancels.
//! - [`JobRunner`] runs a job on a [`SttBackend`](sayso_platform::SttBackend):
//!   [`audio`] reads the file, [`plan`] finds the speech and the chunks,
//!   and [`speakers`] gives the speakers.
//! - [`DictationGate`] holds a job between two engine calls while a dictation
//!   runs or a live session transcribes a part.
//! - [`ModelUse`] tells the memory policy of the app which models the jobs use.
//!
//! The events of a job go to an [`EventSink`]; the connection gives one.

pub mod audio;
mod gate;
mod model_use;
pub mod plan;
mod queue;
pub(crate) mod resolve;
mod runner;
pub mod speakers;

#[cfg(test)]
mod tests;

pub use gate::{DictationGate, LivePart, STALE_AFTER};
pub use model_use::{ModelHold, ModelUse};
pub use plan::VadRun;
pub use queue::{CANCELLED, JobQueue, JobRun, RunJob};
pub use resolve::{EngineView, Refusal, UserChoices, resolve};
pub use runner::{JobRunner, TRANSCRIPTION_SHARE};

use sayso_core::models::{ModelId, Timestamps};
use sayso_protocol::Event;
use std::path::PathBuf;

/// Where the events of a job go: the connection of the extension. `event`
/// must not block, because the queue can call it while it holds its lock.
pub trait EventSink: Send + Sync {
    fn event(&self, event: Event);
}

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
    /// The number of speakers, when the extension knows it.
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
