//! The host of extensions (`sayso-host`, `docs/extensions.md`). It starts
//! after the services and closes when Sayso quits.

use crate::download_ask::Ask;
use crate::mic::SharedMic;
use crate::services::Services;
use sayso_core::config::Config;
use sayso_core::paths::Paths;
use sayso_host::DownloadQuestion;
use sayso_host::live::{MicFeed, Microphone};
use sayso_platform::SttBackend;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What the host gets from the app: the engine, the settings of the user, the
/// questions to the user, and the shared microphone. A later sub-step of step
/// 6 adds the AI providers.
struct AppHostServices {
    engine: Option<Arc<dyn SttBackend>>,
    config_file: PathBuf,
    mic: Arc<SharedMic>,
    /// The questions go to the thread of the windows (`download_ask`).
    questions: async_channel::Sender<Ask>,
}

impl AppHostServices {
    fn load_config(config_file: &Path) -> Config {
        match Config::load(config_file) {
            Ok(loaded) => loaded.config,
            Err(e) => {
                log::warn!("host: {e}. The defaults count for this request.");
                Config::default()
            }
        }
    }
}

impl sayso_host::HostServices for AppHostServices {
    fn engine(&self) -> Option<Arc<dyn SttBackend>> {
        self.engine.clone()
    }

    /// Sayso writes the file at each change of a setting, so the file is the
    /// settings now. The host reads it for each request that needs it.
    fn config(&self) -> Config {
        Self::load_config(&self.config_file)
    }

    fn microphone(&self) -> Option<Arc<dyn Microphone>> {
        Some(Arc::new(HostMic { mic: self.mic.clone(), config_file: self.config_file.clone() }))
    }

    fn ask_download(&self, question: &DownloadQuestion, _withdrawn: &dyn Fn() -> bool) -> bool {
        let (answer, answered) = crossbeam_channel::bounded(1);
        if self.questions.send_blocking(Ask { question: question.clone(), answer }).is_err() {
            return false;
        }
        // The window closes without an answer when Sayso quits. That is no.
        answered.recv().unwrap_or(false)
    }
}

/// The microphone of live sessions: the one capture of Sayso, which dictation
/// also uses (`mic.rs`), on the device that the user chose in Settings.
struct HostMic {
    mic: Arc<SharedMic>,
    config_file: PathBuf,
}

impl Microphone for HostMic {
    fn open(&self) -> Result<MicFeed, String> {
        let device = AppHostServices::load_config(&self.config_file).audio.input_device;
        let (lease, frames) = self.mic.join_queued(device.as_deref()).map_err(|e| e.to_string())?;
        Ok(MicFeed { frames, lease: Box::new(lease) })
    }
}

/// The host, and the questions that it asks the user. The app shows each
/// question as a window (see `download_ask::listen`).
pub struct Started {
    pub host: sayso_host::Host,
    pub questions: async_channel::Receiver<Ask>,
    pub jobs: HostJobs,
}

/// What the dictation and the memory policy tell the jobs of extensions.
#[derive(Clone)]
pub struct HostJobs {
    /// `begin` while a dictation records or is processed, `end` after.
    pub gate: Arc<sayso_host::jobs::DictationGate>,
    /// The models that jobs use: the app does not unload them.
    pub model_use: Arc<sayso_host::jobs::ModelUse>,
}

/// Start the host. When that fails, Sayso runs without extensions.
pub fn start(paths: &Paths, services: &Services) -> Option<Started> {
    let (questions, asked) = async_channel::unbounded();
    let host_services = AppHostServices { engine: services.engine.clone(), config_file: paths.config_file(), mic: services.mic.clone(), questions };
    let started = sayso_host::HostConfig::for_paths(paths).and_then(|mut config| {
        config.host_version = env!("CARGO_PKG_VERSION").into();
        sayso_host::Host::start(config, Arc::new(host_services))
    });
    started
        .map(|host| {
            let jobs = HostJobs { gate: host.dictation_gate(), model_use: host.model_use() };
            Started { host, questions: asked, jobs }
        }).map_err(|e| log::warn!("extensions cannot connect: {e}")).ok()
}
