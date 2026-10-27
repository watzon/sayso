//! The host of extensions (`sayso-host`, `docs/extensions.md`). It starts
//! after the services and closes when Sayso quits.

use crate::download_ask::Ask;
use crate::services::Services;
use sayso_core::config::Config;
use sayso_core::paths::Paths;
use sayso_host::DownloadQuestion;
use sayso_platform::SttBackend;
use std::path::PathBuf;
use std::sync::Arc;

/// What the host gets from the app: the engine, the settings of the user, and
/// the questions to the user. Later sub-steps of step 6 add the capture and
/// the AI providers.
struct AppHostServices {
    engine: Option<Arc<dyn SttBackend>>,
    config_file: PathBuf,
    /// The questions go to the thread of the windows (`download_ask`).
    questions: async_channel::Sender<Ask>,
}

impl sayso_host::HostServices for AppHostServices {
    fn engine(&self) -> Option<Arc<dyn SttBackend>> {
        self.engine.clone()
    }

    /// Sayso writes the file at each change of a setting, so the file is the
    /// settings now. The host reads it for each request that needs it.
    fn config(&self) -> Config {
        match Config::load(&self.config_file) {
            Ok(loaded) => loaded.config,
            Err(e) => {
                log::warn!("host: {e}. The defaults count for this request.");
                Config::default()
            }
        }
    }

    fn ask_download(&self, question: &DownloadQuestion) -> bool {
        let (answer, answered) = crossbeam_channel::bounded(1);
        if self.questions.send_blocking(Ask { question: question.clone(), answer }).is_err() {
            return false;
        }
        // The window closes without an answer when Sayso quits. That is no.
        answered.recv().unwrap_or(false)
    }
}

/// The host, and the questions that it asks the user. The app shows each
/// question as a window (see `download_ask::listen`).
pub struct Started {
    pub host: sayso_host::Host,
    pub questions: async_channel::Receiver<Ask>,
}

/// Start the host. When that fails, Sayso runs without extensions.
pub fn start(paths: &Paths, services: &Services) -> Option<Started> {
    let (questions, asked) = async_channel::unbounded();
    let host_services = AppHostServices { engine: services.engine.clone(), config_file: paths.config_file(), questions };
    let started = sayso_host::HostConfig::for_paths(paths).and_then(|mut config| {
        config.host_version = env!("CARGO_PKG_VERSION").into();
        sayso_host::Host::start(config, Arc::new(host_services))
    });
    started.map(|host| Started { host, questions: asked }).map_err(|e| log::warn!("extensions cannot connect: {e}")).ok()
}
