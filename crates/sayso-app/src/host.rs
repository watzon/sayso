//! The host of extensions (`sayso-host`, `docs/extensions.md`). It starts
//! after the services and closes when Sayso quits.

use crate::download_ask::Ask;
use crate::mic::SharedMic;
use crate::services::Services;
use crate::style_step::{CleanupContext, StyleStep, style_timeout};
use sayso_cleanup_client::cleaner::RuntimeCleaner;
use sayso_core::config::Config;
use sayso_core::enhance::LanguageModel;
use sayso_core::paths::Paths;
use sayso_core::pipeline::PipelineOutput;
use sayso_core::style::StyleLibrary;
use sayso_enhance::SecretStore;
use sayso_host::{DownloadQuestion, Hold, StyleRequest, Work};
use sayso_host::live::{MicFeed, Microphone};
use sayso_platform::SttBackend;
use sayso_store::Store;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// What the host gets from the app: the engine, the settings of the user, the
/// questions to the user, the shared microphone, and the style step.
struct AppHostServices {
    engine: Option<Arc<dyn SttBackend>>,
    config_file: PathBuf,
    mic: Arc<SharedMic>,
    /// The questions go to the thread of the windows (`download_ask`).
    questions: async_channel::Sender<Ask>,
    styles_dir: PathBuf,
    secrets: Arc<dyn SecretStore>,
    language_model: Option<Arc<dyn LanguageModel>>,
    cleaner: Arc<RuntimeCleaner>,
    /// The dictionary of the user: its words go to the provider, as in a dictation.
    store: Option<Arc<Store>>,
}

/// How often the host thread that waits for an answer looks whether the
/// question is still open.
const WITHDRAWN_CHECK: Duration = Duration::from_millis(250);

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

    /// When the extension disconnects or Sayso closes the host, the window
    /// closes, and that counts as Cancel.
    fn ask_download(&self, question: &DownloadQuestion, withdrawn: &dyn Fn() -> bool) -> bool {
        let (answer, answered) = crossbeam_channel::bounded(1);
        let (close, closed) = async_channel::bounded(1);
        if self.questions.send_blocking(Ask { question: question.clone(), answer, withdraw: closed }).is_err() {
            return false;
        }
        loop {
            match answered.recv_timeout(WITHDRAWN_CHECK) {
                Ok(yes) => return yes,
                // The window closed without an answer (Sayso quits). That is no.
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return false,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    if withdrawn() {
                        let _ = close.try_send(());
                        return false;
                    }
                }
            }
        }
    }

    /// The styles of the user, read again for each request: the files can
    /// change while Sayso runs.
    fn styles(&self) -> Option<StyleLibrary> {
        Some(StyleLibrary::load(&self.styles_dir))
    }

    /// The style step of a dictation (`style_step`), with the settings of now.
    fn run_style(&self, request: &StyleRequest<'_>) -> PipelineOutput {
        let config = self.config();
        let vocabulary = self.store.as_ref().and_then(|store| store.list_words().ok()).unwrap_or_default().into_iter().map(|word| word.text).collect();
        // A cleanup model reads the language of a dictation. Text from an
        // extension has the language setting of dictation.
        let cleanup = CleanupContext::for_provider(&self.cleaner, &config.speech, request.provider, None, None, config.dictation.language.clone(), &config.dictation.model);
        let step = StyleStep {
            timeout: style_timeout(&config.ai, request.provider),
            ai: config.ai,
            secrets: self.secrets.clone(),
            engine: self.language_model.clone(),
            vocabulary,
            base_prompt: request.base_prompt.to_string(),
            cleanup,
        };
        step.run(request.style, request.provider, request.text.to_string(), Vec::new())
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
    pub host: Arc<sayso_host::Host>,
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
    let host_services = AppHostServices {
        engine: services.engine.clone(),
        config_file: paths.config_file(),
        mic: services.mic.clone(),
        questions,
        styles_dir: paths.styles_dir(),
        secrets: services.secrets.clone(),
        language_model: services.language_model.clone(),
        cleaner: services.cleaner.clone(),
        store: services.store.clone(),
    };
    let started = sayso_host::HostConfig::for_paths(paths).and_then(|mut config| {
        config.host_version = env!("CARGO_PKG_VERSION").into();
        sayso_host::Host::start(config, Arc::new(host_services))
    });
    started
        .map(|host| {
            let jobs = HostJobs { gate: host.dictation_gate(), model_use: host.model_use() };
            Started { host: Arc::new(host), questions: asked, jobs }
        }).map_err(|e| log::warn!("extensions cannot connect: {e}")).ok()
}

/// What "Restart to update" does now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateRestart {
    /// Install the update and restart.
    Now,
    /// Wait: this extension (the first one) has a live session or a job.
    WaitFor(String),
    /// Not now: a dictation runs. The button is off then.
    NotNow,
}

/// Sayso does not install an update while an extension has a live session
/// or a job (`docs/extensions.md`, "Lifecycle"). `idle`: no dictation runs.
pub fn update_restart(idle: bool, holds: &[Hold]) -> UpdateRestart {
    match holds.first() {
        Some(hold) => UpdateRestart::WaitFor(hold.name.clone()),
        None if idle => UpdateRestart::Now,
        None => UpdateRestart::NotNow,
    }
}

/// The question before Sayso quits while extensions work: "Notetaker is
/// recording. Quit anyway?". None when no extension works: Sayso quits
/// without a question.
pub fn quit_question(holds: &[Hold]) -> Option<String> {
    let first = holds.first()?;
    Some(match first.work {
        Work::LiveSession => sayso_i18n::t!("extensions.quit_ask.recording", extension = first.name),
        Work::Job => sayso_i18n::t!("extensions.quit_ask.transcribing", extension = first.name),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hold(name: &str, work: Work) -> Hold {
        Hold { extension_id: format!("dev.example.{}", name.to_lowercase()), name: name.into(), work }
    }

    #[test]
    fn an_update_waits_for_the_work_of_extensions() {
        assert_eq!(update_restart(true, &[]), UpdateRestart::Now);
        assert_eq!(update_restart(false, &[]), UpdateRestart::NotNow, "a dictation runs");
        let holds = [hold("Notetaker", Work::LiveSession), hold("Media", Work::Job)];
        assert_eq!(update_restart(true, &holds), UpdateRestart::WaitFor("Notetaker".into()));
        assert_eq!(update_restart(false, &holds[1..]), UpdateRestart::WaitFor("Media".into()));
    }

    #[test]
    fn sayso_asks_before_it_quits_only_while_an_extension_works() {
        sayso_i18n::set_locale("en");
        assert_eq!(quit_question(&[]), None);
        assert_eq!(quit_question(&[hold("Notetaker", Work::LiveSession)]).as_deref(), Some("Notetaker is recording. Quit anyway?"));
        assert_eq!(quit_question(&[hold("Media", Work::Job), hold("Notetaker", Work::LiveSession)]).as_deref(), Some("Media is transcribing a file. Quit anyway?"));
    }
}
