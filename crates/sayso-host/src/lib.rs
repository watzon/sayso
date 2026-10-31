//! The host of connected apps: the server of the Host API
//! (`docs/extensions.md`, "Host API", and `docs/sdk.md`, "Approval").
//!
//! An app connects to a local socket (a named pipe on Windows) and sends
//! NDJSON messages of `sayso-protocol`. The host knows the app by its
//! executable ([`Identify`]) and the answer of the user in `apps.toml`
//! ([`AppsFile`]), and checks the capabilities of each request.
//!
//! The host has no GPUI. It gets the engine, the capture, and the questions
//! to the user from `sayso-app` through [`HostServices`], so it runs and is
//! tested without a window.

mod apps;
mod approval;
mod backend;
#[cfg(target_os = "linux")]
mod bridge;
mod bus;
mod connection;
mod download;
pub mod enhance;
mod holds;
mod identity;
mod job_requests;
pub mod live;
mod live_requests;
mod models;
mod responder;
pub mod jobs;
mod sandbox;
mod transport;

pub use enhance::StyleRequest;
pub use holds::{Hold, Work};
pub use approval::ApprovalQuestion;
pub use apps::{AppRecord, Apps, AppsFile};
pub use identity::{Caller, Identify, Identity, Peer, SystemIdentify, clean_name, visible_path, visible_text};
/// The event of [`Host::dictation_done`], for the app.
pub use sayso_protocol::DictationDone;

use parking_lot::Mutex;
use sayso_core::config::Config;
use sayso_core::enhance::EnhanceError;
use sayso_core::history::EnhanceOutcome;
use sayso_core::models::ModelId;
use sayso_core::paths::Paths;
use sayso_core::pipeline::PipelineOutput;
use sayso_core::style::StyleLibrary;
use sayso_platform::SttBackend;
use sayso_protocol::{Capability, Event};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

/// What the host gets from the app.
///
/// The host calls these methods from its own threads, never from the thread
/// of a window.
pub trait HostServices: Send + Sync {
    /// The speech engine, or `None` when it does not run. The host reads it
    /// once, when it starts. An engine that restarts keeps its handle.
    fn engine(&self) -> Option<Arc<dyn SttBackend>>;

    /// The settings of the user now (the final model, the preview model, and
    /// the VAD and diarization models). The host reads them again for each
    /// request, so a change counts at once.
    fn config(&self) -> Config;

    /// Ask the user whether an app may download a model. Blocks until the
    /// user answers. True means download. A user who closes the question
    /// without an answer says no.
    ///
    /// `withdrawn` becomes true when the app disconnects or Sayso closes.
    /// Then close the question, as a Cancel, and return false. Look at it a
    /// few times each second.
    fn ask_download(&self, question: &DownloadQuestion, withdrawn: &dyn Fn() -> bool) -> bool;

    /// Ask the user whether an app may use Sayso. Blocks until the user
    /// answers. `Some(true)` is **Allow**, and `Some(false)` is **Don't allow**
    /// or a question that the user closed. The host keeps both.
    ///
    /// `withdrawn` becomes true when each connection of the app has closed or
    /// Sayso closes. Then close the question and return `None`: the host keeps
    /// no answer, and Sayso asks again at the next connection. Also `None` when
    /// the question cannot show. Look at it a few times each second.
    fn ask_approval(&self, question: &ApprovalQuestion, withdrawn: &dyn Fn() -> bool) -> Option<bool>;

    /// The styles of the user and the base prompt, for `enhance`. Read again
    /// for each request. None: this host has no AI, and `enhance` gets
    /// `unavailable`.
    fn styles(&self) -> Option<StyleLibrary> {
        None
    }

    /// Run a style on text, with the code of the style step of a dictation,
    /// for `enhance`. Blocks until the provider answers or its time ends.
    fn run_style(&self, request: &StyleRequest<'_>) -> PipelineOutput {
        let error = EnhanceError::NotConfigured("this host cannot run styles".into());
        PipelineOutput {
            text: request.text.to_string(),
            replacements: Vec::new(),
            enhance: EnhanceOutcome::Failed { provider_id: request.provider.id.clone(), reason: error.to_string() },
            enhance_error: Some((request.provider.id.clone(), error)),
            timings: None,
            notice: None,
        }
    }

    /// The microphone that the user chose, shared with dictation, for live
    /// sessions with the source `mic`. None: this host cannot capture it, and
    /// `session_start` with `mic` gets `unsupported`.
    fn microphone(&self) -> Option<Arc<dyn live::Microphone>> {
        None
    }

    /// The system audio, for live sessions with the source `system`. None:
    /// `session_start` with `system` gets `unsupported`. Sayso asks the system
    /// for the permission the first time a session opens it.
    fn system_audio(&self) -> Option<Arc<dyn live::SystemAudio>> {
        None
    }

