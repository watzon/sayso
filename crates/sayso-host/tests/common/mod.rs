//! Helpers of the integration tests: a host on a real socket with a real
//! identity check (the record of `apps.toml` has the path of the test
//! program, so the test process is the app), and fakes for the engine and
//! for the questions to the user.

// Each test file uses some of these helpers.
#![allow(dead_code)]

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use parking_lot::Mutex;
use sayso_core::config::Config;
use sayso_core::models::{ModelId, ModelInfo};
use sayso_core::enhance::EnhanceError;
use sayso_core::history::EnhanceOutcome;
use sayso_core::pipeline::PipelineOutput;
use sayso_core::stt::{EngineEvent, ModelStatus, SessionOptions, Transcript};
use sayso_core::style::StyleLibrary;
use sayso_extension::{Error, Fields};
use sayso_host::{ApprovalQuestion, Caller, DownloadQuestion, Host, Identify, Peer, HostConfig, HostServices, StyleRequest};
use sayso_platform::{EngineStart, PlatformError, Result, SttBackend};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// The key of the test program in the host.
pub fn id() -> String {
    sayso_host::Identity::Path(this_program()).id()
}

pub struct Setup {
    pub dir: tempfile::TempDir,
    pub socket: PathBuf,
    pub apps: PathBuf,
}

