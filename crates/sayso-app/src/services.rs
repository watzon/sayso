//! The backends the model uses. Created once at startup.

use sayso_core::enhance::LanguageModel;
use sayso_core::stt::EngineEvent;
use sayso_enhance::SecretStore;
use sayso_platform::{Platform, SttBackend};
use sayso_store::Store;
use std::sync::Arc;

pub struct Services {
    pub platform: Platform,
    /// The microphone, shared by dictation and live sessions
    /// (`SAYSO_FAKE_MIC` plays a file instead).
    pub mic: Arc<crate::mic::SharedMic>,
    /// None when the engine binary is missing, or when the engine did not
    /// answer at the start. The UI shows "Engine stopped".
    pub engine: Option<Arc<dyn SttBackend>>,
    /// The events of the engine from its start on, with `Started` or
    /// `StartFailed`. The model takes it when it starts its loops.
    pub engine_events: Option<crossbeam_channel::Receiver<EngineEvent>>,
    /// The language model of the engine, for AI enhancement on this computer.
    /// None when the engine does not run.
    pub language_model: Option<Arc<dyn LanguageModel>>,
    /// Runs cleanup models (S1-mini, Cleanup 1) in the cleanup runtime.
    pub cleaner: Arc<sayso_cleanup_client::cleaner::RuntimeCleaner>,
    /// The process of the cleanup runtime. Stop it before a model file is deleted.
    pub cleanup_runtime: Arc<sayso_cleanup_client::runtime::Runtime>,
    /// None when the database cannot be opened. History is then off.
    pub store: Option<Arc<Store>>,
    pub secrets: Arc<dyn SecretStore>,
    /// None when this build does not look for updates (a build from source).
    /// The model takes it when it starts the updater.
    pub updates: Option<(sayso_update::Options, sayso_update::AtStart)>,
}
