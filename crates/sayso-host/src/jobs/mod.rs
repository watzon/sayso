//! Jobs: the transcription of a file in the background for an extension
//! (`transcribe_file` and `cancel_job` in `docs/extensions.md`, "Requests"
//! and "Scheduling").
//!
//! The jobs run in `sayso_speech::jobs` (MIT), so the local mode of the SDK
//! runs the same code. The host gives them what is Sayso's:
//!
//! - [`resolve`] checks the models of a request with the choices of the user
//!   and the catalog of Sayso, and gives a [`JobSpec`].
//! - [`JobRunner`] runs a job on the engine of the app (`SttBackend`).
//! - [`DictationGate`] holds a job between two engine calls while a dictation
//!   runs or a live session transcribes a part.
//! - [`ModelUse`] tells the memory policy of the app which models the jobs use.
//!
//! The events of a job go to an [`EventSink`]; the connection gives one.

mod model_use;
pub(crate) mod resolve;

#[cfg(test)]
mod tests;

pub use model_use::{ModelHold, ModelUse};
pub use resolve::{EngineView, Refusal, SaysoModels, UserChoices, resolve};
pub use sayso_speech::EventSink;
pub use sayso_speech::jobs::{CANCELLED, DictationGate, GRANT_GONE, JobQueue, JobRun, JobSpec, LivePart, RunJob, STALE_AFTER, TRANSCRIPTION_SHARE, VadRun, audio, plan, speakers};

use crate::backend::BackendEngine;
use sayso_platform::SttBackend;
use std::sync::Arc;

/// Runs jobs on the engine of the app: [`sayso_speech::jobs::JobRunner`]
/// over the `SttBackend` of Sayso.
pub struct JobRunner(sayso_speech::jobs::JobRunner);

impl JobRunner {
    /// `models` tells the memory policy of the app which models the jobs use.
    pub fn new(backend: Arc<dyn SttBackend>, models: Arc<ModelUse>) -> JobRunner {
        JobRunner(sayso_speech::jobs::JobRunner::new(Arc::new(BackendEngine::new(backend, None)), models))
    }

    /// A runner whose VAD model gets windows of `window_ms`, for the tests.
    #[cfg(test)]
    pub(crate) fn with_vad_window(self, window_ms: u64) -> JobRunner {
        JobRunner(self.0.with_vad_window(window_ms))
    }
}

impl RunJob for JobRunner {
    fn run(&self, job: &JobSpec, run: &JobRun<'_>) {
        self.0.run(job, run);
    }
}
