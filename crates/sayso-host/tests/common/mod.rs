//! Helpers of the integration tests: a host on a real socket with a real
//! identity check (the record of `state.toml` has the folder of the test
//! program, so the test process is the extension), and fakes for the engine
//! and for the questions to the user.

// Each test file uses some of these helpers.
#![allow(dead_code)]

use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::Mutex;
use sayso_core::config::Config;
use sayso_core::models::{ModelId, ModelInfo};
use sayso_core::stt::{EngineEvent, ModelStatus, SessionOptions, Transcript};
use sayso_extension::{Error, Fields};
use sayso_host::{DownloadQuestion, Host, HostConfig, HostServices};
use sayso_platform::{EngineStart, PlatformError, Result, SttBackend};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::time::{Duration, Instant};

pub const ID: &str = "dev.example.test";

pub struct Setup {
    pub dir: tempfile::TempDir,
    pub socket: PathBuf,
    pub state: PathBuf,
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
    let state = dir.path().join("state.toml");
    Setup { dir, socket, state }
}

/// The folder of this test program.
pub fn here() -> PathBuf {
    std::env::current_exe().unwrap().parent().unwrap().to_path_buf()
}

pub fn write_state(path: &Path, folder: &Path, granted: &[&str], team_id: Option<&str>) {
    let granted: Vec<String> = granted.iter().map(|g| format!("\"{g}\"")).collect();
    let team = team_id.map(|t| format!("team_id = \"{t}\"\n")).unwrap_or_default();
    // A literal string: Windows paths have backslashes.
    let text = format!("[[extensions]]\nid = \"{ID}\"\nversion = \"0.1.0\"\npath = '{}'\nsideloaded = true\ngranted = [{}]\n{team}", folder.display(), granted.join(", "));
    std::fs::write(path, text).unwrap();
}

pub fn start_with(setup: &Setup, services: Arc<dyn HostServices>) -> Host {
    Host::start(HostConfig::new(setup.socket.clone(), setup.state.clone()), services).unwrap()
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
/// `ask_download` records the question and waits for `answer`.
pub struct FakeServices {
    pub engine: Option<Arc<FakeEngine>>,
    pub config: Mutex<Config>,
    pub asked: Mutex<Vec<DownloadQuestion>>,
    answers: Receiver<bool>,
    answer_in: Sender<bool>,
}

impl FakeServices {
    pub fn new(engine: Option<Arc<FakeEngine>>) -> Arc<FakeServices> {
        let (answer_in, answers) = unbounded();
        Arc::new(FakeServices { engine, config: Mutex::new(Config::default()), asked: Mutex::new(Vec::new()), answers, answer_in })
    }

    /// The answer of the user to the next question, now or later.
    pub fn answer(&self, yes: bool) {
        self.answer_in.send(yes).unwrap();
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
    fn ask_download(&self, question: &DownloadQuestion) -> bool {
        self.asked.lock().push(question.clone());
        self.answers.recv_timeout(Duration::from_secs(10)).unwrap_or(false)
    }
}