pub fn setup() -> Setup {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = tempfile::tempdir().unwrap();
    let socket = if cfg!(windows) {
        PathBuf::from(format!(r"\\.\pipe\sayso-host-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)))
    } else {
        // The host makes this folder with mode 0700.
        dir.path().join("sayso").join("host.sock")
    };
    let apps = dir.path().join("apps.toml");
    Setup { dir, socket, apps }
}

/// This test program, as the system names it.
pub fn this_program() -> PathBuf {
    std::env::current_exe().unwrap().canonicalize().unwrap()
}

/// Approve the test program, with these grants.
pub fn write_apps(path: &Path, granted: &[&str]) {
    write_app(path, &this_program(), granted);
}

/// Approve `program`, with these grants.
pub fn write_app(path: &Path, program: &Path, granted: &[&str]) {
    let granted: Vec<String> = granted.iter().map(|g| format!("\"{g}\"")).collect();
    // A literal string: Windows paths have backslashes.
    let text = format!("[[apps]]\nname = \"Test\"\npath = '{}'\nallowed = true\ngranted = [{}]\n", program.display(), granted.join(", "));
    std::fs::write(path, text).unwrap();
}

pub fn start_with(setup: &Setup, services: Arc<dyn HostServices>) -> Host {
    Host::start(HostConfig::new(setup.socket.clone(), setup.apps.clone()), services).unwrap()
}

/// A host where each new connection has the next of these callers, and the
/// last one for each connection after them. An error means a program with no
/// identity.
pub fn start_as(setup: &Setup, callers: Vec<std::result::Result<Caller, String>>, services: Arc<dyn HostServices>) -> Host {
    let mut config = HostConfig::new(setup.socket.clone(), setup.apps.clone());
    config.identify = Arc::new(FakeIdentify { callers: Mutex::new(callers.into()) });
    Host::start(config, services).unwrap()
}

/// A host with another cooldown after a question that an app took back.
pub fn start_cooling(setup: &Setup, cooldown: Duration, services: Arc<dyn HostServices>) -> Host {
    let mut config = HostConfig::new(setup.socket.clone(), setup.apps.clone());
    config.approval_cooldown = cooldown;
    Host::start(config, services).unwrap()
}

struct FakeIdentify {
    callers: Mutex<std::collections::VecDeque<std::result::Result<Caller, String>>>,
}

impl Identify for FakeIdentify {
    fn identify(&self, _peer: &Peer) -> std::result::Result<Caller, String> {
        let mut callers = self.callers.lock();
        if callers.len() > 1 { callers.pop_front().unwrap() } else { callers.front().unwrap().clone() }
    }
}

pub fn fields(value: serde_json::Value) -> Fields {
    value.as_object().unwrap().clone()
}

/// The error of a request that the host refused.
pub fn refused(result: Answer) -> sayso_protocol::ErrorReply {
    match result {
        Err(Error::Host(error)) => error,
        other => panic!("expected an error reply, got {other:?}"),
    }
}

pub type Answer = std::result::Result<sayso_extension::Reply, Error>;

pub fn eventually(what: &str, check: impl Fn() -> bool) {
    let start = Instant::now();
    while !check() {
        assert!(start.elapsed() < Duration::from_secs(5), "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// An engine that the test drives. It records the downloads that the host
/// starts and sends the statuses that the test sets.
pub struct FakeEngine {
    statuses: Mutex<std::collections::HashMap<ModelId, ModelStatus>>,
    subscribers: Mutex<Vec<Sender<EngineEvent>>>,
    pub downloads: Mutex<Vec<ModelId>>,
    start: AtomicU8,
    /// The error that `download` gives, when set.
    pub download_error: Mutex<Option<String>>,
}

impl FakeEngine {
    pub fn new() -> Arc<FakeEngine> {
        Arc::new(FakeEngine {
            statuses: Mutex::new(Default::default()),
            subscribers: Mutex::new(Vec::new()),
            downloads: Mutex::new(Vec::new()),
            start: AtomicU8::new(1),
            download_error: Mutex::new(None),
        })
    }

    /// Set a status and tell the subscribers, as the client of the engine does.
    pub fn set(&self, model: &ModelId, status: ModelStatus) {
        self.statuses.lock().insert(model.clone(), status.clone());
        self.emit(EngineEvent::ModelStatus { model: model.clone(), status });
    }

    /// Set a status without an event: the state before the host started.
    pub fn preset(&self, model: &ModelId, status: ModelStatus) {
        self.statuses.lock().insert(model.clone(), status);
    }

    pub fn emit(&self, event: EngineEvent) {
        self.subscribers.lock().retain(|tx| tx.send(event.clone()).is_ok());
    }

    pub fn set_start(&self, state: EngineStart) {
        self.start.store(
            match state {
                EngineStart::Starting => 0,
                EngineStart::Started => 1,
                EngineStart::Failed => 2,
            },
            Ordering::SeqCst,
        );
    }

    pub fn wait_for_downloads(&self, count: usize) {
        eventually("the host starts the download", || self.downloads.lock().len() >= count);
    }
}

fn unused<T>() -> Result<T> {
    Err(PlatformError::Failed("the fake engine does not do this".into()))
}

impl SttBackend for FakeEngine {
    fn start_state(&self) -> EngineStart {
        match self.start.load(Ordering::SeqCst) {
            0 => EngineStart::Starting,
            1 => EngineStart::Started,
            _ => EngineStart::Failed,
        }
    }
    fn catalog(&self) -> Vec<ModelInfo> {
        Vec::new()
    }
    fn status(&self, model: &ModelId) -> ModelStatus {
        self.statuses.lock().get(model).cloned().unwrap_or(ModelStatus::NotDownloaded)
    }
    fn download(&self, model: &ModelId) -> Result<()> {
        if let Some(message) = self.download_error.lock().clone() {
            return Err(PlatformError::Failed(message));
        }
        self.downloads.lock().push(model.clone());
        Ok(())
    }
    fn delete(&self, _: &ModelId) -> Result<()> {
        unused()
    }
    fn load(&self, _: &ModelId) -> Result<()> {
        unused()
    }
    fn unload(&self, _: &ModelId, _: bool) -> Result<()> {
        unused()
    }
    fn preview_ready(&self, _: &ModelId) -> bool {
        false
    }
    fn start_stream(&self, _: u64, _: &SessionOptions) -> Result<()> {
        unused()
    }
    fn push_audio(&self, _: u64, _: &[f32]) -> Result<()> {
        unused()
    }
    fn end_stream(&self, _: u64) {}
    fn transcribe(&self, _: &[f32], _: &SessionOptions) -> Result<Transcript> {
        unused()
    }
    fn subscribe(&self) -> Receiver<EngineEvent> {
        let (tx, rx) = unbounded();
        self.subscribers.lock().push(tx);
        rx
    }
}

/// The services of the app, with an engine and a user that the test drives.
/// `ask_download` records the question and waits for `answer`, or until the
/// host withdraws the question. `run_style` records the style step and gives
/// `[<style>] <text>`, or fails with `style_error`.
pub struct FakeServices {
    pub engine: Option<Arc<FakeEngine>>,
    pub config: Mutex<Config>,
    pub asked: Mutex<Vec<DownloadQuestion>>,
    /// The questions that the host withdrew before an answer.
    pub withdrawn: AtomicUsize,
    answers: Receiver<bool>,
    answer_in: Sender<bool>,
    /// Each approval question that showed.
    pub approvals: Mutex<Vec<ApprovalQuestion>>,
    /// The approval questions that the host withdrew before an answer.
    pub approvals_withdrawn: AtomicUsize,
    /// True while an approval question shows. Tests read the most at one time.
    pub approvals_showing: AtomicUsize,
    pub approvals_most: AtomicUsize,
    /// The styles of the user. None: no AI.
    pub styles: Mutex<Option<StyleLibrary>>,
    /// Each style step: (style id, style prompt, provider id, text).
    pub style_runs: Mutex<Vec<(String, String, String, String)>>,
    pub style_error: Mutex<Option<EnhanceError>>,
}

impl FakeServices {
    pub fn new(engine: Option<Arc<FakeEngine>>) -> Arc<FakeServices> {
        let (answer_in, answers) = unbounded();
        Arc::new(FakeServices {
            engine,
            config: Mutex::new(Config::default()),
            asked: Mutex::new(Vec::new()),
            withdrawn: AtomicUsize::new(0),
            answers,
            answer_in,
            approvals: Mutex::new(Vec::new()),
            approvals_withdrawn: AtomicUsize::new(0),
            approvals_showing: AtomicUsize::new(0),
            approvals_most: AtomicUsize::new(0),
            styles: Mutex::new(None),
            style_runs: Mutex::new(Vec::new()),
            style_error: Mutex::new(None),
        })
    }

    /// The answer of the user to the next question, now or later.
    pub fn answer(&self, yes: bool) {
        self.answer_in.send(yes).unwrap();
    }

    pub fn wait_for_approvals(&self, count: usize) {
        eventually("the host asks the user about an app", || self.approvals.lock().len() >= count);
    }

    pub fn wait_for_questions(&self, count: usize) {
        eventually("the host asks the user", || self.asked.lock().len() >= count);
    }
}

impl HostServices for FakeServices {
    fn engine(&self) -> Option<Arc<dyn SttBackend>> {
        self.engine.clone().map(|engine| engine as Arc<dyn SttBackend>)
    }
    fn config(&self) -> Config {
        self.config.lock().clone()
    }
    fn ask_download(&self, question: &DownloadQuestion, withdrawn: &dyn Fn() -> bool) -> bool {
        self.asked.lock().push(question.clone());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match self.answers.recv_timeout(Duration::from_millis(20)) {
                Ok(answer) => return answer,
                Err(RecvTimeoutError::Timeout) if withdrawn() => {
                    self.withdrawn.fetch_add(1, Ordering::SeqCst);
                    return false;
                }
                Err(RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
                Err(_) => return false,
            }
        }
    }
    fn ask_approval(&self, question: &ApprovalQuestion, withdrawn: &dyn Fn() -> bool) -> Option<bool> {
        let showing = self.approvals_showing.fetch_add(1, Ordering::SeqCst) + 1;
        self.approvals_most.fetch_max(showing, Ordering::SeqCst);
        self.approvals.lock().push(question.clone());
        let deadline = Instant::now() + Duration::from_secs(10);
        let answer = loop {
            match self.answers.recv_timeout(Duration::from_millis(20)) {
                Ok(answer) => break Some(answer),
                Err(RecvTimeoutError::Timeout) if withdrawn() => {
                    self.approvals_withdrawn.fetch_add(1, Ordering::SeqCst);
                    break None;
                }
                Err(RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
                Err(_) => break None,
            }
        };
        self.approvals_showing.fetch_sub(1, Ordering::SeqCst);
        answer
    }
    fn styles(&self) -> Option<StyleLibrary> {
        self.styles.lock().clone()
    }
    fn run_style(&self, request: &StyleRequest<'_>) -> PipelineOutput {
        let provider_id = request.provider.id.clone();
        self.style_runs.lock().push((request.style.id.clone(), request.style.prompt.clone(), provider_id.clone(), request.text.to_string()));
        let (text, enhance, enhance_error) = match self.style_error.lock().clone() {
            Some(error) => (request.text.to_string(), EnhanceOutcome::Failed { provider_id: provider_id.clone(), reason: error.to_string() }, Some((provider_id, error))),
            None => (format!("[{}] {}", request.style.id, request.text), EnhanceOutcome::Applied { provider_id, model: "fake-model".into(), elapsed_ms: 1 }, None),
        };
        PipelineOutput { text, replacements: Vec::new(), enhance, enhance_error, timings: None, notice: None }
    }
}
