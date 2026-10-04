//! The backends the model uses. Created once at startup.

use sayso_core::enhance::LanguageModel;
use sayso_enhance::SecretStore;
use sayso_platform::{Platform, SttBackend};
use sayso_store::Store;
use std::sync::Arc;

pub struct Services {
    pub platform: Platform,
    /// None when the engine binary is missing. The UI shows "Engine stopped".
    pub engine: Option<Arc<dyn SttBackend>>,
    /// The language model of the engine, for AI enhancement on this computer.
    /// None when the engine does not run.
    pub language_model: Option<Arc<dyn LanguageModel>>,
    /// None when the database cannot be opened. History is then off.
    pub store: Option<Arc<Store>>,
    pub secrets: Arc<dyn SecretStore>,
    /// None when this build does not look for updates (a build from source).
    /// The model takes it when it starts the updater.
    pub updates: Option<(sayso_update::Options, sayso_update::AtStart)>,
}