    /// The folder for the recordings of live sessions without `record_to`.
    /// The host deletes each recording after its session.
    fn live_temp_dir(&self) -> PathBuf {
        std::env::temp_dir().join("sayso-live")
    }
}

/// The question to the user before the download of a model for an app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadQuestion {
    pub app_id: String,
    /// The name to show: the name in `apps.toml`.
    pub app_name: String,
    pub model_id: String,
    pub model_name: String,
    pub size_bytes: u64,
    /// Why the app needs the model. It is text from the app, and the host
    /// cuts it to 300 characters.
    pub reason: String,
}

/// Where the host listens and what it reads.
pub struct HostConfig {
    /// The socket, or the name of the pipe on Windows.
    pub socket: PathBuf,
    /// `apps.toml`.
    pub apps: Arc<AppsFile>,
    /// The version of Sayso, for the `hello` reply.
    pub host_version: String,
    pub identify: Arc<dyn Identify>,
    /// How long an identity gets no new question after it took one back.
    pub approval_cooldown: std::time::Duration,
    /// A Sayso in a Flatpak: where `sayso-bridge` listens outside the sandbox.
    pub bridge_socket: Option<PathBuf>,
}

impl HostConfig {
    pub fn new(socket: PathBuf, apps_file: PathBuf) -> HostConfig {
        HostConfig { socket, apps: Arc::new(AppsFile::new(apps_file)), host_version: env!("CARGO_PKG_VERSION").into(), identify: Arc::new(SystemIdentify), approval_cooldown: approval::COOLDOWN, bridge_socket: None }
    }

    /// The socket of `sayso_protocol::socket`, and `apps.toml` in the data folder.
    pub fn for_paths(paths: &Paths) -> io::Result<HostConfig> {
        let mut config = HostConfig::new(sayso_protocol::socket::host_socket_path()?, paths.apps_file());
        if sayso_core::flatpak::app_id().is_some() {
            config.bridge_socket = Some(sayso_protocol::socket::client_socket_path()?);
        }
        Ok(config)
    }
}

/// The running host. [`Host::close`] (or drop) stops it.
pub struct Host {
    socket: PathBuf,
    shared: Arc<Shared>,
    listener: Arc<transport::Listener>,
    accept: Mutex<Option<JoinHandle<()>>>,
    #[cfg(target_os = "linux")]
    bridge: Mutex<Option<bridge::Bridge>>,
}

pub(crate) struct Shared {
    pub host_version: String,
    pub identify: Arc<dyn Identify>,
    pub apps: Arc<AppsFile>,
    /// The connections that wait for the approval question.
    pub approvals: approval::Approvals,
    pub services: Arc<dyn HostServices>,
    /// `services.engine()` at the start.
    pub engine: Option<Arc<dyn SttBackend>>,
    /// The one subscription to the events of the engine.
    pub bus: Arc<bus::Bus>,
    /// The models that the host asked the engine to download, until the
    /// request that asked ends (see `download`).
    pub downloads: Mutex<HashSet<ModelId>>,
    /// One lock for each app: the one download question at a time.
    questions: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Holds jobs between two engine calls while a dictation runs.
    pub gate: Arc<jobs::DictationGate>,
    /// The models that jobs use, for the memory policy of the app.
    pub model_use: Arc<jobs::ModelUse>,
    /// The queue of jobs. None without an engine.
    pub jobs: Option<jobs::JobQueue>,
    /// The live sessions. None without an engine.
    pub live: Option<live::LiveSessions>,
    pub closing: AtomicBool,
    /// The open connections by a number of the host.
    pub connections: Mutex<HashMap<u64, connection::Entry>>,
    next_connection: AtomicU64,
    /// The observers of the app: the live session (the menu bar) and the
    /// work that holds Sayso open (the quit question and the update).
    pub observers: holds::Observers,
}

impl Shared {
    pub fn is_closing(&self) -> bool {
        self.closing.load(Ordering::SeqCst)
    }

    /// See [`Host::stop_work_without_grant`]. A session in its start and a job
    /// in its checks are not there yet, so these requests call this too when
    /// their work runs.
    pub fn stop_work_without_grant(&self) {
        let apps = self.apps.current();
        let has_all = |app_id: &str, needed: &[Capability]| {
            let granted = apps.record(app_id).filter(|record| record.allowed).map(|record| record.capabilities()).unwrap_or_default();
            needed.iter().all(|capability| granted.contains(capability))
        };
        if let Some(live) = self.live.as_ref().and_then(live::LiveSessions::current).filter(|live| !live.stopping)
            && !has_all(&live.app_id, &sayso_protocol::session_capabilities(&live.sources))
        {
            log::info!("host: live session {} stops: the grants of its app changed", live.session);
            self.live.as_ref().map(|sessions| sessions.stop_for_user(live.session));
        }
        let Some(jobs) = &self.jobs else { return };
        let without_grant: Vec<u64> = {
            let connections = self.connections.lock();
            let mut numbers = jobs.connections();
            numbers.sort_unstable();
            numbers.dedup();
            numbers.retain(|number| connections.get(number).and_then(|entry| entry.app_id.as_deref()).is_some_and(|app_id| !has_all(app_id, &[Capability::TranscribeFile])));
            numbers
        };
        for number in without_grant {
            log::info!("host: the jobs of connection {number} stop: the grants of its app changed");
            jobs.grant_gone(number);
        }
    }

