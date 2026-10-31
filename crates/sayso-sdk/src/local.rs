//! Local mode: the engine of the app, with the jobs and the live sessions of
//! `sayso-speech` (`docs/sdk.md`, "Local mode").

use crate::local_engine::LocalEngine;
use crate::local_models::LocalModels;
use crate::protocol::{self, ErrorCode, ErrorReply, ModelState};
use crate::{CaptureSources, Engine, ModelChoices, Timing, paths};
use sayso_capture::live::{LiveMicrophone, LiveSystemAudio};
use sayso_capture::system;
use sayso_engine_sdk::{EngineOptions, Stderr};
use sayso_speech::jobs::{DictationGate, JobQueue, JobRunner};
use sayso_speech::live::{LiveDeps, LiveError, LiveSessions, Owner, RecordingFolder, Started};
use sayso_speech::{EventSink, JobSpec, SessionSpec};
use std::sync::Arc;
use tempfile::TempDir;

/// Local mode has one app, so one number for its connection.
const CONNECTION: u64 = 1;

/// The engine of the app while it runs, and the work on it. Dropping it stops the jobs
/// and the sessions, and then the engine stops.
pub(crate) struct LocalRun {
    engine: Arc<LocalEngine>,
    jobs: JobQueue,
    live: LiveSessions,
    /// Goes with the run, after the sessions.
    _recordings: TempDir,
}

/// A folder for the temporary recordings of one run. They hold what people said, and the
/// temporary folder of Linux is open to each user. So the name is random, the mode is 0700,
/// and the creation fails on a name that exists (it then tries another name).
fn recordings_folder() -> std::io::Result<TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("sayso-sdk-live-");
    // The default of the crate is 0755, less the umask.
    #[cfg(unix)]
    builder.permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700));
    builder.tempdir()
}

impl LocalRun {
    /// Start the engine. The engine SDK checks the protocol version in `hello`. It gives the
    /// store only to an engine that lists `store_v1`. Each request checks the features that it needs.
    pub fn start(engine: &Engine, app: &str, timing: &Timing, capture: Option<&CaptureSources>, downloads: Arc<dyn Fn(protocol::Event) + Send + Sync>) -> Result<Arc<LocalRun>, ErrorReply> {
        let program = engine.program().map_err(|e| ErrorReply { code: Some(ErrorCode::Unsupported), message: e.to_string(), capability: None, reason: None })?;
        let models_dir = engine.models_dir.clone().unwrap_or_else(|| paths::app_models(app));
        let store = engine.store.clone().unwrap_or_else(paths::model_store);
        let options = EngineOptions::new(program, models_dir).model_store(store).stderr(Stderr::lines(|line| log::debug!("engine: {line}")));
        let started = sayso_engine_sdk::Engine::start(options).map_err(|e| ErrorReply { code: Some(ErrorCode::Unsupported), message: format!("the engine of this app did not start: {e}"), capability: None, reason: None })?;
        let adapter = LocalEngine::new(started, timing.keep_model, downloads);
        // No dictation here, so `begin` is never called, and live sessions still come before jobs.
        let gate = Arc::new(DictationGate::new());
        let jobs = JobQueue::start(Arc::new(JobRunner::new(adapter.clone(), adapter.clone())), gate.clone()).map_err(|e| failed(format!("cannot start a thread: {e}")))?;
        let capture = capture.cloned().unwrap_or_else(real_capture);
        let recordings = recordings_folder().map_err(|e| failed(format!("cannot make a folder for the recordings: {e}")))?;
        let live = LiveSessions::new(LiveDeps {
            engine: adapter.clone(),
            microphone: Some(capture.microphone),
            system_audio: Some(capture.system_audio),
            gate,
            models: adapter.clone(),
            recordings: Arc::new(RecordingFolder::new(recordings.path().to_path_buf())),
        });
        Ok(Arc::new(LocalRun { engine: adapter, jobs, live, _recordings: recordings }))
    }

    pub fn is_running(&self) -> bool {
        self.engine.is_running()
    }

    pub fn engine_features(&self) -> Vec<String> {
        self.engine.features()
    }

    /// The models that the engine has now.
    pub fn models(&self, choices: &ModelChoices) -> Result<LocalModels, ErrorReply> {
        LocalModels::read(&self.engine, choices)
    }

    pub fn download(&self, model: &str, choices: &ModelChoices) -> Result<(), ErrorReply> {
        let Some(info) = self.models(choices)?.find(model) else {
            return Err(ErrorReply { code: Some(ErrorCode::UnknownModel), message: format!("there is no model {model} for this computer"), capability: None, reason: None });
        };
        if matches!(info.state, ModelState::Downloaded | ModelState::Optimizing | ModelState::Ready) {
            return Ok(());
        }
        self.engine.download(model, info.size_bytes)
    }

    /// Add a job to the queue. None after a close.
    pub fn submit(&self, spec: JobSpec, sink: Arc<dyn EventSink>) -> Option<u64> {
        self.jobs.submit(CONNECTION, spec, sink, |_| {})
    }

    pub fn cancel(&self, job: u64) {
        self.jobs.cancel(CONNECTION, job);
    }

    /// Start a session. The capture of the source `system` can wait for the answer of the user,
    /// so call this without the lock of the SDK.
    pub fn start_session(&self, app: &str, spec: SessionSpec, sink: Arc<dyn EventSink>) -> Result<Started, LiveError> {
        self.live.start(Owner { connection: CONNECTION, app_id: app.to_string(), pid: None }, spec, sink)
    }

    pub fn audio(&self, session: u64, pcm: &str) -> Result<(), LiveError> {
        self.live.audio(CONNECTION, session, pcm)
    }

    pub fn stop_session(&self, session: u64) -> Result<String, LiveError> {
        self.live.stop(CONNECTION, session)
    }
}

impl Drop for LocalRun {
    fn drop(&mut self) {
        self.live.close();
    }
}

fn failed(message: String) -> ErrorReply {
    ErrorReply { code: None, message, capability: None, reason: None }
}

/// The real capture, in the process of the app. On macOS the system asks the user for the app,
/// so it needs `NSMicrophoneUsageDescription` and `NSAudioCaptureUsageDescription`.
fn real_capture() -> CaptureSources {
    CaptureSources { microphone: Arc::new(LiveMicrophone { device: None }), system_audio: Arc::new(real_system_audio()) }
}

/// Local mode has one process, so the capture leaves out `app` (`excludes: ["app"]`).
fn real_system_audio() -> LiveSystemAudio {
    LiveSystemAudio { this_process: "app" }
}

/// The source `system` can start with the real capture. It never asks the user.
pub(crate) fn real_system_audio_usable() -> bool {
    system::supported() && system::access() != system::Access::Denied
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn the_folder_of_the_recordings_is_private_and_has_a_random_name() {
        use std::os::unix::fs::PermissionsExt;
        let (one, two) = (recordings_folder().unwrap(), recordings_folder().unwrap());
        assert_ne!(one.path(), two.path());
        for folder in [&one, &two] {
            assert_eq!(std::fs::metadata(folder.path()).unwrap().permissions().mode() & 0o777, 0o700);
            let name = folder.path().file_name().unwrap().to_string_lossy().into_owned();
            assert!(!name.contains(&std::process::id().to_string()), "{name}: the name does not say the process");
        }
    }

    #[test]
    fn local_mode_names_this_process_app() {
        assert_eq!(real_system_audio().this_process, "app");
    }
}
