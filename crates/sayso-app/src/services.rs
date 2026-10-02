//! The backends the model uses. Created once at startup.

use sayso_enhance::SecretStore;
use sayso_platform::{Platform, SttBackend};
use sayso_store::Store;
use std::sync::Arc;

pub struct Services {
    pub platform: Platform,
    /// None when the engine binary is missing. The UI shows "Engine stopped".
    pub engine: Option<Arc<dyn SttBackend>>,
    /// None when the database cannot be opened. History is then off.
    pub store: Option<Arc<Store>>,
    pub secrets: Arc<dyn SecretStore>,
}