    /// The lock that an app holds while a download question to the user is open.
    pub fn question_gate(&self, app_id: &str) -> Arc<Mutex<()>> {
        self.questions.lock().entry(app_id.to_string()).or_default().clone()
    }
}

impl Host {
    /// Listen on the socket of `config`. Fails when the socket cannot be made,
    /// for example when another Sayso serves apps.
    pub fn start(config: HostConfig, services: Arc<dyn HostServices>) -> io::Result<Host> {
        let listener = Arc::new(transport::Listener::bind(&config.socket)?);
        let engine = services.engine();
        let gate = Arc::new(jobs::DictationGate::new());
        let model_use = jobs::ModelUse::new();
        let queue = match &engine {
            Some(engine) => Some(jobs::JobQueue::start(Arc::new(jobs::JobRunner::new(engine.clone(), model_use.clone())), gate.clone())?),
            None => None,
        };
        let bus = Arc::new(bus::Bus::start(engine.as_ref()));
        let live = engine.as_ref().map(|engine| {
            live::LiveSessions::new(live::LiveDeps {
                engine: Arc::new(live::BackendEngine::new(engine.clone(), Some(bus.clone()))),
                microphone: services.microphone(),
                system_audio: services.system_audio(),
                gate: gate.clone(),
                models: model_use.clone(),
                recordings: Arc::new(live::RecordingFolder::new(services.live_temp_dir())),
            })
        });
        let observers = holds::Observers::default();
        if let Some(live) = &live {
            live.set_observer(Arc::new(observers.clone()));
        }
        if let Some(jobs) = &queue {
            let observers = observers.clone();
            jobs.set_on_change(Arc::new(move || observers.holds_changed()));
        }
        let shared = Arc::new(Shared {
            host_version: config.host_version,
            identify: config.identify,
            apps: config.apps,
            approvals: approval::Approvals::new(config.approval_cooldown),
            bus,
            engine,
            services,
            downloads: Mutex::new(HashSet::new()),
            questions: Mutex::new(HashMap::new()),
            gate,
            model_use,
            jobs: queue,
            live,
            closing: AtomicBool::new(false),
            connections: Mutex::new(HashMap::new()),
            next_connection: AtomicU64::new(1),
            observers,
        });
        let accept = {
            let shared = shared.clone();
            let listener = listener.clone();
            std::thread::Builder::new().name("sayso-host".into()).spawn(move || accept_loop(&shared, &listener))?
        };
        log::info!("host: apps connect at {}", config.socket.display());
        #[cfg(target_os = "linux")]
        let bridge = config.bridge_socket.and_then(|socket| bridge::Bridge::start(shared.clone(), socket));
        Ok(Host {
            socket: config.socket,
            shared,
            listener,
            accept: Mutex::new(Some(accept)),
            #[cfg(target_os = "linux")]
            bridge: Mutex::new(bridge),
        })
    }

    /// The keys of the apps that are connected now, in order.
    pub fn connected_apps(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.shared.connections.lock().values().filter_map(|entry| entry.app_id.clone()).collect();
        ids.sort();
        ids
    }

    /// Send `host_closing` to each connection, close them, and remove the
    /// socket. A second call does nothing.
    pub fn close(&self) {
        let writers = {
            let mut connections = self.shared.connections.lock();
            if self.shared.closing.swap(true, Ordering::SeqCst) {
                return;
            }
            // Each job ends with `job_failed` before `host_closing`. The live
            // session is cancelled: no more parts and no diarization.
            if let Some(jobs) = &self.shared.jobs {
                jobs.close();
            }
            if let Some(live) = &self.shared.live {
                live.close();
            }
            connections.values_mut().filter_map(connection::Entry::close_with_host_closing).collect::<Vec<_>>()
        };
        #[cfg(target_os = "linux")]
        if let Some(bridge) = self.bridge.lock().take() {
            bridge.close();
        }
        if self.listener.wake()
            && let Some(accept) = self.accept.lock().take()
        {
            let _ = accept.join();
        }
        // Each writer stops after its last message, or after its timeout.
        for writer in writers {
            let _ = writer.join();
        }
        self.listener.remove();
        self.shared.bus.stop();
        log::info!("host: closed");
    }

    /// Take the connections of a `sayso-bridge` that holds the other end of
    /// `control`, as a Sayso in a Flatpak does with the bridge that it starts.
    /// For the tests.
    #[cfg(target_os = "linux")]
    pub fn attach_bridge(&self, control: std::os::fd::OwnedFd) {
        *self.bridge.lock() = bridge::Bridge::attach(self.shared.clone(), control);
    }

    /// The gate of the jobs. The app calls `begin` when a dictation starts
    /// and `end` when it no longer needs the engine (see [`jobs::DictationGate`]).
    pub fn dictation_gate(&self) -> Arc<jobs::DictationGate> {
        self.shared.gate.clone()
    }

    /// The models that jobs use. The memory policy of the app does not
    /// unload them (see [`jobs::ModelUse::wanted`]).
    pub fn model_use(&self) -> Arc<jobs::ModelUse> {
        self.shared.model_use.clone()
    }

    /// The live session that runs now: its app and whether it captures the
    /// microphone, for the menu bar.
    pub fn live_session(&self) -> Option<live::LiveInfo> {
        self.shared.live.as_ref().and_then(live::LiveSessions::current)
    }

    /// **Stop** in the popover: end the live session. The last parts go on
    /// the connection of the app, then `session_ended`. False when this
    /// session no longer runs.
    pub fn stop_live_session(&self, session: u64) -> bool {
        self.shared.live.as_ref().is_some_and(|live| live.stop_for_user(session))
    }

    /// Stop the live session and the jobs of an app that no longer has the
    /// grants for them, or no longer has a record. Without this, a microphone
    /// that the user turned off for the app would record until the next
    /// request. The app calls it after each change of `apps.toml`.
    pub fn stop_work_without_grant(&self) {
        self.shared.stop_work_without_grant();
    }

    /// `apps.toml`, for Settings.
    pub fn apps(&self) -> Arc<AppsFile> {
        self.shared.apps.clone()
    }

    /// Tell `observer` each time a live session starts, stops, or ends.
    pub fn set_live_observer(&self, observer: Arc<dyn live::LiveObserver>) {
        self.shared.observers.set_live(observer);
    }

    /// The apps that hold Sayso open now: the live session first, then the
    /// jobs that run or wait, one entry for each app and kind of work. Sayso asks before it quits, and does not install an update,
    /// while this is not empty (`docs/extensions.md`, "Lifecycle").
    pub fn holds(&self) -> Vec<Hold> {
        holds::holds(&self.shared)
    }

    /// Call `observer` each time [`Host::holds`] can have changed: a live
    /// session or a job starts or ends. It runs on a thread of the host and
    /// must not block.
    pub fn set_holds_observer(&self, observer: Arc<dyn Fn() + Send + Sync>) {
        self.shared.observers.set_holds(observer);
    }

    /// A dictation was inserted. Each connection that subscribed to
    /// `dictation_done` gets the event, when its extension still has the
    /// capability `dictation.observe`. Nothing goes out when incognito was on
    /// at the start or at the end of the dictation: the rule of history
    /// (`sayso_core::dictation::incognito_during`, `docs/extensions.md`, "Privacy").
    pub fn dictation_done(&self, done: DictationDone, incognito_at_start: bool, incognito_at_end: bool) {
        if sayso_core::dictation::incognito_during(incognito_at_start, incognito_at_end) {
            return;
        }
        connection::send_to_subscribers(&self.shared, connection::DICTATION_DONE, &Event::DictationDone(done));
    }

    /// The socket, or the name of the pipe on Windows.
    pub fn socket(&self) -> &Path {
        &self.socket
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.close();
    }
}

fn accept_loop(shared: &Arc<Shared>, listener: &transport::Listener) {
    loop {
        let accepted = listener.accept();
        if shared.closing.load(Ordering::SeqCst) {
            return;
        }
        match accepted {
            Ok(conn) => serve_connection(shared, conn),
            Err(e) => {
                log::warn!("host: accept failed: {e}");
                // Do not spin when the listener is broken.
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
    }
}

/// Serve one connection on a thread of its own.
fn serve_connection(shared: &Arc<Shared>, conn: transport::Conn) {
    if shared.connections.lock().len() >= connection::MAX_CONNECTIONS {
        log::warn!("host: too many connections, one more is closed");
        (conn.closer)();
        return;
    }
    let number = shared.next_connection.fetch_add(1, Ordering::SeqCst);
    let shared = shared.clone();
    let spawned = std::thread::Builder::new().name("sayso-host-read".into()).spawn(move || connection::serve(&shared, number, conn));
    if let Err(e) = spawned {
        log::warn!("host: cannot start a connection thread: {e}");
    }
}
