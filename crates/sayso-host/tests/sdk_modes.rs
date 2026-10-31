//! One test program in both modes of `Speech` (`docs/sdk.md`, step 12). Sayso is a real
//! `sayso_host::Host` in a child process of this test, so a test can kill it. The app is this
//! test process, with the real identity check. The fake engine runs by default, and
//! `real_engine_check` runs the same scenarios with the real engine:
//!
//! ```text
//! (cd native/macos/SaysoEngine && swift build -c release)
//! cargo test -p sayso-host --test sdk_modes -- --ignored --nocapture real_engine_check
//! ```
//!
//! Set `SAYSO_SDK_REAL_MODEL` to use another model than the smallest. Each test has its own
//! temporary folders and socket. No test uses the folders of the user. Only on Unix: the
//! tests kill processes, and they name a socket file.
#![cfg(unix)]

use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::Mutex;
use sayso_core::config::{Config, Provider, ProviderKind, RoleChoice};
use sayso_core::engine_catalog;
use sayso_core::enhance::EnhanceError;
use sayso_core::history::EnhanceOutcome;
use sayso_core::models::{self, ModelId, ModelInfo, ModelTask};
use sayso_core::pipeline::PipelineOutput;
use sayso_core::style::StyleLibrary;
use sayso_engine_client::{EngineClient, Options as EngineClientOptions};
use sayso_host::{ApprovalQuestion, DownloadQuestion, HostConfig, HostServices, StyleRequest};
use sayso_platform::SttBackend;
use sayso_sdk::protocol::{self, Capability, Enhance, ErrorCode, SessionStart, Source, TranscribeFile};
use sayso_sdk::capture::{AudioFeed, CapturedAudio, FeedItem, Microphone, OpenError, SourceEnd, SystemAudio};
use sayso_sdk::{ActiveMode, App, CaptureSources, Engine, Event, Features, Mode, ModelChoices, Options, Speech, Timing};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

mod common;
use common::{Setup, setup, this_program, write_app};

/// The most that a step of the fake engine can take. A slow machine does not reach it.
const LONG: Duration = Duration::from_secs(120);
/// For a check that says that something did not happen yet. A slow machine can only make
/// it pass falsely, never fail falsely.
const A_MOMENT: Duration = Duration::from_millis(150);
const GRANTS: [&str; 8] = ["models.read", "models.download", "transcribe.file", "session.live", "audio.mic", "audio.system", "ai.enhance", "dictation.observe"];

// --- Fake capture sources. No test does a real capture: a tap on macOS waits for a person. ---

#[derive(Default)]
struct SourceState {
    feeds: HashMap<String, Sender<FeedItem>>,
    /// The audio that each source has given, in ms.
    clock: HashMap<String, u64>,
    refuse: HashMap<String, OpenError>,
    opening: usize,
    block: bool,
    app_pid: Option<u32>,
}

/// The microphone and the system audio, driven by the words of [`FakeSources::command`].
#[derive(Clone)]
struct FakeSources {
    state: Arc<Mutex<SourceState>>,
    excludes: Vec<String>,
}

impl FakeSources {
    fn new(excludes: &[&str]) -> FakeSources {
        FakeSources { state: Arc::default(), excludes: excludes.iter().map(|name| name.to_string()).collect() }
    }

    fn open(&self, source: &str, excludes: Vec<String>) -> Result<AudioFeed, OpenError> {
        self.state.lock().opening += 1;
        // As a question of the system: it waits outside each lock.
        while self.state.lock().block {
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut state = self.state.lock();
        state.opening -= 1;
        if let Some(error) = state.refuse.get(source) {
            return Err(error.clone());
        }
        let (feed, items) = unbounded();
        state.feeds.insert(source.to_string(), feed);
        state.clock.insert(source.to_string(), 0);
        Ok(AudioFeed { items, lease: Box::new(FeedLease { state: self.state.clone(), source: source.to_string() }), excludes })
    }

    fn feed(&self, source: &str, item: FeedItem) {
        let state = self.state.lock();
        let _ = state.feeds.get(source).unwrap_or_else(|| panic!("the source {source} is not open")).send(item);
    }

    /// Frames of 20 ms on one clock for each source of a test, so two sources can overlap.
    fn audio(&self, source: &str, ms: u64, sample: impl Fn(u64) -> f32) {
        for _ in 0..ms / 20 {
            let at = {
                let mut state = self.state.lock();
                let clock = state.clock.entry(source.to_string()).or_default();
                let at = *clock;
                *clock += 20;
                at
            };
            let samples = (0..320).map(|n| sample(at * 16 + n)).collect();
            self.feed(source, FeedItem::Audio(CapturedAudio { samples, time: Some(Duration::from_secs(1000) + Duration::from_millis(at)) }));
        }
    }

    fn command(&self, words: &[&str]) -> String {
        match words {
            ["tone", source, ms] => self.audio(source, ms.parse().unwrap(), speech_sample),
            ["silence", source, ms] => self.audio(source, ms.parse().unwrap(), |_| 0.0),
            ["gap", source, ms] => self.feed(source, FeedItem::Gap { ms: ms.parse().unwrap() }),
            ["end", source, "permission"] => self.feed(source, FeedItem::Ended(SourceEnd::Permission)),
            ["end", source, "failed"] => self.feed(source, FeedItem::Ended(SourceEnd::Failed("the device broke".into()))),
            ["refuse", source] => {
                self.state.lock().refuse.insert(source.to_string(), OpenError::Permission("the system refused it".into()));
            }
            ["allow", source] => {
                self.state.lock().refuse.remove(*source);
            }
            ["block"] => self.state.lock().block = true,
            ["unblock"] => self.state.lock().block = false,
            ["opening"] => return self.state.lock().opening.to_string(),
            ["app_pid"] => return format!("{:?}", self.state.lock().app_pid),
            other => panic!("unknown source command {other:?}"),
        }
        "done".into()
    }

    fn capture(&self) -> CaptureSources {
        CaptureSources { microphone: Arc::new(self.clone()), system_audio: Arc::new(FakeSystem(self.clone())) }
    }
}

/// Something like speech for Basic VAD: a voice of 140 Hz with harmonics in syllables of 200 ms.
fn speech_sample(n: u64) -> f32 {
    let t = n as f32 / 16_000.0;
    let voice: f32 = (1..=5).map(|h| (2.0 * std::f32::consts::PI * 140.0 * h as f32 * t).sin() / h as f32).sum();
    let syllable = (t * 5.0).fract();
    let envelope = if syllable < 0.85 { (std::f32::consts::PI * syllable / 0.85).sin().max(0.3) } else { 0.05 };
    0.1 * voice * envelope
}

impl Microphone for FakeSources {
    fn open(&self) -> Result<AudioFeed, OpenError> {
        FakeSources::open(self, "mic", Vec::new())
    }
}

/// As with a real capture: dropping it ends the feed, so the channel closes.
struct FeedLease {
    state: Arc<Mutex<SourceState>>,
    source: String,
}

impl Drop for FeedLease {
    fn drop(&mut self) {
        self.state.lock().feeds.remove(&self.source);
    }
}

struct FakeSystem(FakeSources);

impl SystemAudio for FakeSystem {
    fn open(&self, app_pid: Option<u32>) -> Result<AudioFeed, OpenError> {
        self.0.state.lock().app_pid = app_pid;
        self.0.open("system", self.0.excludes.clone())
    }
}

// --- Sayso in a child process ---

struct ProcessServices {
    engine: Arc<dyn SttBackend>,
    config: Config,
    answers: Receiver<bool>,
    questions: Arc<AtomicUsize>,
    styles: PathBuf,
    sources: FakeSources,
}

impl ProcessServices {
    fn answer(&self, withdrawn: &dyn Fn() -> bool) -> Option<bool> {
        self.questions.fetch_add(1, Ordering::SeqCst);
        loop {
            match self.answers.recv_timeout(Duration::from_millis(10)) {
                Ok(answer) => return Some(answer),
                Err(_) if withdrawn() => return None,
                Err(_) => {}
            }
        }
    }
}

impl HostServices for ProcessServices {
    fn engine(&self) -> Option<Arc<dyn SttBackend>> {
        Some(self.engine.clone())
    }
    fn config(&self) -> Config {
        self.config.clone()
    }
    fn ask_download(&self, _: &DownloadQuestion, withdrawn: &dyn Fn() -> bool) -> bool {
        self.answer(withdrawn).unwrap_or(false)
    }
    fn ask_approval(&self, _: &ApprovalQuestion, withdrawn: &dyn Fn() -> bool) -> Option<bool> {
        self.answer(withdrawn)
    }
    fn microphone(&self) -> Option<Arc<dyn sayso_host::live::Microphone>> {
        Some(Arc::new(self.sources.clone()))
    }
    fn system_audio(&self) -> Option<Arc<dyn sayso_host::live::SystemAudio>> {
        Some(Arc::new(FakeSystem(self.sources.clone())))
    }
    fn styles(&self) -> Option<StyleLibrary> {
        Some(StyleLibrary::load(&self.styles))
    }
    fn run_style(&self, request: &StyleRequest<'_>) -> PipelineOutput {
        let error: Option<(String, EnhanceError)> = None;
        PipelineOutput { text: format!("[{}] {}", request.style.id, request.text), replacements: Vec::new(), enhance: EnhanceOutcome::Applied { provider_id: request.provider.id.clone(), model: "fake-model".into(), elapsed_ms: 1 }, enhance_error: error, timings: None, notice: None }
    }
}

/// Not a test: the process of Sayso that [`World::start_sayso`] starts. Commands come on stdin,
/// each with one answer on stdout, and the end of stdin ends the process.
#[test]
#[ignore = "the Sayso of the other tests; it does nothing without them"]
fn sayso_process() {
    let Some(engine) = std::env::var_os("SDK_TEST_ENGINE") else { return };
    let var = |key: &str| PathBuf::from(std::env::var_os(key).unwrap());
    let mut options = EngineClientOptions::default();
    if let Some(store) = std::env::var("SDK_TEST_STORE").ok().filter(|store| !store.is_empty()) {
        options.env.push(("SAYSO_MODEL_STORE".into(), store));
    }
    let client = EngineClient::spawn_with(PathBuf::from(engine), var("SDK_TEST_MODELS"), var("SDK_TEST_CACHE"), options).unwrap();
    let mut config = Config::default();
    config.dictation.model = ModelId::new(std::env::var("SDK_TEST_FINAL").unwrap());
    config.models.vad = RoleChoice::from(std::env::var("SDK_TEST_VAD").unwrap());
    config.ai.enabled = true;
    config.ai.providers = vec![Provider { id: "claude".into(), name: "Claude".into(), kind: ProviderKind::ClaudeCli { path: None, model: "haiku".into(), config_dir: None, thinking: None } }];
    config.ai.default_provider = Some("claude".into());
    config.ai.active_style = "polished".into();
    let (answer, answers) = unbounded();
    let questions = Arc::new(AtomicUsize::new(0));
    let sources = FakeSources::new(&["sayso", "app"]);
    let services = ProcessServices { engine: Arc::new(client), config, answers, questions: questions.clone(), styles: var("SDK_TEST_STYLES"), sources: sources.clone() };
    let host = sayso_host::Host::start(HostConfig::new(var("SDK_TEST_SOCKET"), var("SDK_TEST_APPS")), Arc::new(services)).unwrap();
    let say = |text: String| {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "\nSAYSO> {text}");
        let _ = out.flush();
    };
    say("ready".into());
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["source", rest @ ..] => {
                say(sources.command(rest));
                continue;
            }
            ["answer", yes] => answer.send(*yes == "yes").unwrap(),
            ["questions"] => {
                say(questions.load(Ordering::SeqCst).to_string());
                continue;
            }
            ["connected"] => {
                say(host.connected_apps().len().to_string());
                continue;
            }
            ["revoke", capability] => {
                let capability: Capability = serde_json::from_value(Value::String((*capability).to_string())).unwrap();
                host.apps().set_grant(&sayso_host::Identity::Path(this_program()).id(), &capability, false).unwrap();
                host.stop_work_without_grant();
            }
            ["dictation", "begin"] => host.dictation_gate().begin(),
            ["dictation", "end"] => host.dictation_gate().end(),
            ["close"] => {
                host.close();
                say("closed".into());
                std::process::exit(0);
            }
            other => panic!("unknown command {other:?}"),
        }
        say("done".into());
    }
    std::process::exit(0);
}

struct Sayso {
    child: Child,
    stdin: ChildStdin,
    answers: Receiver<String>,
}

impl Sayso {
    fn tell(&mut self, command: &str) -> String {
        writeln!(self.stdin, "{command}").unwrap();
        self.stdin.flush().unwrap();
        self.answers.recv_timeout(LONG).unwrap_or_else(|_| panic!("Sayso did not answer {command}"))
    }

    fn count(&mut self, command: &str) -> usize {
        self.tell(command).parse().unwrap()
    }

    fn wait_count(&mut self, command: &str, at_least: usize) {
        let start = Instant::now();
        while self.count(command) < at_least {
            assert!(start.elapsed() < LONG, "{command} did not reach {at_least}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// As a crash: no `host_closing`.
    fn kill(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}

impl Drop for Sayso {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// --- The world of a test ---

#[derive(Clone, Copy, PartialEq)]
enum Flavor {
    Fake,
    Real,
}

/// The engine that this system uses.
fn catalog_engine() -> engine_catalog::Engine {
    #[cfg(target_os = "macos")]
    return engine_catalog::Engine::Swift;
    #[cfg(target_os = "windows")]
    return engine_catalog::Engine::Rust(sayso_core::models_onnx::System::Windows);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return engine_catalog::Engine::Rust(sayso_core::models_onnx::System::Linux);
}

fn fake_engine() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        // <target>/<profile>/deps/<this test>
        let exe = std::env::current_exe().unwrap();
        let profile_dir = exe.parent().and_then(Path::parent).unwrap().to_path_buf();
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut build = Command::new(cargo);
        build.args(["build", "--quiet", "-p", "sayso-engine-client", "--bin", "sayso-fake-engine", "--manifest-path"]).arg(&manifest).arg("--target-dir").arg(profile_dir.parent().unwrap());
        if profile_dir.file_name().is_some_and(|name| name == "release") {
            build.arg("--release");
        }
        assert!(build.status().expect("cargo runs").success(), "the fake engine builds");
        profile_dir.join(format!("sayso-fake-engine{}", std::env::consts::EXE_SUFFIX))
    })
}

fn is_speech(model: &ModelInfo) -> bool {
    model.task() == ModelTask::Speech
}

struct World {
    flavor: Flavor,
    setup: Setup,
    engine: PathBuf,
    /// The folders of the two engines: Sayso ships one, and the app another.
    sayso_models: PathBuf,
    app_models: PathBuf,
    store: PathBuf,
    styles: PathBuf,
    final_model: String,
    other_final: String,
    /// A model that is not on disk.
    missing: String,
    vad: String,
    diarization: String,
    wav: PathBuf,
    pcm: Vec<i16>,
    timing: Timing,
    /// The capture of local mode. Sayso has its own, in its process.
    sources: FakeSources,
}

fn write_wav(path: &Path, samples: &[i16]) {
    let bytes = samples.len() as u32 * 2;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&32_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&bytes.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, wav).unwrap();
}

fn tone(ms: usize) -> Vec<i16> {
    (0..16 * ms).map(|n| (((n as f32) * 0.05).sin() * 8000.0) as i16).collect()
}

impl World {
    fn base(flavor: Flavor, engine: PathBuf) -> World {
        let setup = setup();
        let folder = |name: &str| {
            let path = setup.dir.path().join(name);
            std::fs::create_dir_all(&path).unwrap();
            path
        };
        let (sayso_models, app_models, store, styles) = (folder("sayso-models"), folder("app-models"), folder("store"), folder("styles"));
        folder("cache");
        write_app(&setup.apps, &this_program(), &GRANTS);
        let catalog = models::local_models();
        let speech: Vec<&ModelInfo> = catalog.iter().filter(|model| is_speech(model) && model.final_pass).collect();
        let streaming = speech.iter().find(|model| model.live_preview).copied().unwrap_or(speech[0]);
        let other = speech.iter().find(|model| model.id != streaming.id).copied().unwrap();
        let missing = speech.iter().find(|model| model.id != streaming.id && model.id != other.id).copied().unwrap();
        let of_task = |task: ModelTask| catalog.iter().find(|model| model.task() == task).unwrap().id.0.clone();
        let wav = setup.dir.path().join("talk.wav");
        World {
            flavor,
            engine,
            sayso_models,
            app_models,
            store,
            styles,
            final_model: streaming.id.0.clone(),
            other_final: other.id.0.clone(),
            missing: missing.id.0.clone(),
            vad: of_task(ModelTask::Vad),
            diarization: of_task(ModelTask::Diarization),
            wav,
            pcm: Vec::new(),
            timing: Timing { watch: Duration::from_millis(5), start_wait: Duration::from_secs(30), ..Timing::default() },
            sources: FakeSources::new(&["app"]),
            setup,
        }
    }

    fn fake() -> World {
        let mut world = World::base(Flavor::Fake, fake_engine().to_path_buf());
        let catalog = serde_json::to_string(&engine_catalog::json(catalog_engine())).unwrap();
        for dir in [&world.sayso_models, &world.app_models] {
            std::fs::write(dir.join("catalog.json"), &catalog).unwrap();
            std::fs::write(dir.join("more-features"), "catalog concurrent_streams").unwrap();
            for model in [&world.final_model, &world.other_final, &world.vad, &world.diarization] {
                std::fs::write(dir.join(format!("downloaded-{model}")), "").unwrap();
            }
        }
        write_wav(&world.wav, &tone(2000));
        world.pcm = tone(2000);
        world
    }

    fn speech_options(&self, mode: Mode) -> Options {
        let vad = if self.flavor == Flavor::Real { "basic".to_string() } else { self.vad.clone() };
        Options::new(mode, App::new("Notetaker"))
            .capabilities([Capability::ModelsRead, Capability::ModelsDownload, Capability::TranscribeFile, Capability::SessionLive, Capability::AudioMic, Capability::AudioSystem, Capability::AiEnhance, Capability::DictationObserve])
            .engine(Engine::from_program(&self.engine).store(&self.store).models_dir(&self.app_models))
            .socket(&self.setup.socket)
            .timing(self.timing)
            .capture(self.sources.capture())
            .models(ModelChoices { final_model: Some(self.final_model.clone()), vad_model: Some(vad), ..ModelChoices::default() })
    }

    fn speech(&self, mode: Mode) -> Speech {
        Speech::connect(self.speech_options(mode)).unwrap()
    }

    fn start_sayso(&self) -> Sayso {
        let vad = if self.flavor == Flavor::Real { "basic" } else { &self.vad };
        let dir = self.setup.dir.path();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "sayso_process", "--ignored", "--nocapture"])
            .env("SDK_TEST_ENGINE", &self.engine)
            .env("SDK_TEST_MODELS", &self.sayso_models)
            .env("SDK_TEST_CACHE", dir.join("cache"))
            .env("SDK_TEST_SOCKET", &self.setup.socket)
            .env("SDK_TEST_APPS", &self.setup.apps)
            .env("SDK_TEST_STYLES", &self.styles)
            .env("SDK_TEST_FINAL", &self.final_model)
            .env("SDK_TEST_VAD", vad)
            // This Sayso and its engine have no way to find the folders of the user.
            .env("HOME", dir)
            .env("XDG_DATA_HOME", dir.join("xdg-data"))
            .env("XDG_CONFIG_HOME", dir.join("xdg-config"))
            .env("XDG_CACHE_HOME", dir.join("xdg-cache"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // The fake engine has no store, as an older engine.
        if self.flavor == Flavor::Real {
            command.env("SDK_TEST_STORE", &self.store);
        }
        let mut child = command.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (answer, answers) = unbounded();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(text) = line.strip_prefix("SAYSO> ") {
                    let _ = answer.send(text.to_string());
                }
            }
        });
        let sayso = Sayso { child, stdin, answers };
        assert_eq!(sayso.answers.recv_timeout(LONG).expect("Sayso starts"), "ready");
        sayso
    }

    fn job(&self) -> TranscribeFile {
        TranscribeFile { path: self.wav.clone(), vad_model: (self.flavor == Flavor::Real).then(|| "basic".to_string()), ..Default::default() }
    }

    fn session(&self) -> SessionStart {
        SessionStart { sources: vec![Source::Client], vad_model: (self.flavor == Flavor::Real).then(|| "basic".to_string()), ..Default::default() }
    }

    fn folder(&self, mode: ActiveMode) -> &Path {
        match mode {
            ActiveMode::Sayso => &self.sayso_models,
            ActiveMode::Local => &self.app_models,
        }
    }

    /// How many requests of this kind the engine of this mode got.
    fn requests(&self, mode: ActiveMode, kind: &str) -> usize {
        let needle = format!("\"type\":\"{kind}\"");
        std::fs::read_to_string(self.folder(mode).join("requests.log")).unwrap_or_default().lines().filter(|line| line.contains(&needle)).count()
    }

    /// The fake engine waits in `transcribe` until [`Hold`] drops. Nothing with the real engine.
    fn hold(&self, mode: ActiveMode) -> Hold {
        let path = (self.flavor == Flavor::Fake).then(|| self.folder(mode).join("hold-transcribe"));
        if let Some(path) = &path {
            std::fs::write(path, "").unwrap();
        }
        Hold(path)
    }

    fn wait_requests(&self, mode: ActiveMode, kind: &str, at_least: usize) {
        if self.flavor == Flavor::Fake {
            eventually(&format!("the engine gets {kind}"), || self.requests(mode, kind) >= at_least);
        }
    }

    fn engine_pid(&self, mode: ActiveMode) -> u32 {
        std::fs::read_to_string(self.folder(mode).join("pid")).unwrap().trim().parse().unwrap()
    }
}

struct Hold(Option<PathBuf>);

impl Drop for Hold {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            std::fs::remove_file(path).unwrap();
        }
    }
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let start = Instant::now();
    while !check() {
        assert!(start.elapsed() < LONG, "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn process_runs(pid: u32) -> bool {
    Command::new("kill").arg("-0").arg(pid.to_string()).stderr(Stdio::null()).status().is_ok_and(|status| status.success())
}

// --- Events ---

fn wait_for(events: &Receiver<Event>, limit: Duration, done: impl Fn(&Event) -> bool) -> Vec<Event> {
    let mut seen = Vec::new();
    loop {
        let event = events.recv_timeout(limit).unwrap_or_else(|_| panic!("the event did not come; got {seen:?}"));
        let stop = done(&event);
        seen.push(event);
        if stop {
            return seen;
        }
    }
}

fn is_end_of(job: u64, event: &Event) -> bool {
    matches!(event, Event::Message(protocol::Event::JobDone { job: id, .. } | protocol::Event::JobFailed { job: id, .. }) if *id == job)
}

/// Events until the end of `job`, and then a check for a second end.
fn until_end(events: &Receiver<Event>, limit: Duration, job: u64) -> Vec<Event> {
    let mut seen = wait_for(events, limit, |event| is_end_of(job, event));
    std::thread::sleep(A_MOMENT);
    let later: Vec<Event> = events.try_iter().collect();
    assert!(!later.iter().any(|event| is_end_of(job, event)), "one end for each job: {later:?}");
    seen.extend(later);
    seen
}

fn modes_in(events: &[Event]) -> Vec<ActiveMode> {
    events.iter().filter_map(|event| if let Event::ModeChanged { mode, .. } = event { Some(*mode) } else { None }).collect()
}

/// What a scenario did, without the ids, which come from the SDK in order.
#[derive(Debug, PartialEq, Default)]
struct Trace {
    /// The messages with the ids of work as 0.
    messages: Vec<Value>,
    /// The codes of the errors of requests.
    errors: Vec<Option<ErrorCode>>,
    text: Vec<String>,
}

fn normalized(message: &protocol::Event) -> Value {
    let mut value = serde_json::to_value(message).unwrap();
    for field in ["job", "session"] {
        if value.get(field).is_some_and(|id| !id.is_null()) {
            value[field] = 0.into();
        }
    }
    value
}

fn kind(value: &Value) -> &str {
    value["type"].as_str().unwrap_or_default()
}

impl Trace {
    fn add(&mut self, events: &[Event]) {
        for event in events {
            if let Event::Message(message) = event {
                self.messages.push(normalized(message));
            }
        }
    }

    /// The kinds of the messages, without `partial` (its number depends on the time) and
    /// without repeats.
    fn kinds(&self) -> Vec<String> {
        let mut kinds: Vec<String> = self.messages.iter().map(|message| kind(message).to_string()).filter(|kind| kind != "partial").collect();
        kinds.dedup();
        kinds
    }
}

// --- The scenarios, the same in both modes and with both engines ---

struct Run<'a> {
    world: &'a World,
    mode: ActiveMode,
    speech: Speech,
    events: Receiver<Event>,
    sayso: Option<Sayso>,
    limit: Duration,
}

impl World {
    fn run(&self, mode: ActiveMode) -> Run<'_> {
        let sayso = (mode == ActiveMode::Sayso).then(|| self.start_sayso());
        let asked = if mode == ActiveMode::Sayso { Mode::Sayso { start: false } } else { Mode::Local };
        let speech = self.speech(asked);
        let events = speech.events();
        let limit = if self.flavor == Flavor::Real { Duration::from_secs(600) } else { LONG };
        Run { world: self, mode, speech, events, sayso, limit }
    }
}

fn file_job(run: &mut Run<'_>) -> Trace {
    let job = run.speech.transcribe_file(run.world.job()).unwrap();
    let mut trace = Trace::default();
    let seen = until_end(&run.events, run.limit, job);
    assert!(matches!(seen.last().filter(|_| run.world.flavor == Flavor::Fake).or(seen.iter().rfind(|event| is_end_of(job, event))), Some(Event::Message(protocol::Event::JobDone { .. }))), "{seen:?}");
    trace.add(&seen);
    trace
}

fn live_session(run: &mut Run<'_>) -> Trace {
    let session = run.speech.start_session(run.world.session()).unwrap().session;
    for chunk in run.world.pcm.chunks(16 * 320) {
        run.speech.session_audio(session, chunk).unwrap();
        // A real engine gives a partial while the audio comes, at 4 times real time here.
        if run.world.flavor == Flavor::Real {
            std::thread::sleep(Duration::from_millis(80));
        }
    }
    if run.world.flavor == Flavor::Fake {
        wait_for(&run.events, run.limit, |event| matches!(event, Event::Message(protocol::Event::Partial(_))));
    }
    let text = run.speech.stop_session(session).unwrap();
    assert!(!text.is_empty(), "the session gave text");
    std::thread::sleep(A_MOMENT);
    let mut trace = Trace { text: vec!["text".into()], ..Trace::default() };
    let events: Vec<Event> = run.events.try_iter().collect();
    assert!(!events.iter().any(|event| matches!(event, Event::Message(protocol::Event::SessionEnded { .. }))), "the reply of the stop is the end: {events:?}");
    trace.add(&events);
    assert_eq!(run.speech.stop_session(session).unwrap_err().code, Some(ErrorCode::BadRequest), "the session ended");
    println!("{:?} session text: {text}", run.mode);
    trace
}

fn refused_download(run: &mut Run<'_>) -> Trace {
    let error = run.speech.download_model("no-such-model", "a test").unwrap_err();
    let mut trace = Trace { errors: vec![error.code], ..Trace::default() };
    std::thread::sleep(A_MOMENT);
    trace.add(&run.events.try_iter().collect::<Vec<_>>());
    trace
}

fn queued_jobs(run: &mut Run<'_>) -> Trace {
    let hold = run.world.hold(run.mode);
    let before = run.world.requests(run.mode, "transcribe");
    let first = run.speech.transcribe_file(run.world.job()).unwrap();
    run.world.wait_requests(run.mode, "transcribe", before + 1);
    let second = run.speech.transcribe_file(run.world.job()).unwrap();
    assert_ne!(first, second);
    if run.world.flavor == Flavor::Fake {
        let waiting = wait_for(&run.events, run.limit, |event| matches!(event, Event::Message(protocol::Event::JobProgress { job, .. }) if *job == second));
        assert!(!waiting.iter().any(|event| is_end_of(first, event) || is_end_of(second, event)), "the second job waits: {waiting:?}");
        std::thread::sleep(A_MOMENT);
        let more: Vec<Event> = run.events.try_iter().collect();
        assert!(!more.iter().any(|event| is_end_of(second, event)), "the second job waits: {more:?}");
    }
    drop(hold);
    let seen = wait_for(&run.events, run.limit, |event| is_end_of(second, event));
    let at = |job: u64| seen.iter().position(|event| is_end_of(job, event));
    assert!(at(first).is_none_or(|first| first < at(second).unwrap()), "first in, first out: {seen:?}");
    // Each kind of message the same. The ids come from the SDK, in order.
    let mut trace = Trace::default();
    trace.add(&seen);
    trace
}

/// Features that one mode does not have: the same error type, and `features()` tells first.
fn missing_features(run: &mut Run<'_>) {
    let features = run.speech.features();
    let enhance = run.speech.enhance(Enhance { text: Some("words".into()), ..Default::default() });
    let subscribe = run.speech.subscribe(&["dictation_done"]);
    assert!(features.system_audio, "both modes have it");
    match run.mode {
        ActiveMode::Sayso => {
            assert!(features.enhance && features.subscribe && features.download_question, "{features:?}");
            assert!(features.capabilities.contains(&Capability::AiEnhance), "{features:?}");
            assert_eq!(enhance.unwrap().text, "[polished] words");
            assert_eq!(subscribe.unwrap(), vec!["dictation_done".to_string()]);
        }
        ActiveMode::Local => {
            assert!(!features.enhance && !features.subscribe && !features.download_question, "{features:?}");
            assert_eq!(enhance.unwrap_err().code, Some(ErrorCode::Unsupported));
            assert_eq!(subscribe.unwrap_err().code, Some(ErrorCode::Unsupported));
        }
    }
}

/// A grant that the user takes back. Local mode has no grants, so the work has its normal end.
fn grant_taken_back(run: &mut Run<'_>) {
    let session = run.speech.start_session(run.world.session()).unwrap().session;
    run.speech.session_audio(session, &run.world.pcm[..run.world.pcm.len().min(16 * 320)]).unwrap();
    match run.sayso.as_mut() {
        Some(sayso) => {
            sayso.tell("revoke session.live");
            let seen = wait_for(&run.events, run.limit, |event| matches!(event, Event::Message(protocol::Event::SessionEnded { session: id, .. }) if *id == session));
            assert_eq!(seen.iter().filter(|event| matches!(event, Event::Message(protocol::Event::SessionEnded { .. }))).count(), 1);
            assert_eq!(run.speech.stop_session(session).unwrap_err().code, Some(ErrorCode::BadRequest), "the session ended");
            let error = run.speech.start_session(run.world.session()).unwrap_err();
            assert_eq!((error.code, error.capability), (Some(ErrorCode::Capability), Some(Capability::SessionLive)));
        }
        None => {
            run.speech.stop_session(session).unwrap();
            assert_eq!(run.speech.stop_session(session).unwrap_err().code, Some(ErrorCode::BadRequest));
        }
    }
    std::thread::sleep(A_MOMENT);
    let later: Vec<Event> = run.events.try_iter().collect();
    assert!(!later.iter().any(|event| matches!(event, Event::Message(protocol::Event::SessionEnded { .. }))), "one end: {later:?}");
    if run.world.flavor == Flavor::Real {
        return;
    }
    // A job that runs, with the fake engine, which can hold it.
    let hold = run.world.hold(run.mode);
    let before = run.world.requests(run.mode, "transcribe");
    let job = run.speech.transcribe_file(run.world.job()).unwrap();
    run.world.wait_requests(run.mode, "transcribe", before + 1);
    let revoked = run.sayso.as_mut().map(|sayso| sayso.tell("revoke transcribe.file")).is_some();
    if !revoked {
        drop(hold);
        assert!(matches!(until_end(&run.events, run.limit, job).iter().rfind(|event| is_end_of(job, event)), Some(Event::Message(protocol::Event::JobDone { .. }))));
        return;
    }
    let seen = until_end(&run.events, run.limit, job);
    assert!(matches!(seen.iter().rfind(|event| is_end_of(job, event)), Some(Event::Message(protocol::Event::JobFailed { code: Some(ErrorCode::Capability), .. }))), "{seen:?}");
    drop(hold);
    std::thread::sleep(A_MOMENT);
    let later: Vec<Event> = run.events.try_iter().collect();
    assert!(!later.iter().any(|event| is_end_of(job, event)), "one end, also after the engine finished: {later:?}");
    assert_eq!(run.speech.transcribe_file(run.world.job()).unwrap_err().code, Some(ErrorCode::Capability));
}

/// The scenario in Sayso mode and in local mode.
fn in_both_modes<T>(world: &World, scenario: impl Fn(&mut Run<'_>) -> T) -> (T, T) {
    let in_sayso = scenario(&mut world.run(ActiveMode::Sayso));
    let in_local = scenario(&mut world.run(ActiveMode::Local));
    (in_sayso, in_local)
}

#[test]
fn a_file_job_gives_the_same_messages_in_both_modes() {
    let (sayso, local) = in_both_modes(&World::fake(), file_job);
    assert_eq!(sayso, local);
    assert!(sayso.kinds().contains(&"segment".to_string()), "{sayso:?}");
}

#[test]
fn a_live_session_with_the_source_client_ends_the_same_in_both_modes() {
    let (sayso, local) = in_both_modes(&World::fake(), live_session);
    assert_eq!(sayso.kinds(), local.kinds());
    assert_eq!(sayso.text, local.text);
}

#[test]
fn a_refused_download_gives_the_same_error_in_both_modes() {
    let (sayso, local) = in_both_modes(&World::fake(), refused_download);
    assert_eq!(sayso, local);
    assert_eq!(sayso.errors, vec![Some(ErrorCode::UnknownModel)]);
}

#[test]
fn a_job_that_waits_in_the_queue_gives_the_same_messages_in_both_modes() {
    let (sayso, local) = in_both_modes(&World::fake(), queued_jobs);
    assert_eq!(sayso, local);
}

#[test]
fn a_feature_that_one_mode_does_not_have_gives_unsupported() {
    in_both_modes(&World::fake(), missing_features);
}

#[test]
fn a_grant_that_the_user_takes_back_ends_the_work_once() {
    in_both_modes(&World::fake(), grant_taken_back);
}

/// A table of requests. Both modes give the same codes, so local mode has the checks of Sayso.
#[test]
fn both_modes_refuse_the_same_requests() {
    let world = World::fake();
    let (speech_model, vad) = (world.final_model.clone(), world.vad.clone());
    let path = world.wav.clone();
    let jobs: Vec<TranscribeFile> = vec![
        TranscribeFile { path: "relative.wav".into(), ..Default::default() },
        TranscribeFile { path: path.clone(), speakers: Some(0), ..Default::default() },
        TranscribeFile { path: path.clone(), model: Some("no-such-model".into()), ..Default::default() },
        TranscribeFile { path: path.clone(), model: Some(vad.clone()), ..Default::default() },
        TranscribeFile { path: path.clone(), model: Some(world.missing.clone()), ..Default::default() },
        TranscribeFile { path: path.clone(), vad_model: Some("no-such-model".into()), ..Default::default() },
        TranscribeFile { path: path.clone(), vad_model: Some(speech_model.clone()), ..Default::default() },
        TranscribeFile { path: path.clone(), diarization_model: Some(vad.clone()), ..Default::default() },
        TranscribeFile { path: path.clone(), diarization_model: Some("no-such-model".into()), ..Default::default() },
    ];
    let sessions: Vec<SessionStart> = vec![
        SessionStart { sources: vec![], ..Default::default() },
        SessionStart { sources: vec![Source::Client], model: Some("no-such-model".into()), ..Default::default() },
        SessionStart { sources: vec![Source::Client], model: Some(vad.clone()), ..Default::default() },
        SessionStart { sources: vec![Source::Client], preview_model: Some(vad.clone()), ..Default::default() },
        SessionStart { sources: vec![Source::Client], preview_model: Some(world.missing.clone()), ..Default::default() },
        SessionStart { sources: vec![Source::Client], vad_model: Some(speech_model.clone()), ..Default::default() },
        SessionStart { sources: vec![Source::Client], diarization_model: Some(speech_model), ..Default::default() },
    ];
    let codes = |run: &mut Run<'_>| {
        let mut codes = Vec::new();
        for job in &jobs {
            codes.push(run.speech.transcribe_file(job.clone()).map(|_| ()).map_err(|e| e.code));
        }
        for session in &sessions {
            codes.push(run.speech.start_session(session.clone()).map(|_| ()).map_err(|e| e.code));
        }
        codes
    };
    let (sayso, local) = in_both_modes(&world, codes);
    assert_eq!(sayso, local);
    assert!(sayso.iter().all(Result::is_err), "{sayso:?}");
}

/// The same with a job that names models, with diarization.
#[test]
fn diarization_gives_the_same_messages_in_both_modes() {
    let world = World::fake();
    let scenario = |run: &mut Run<'_>| {
        let job = run.speech.transcribe_file(TranscribeFile { path: run.world.wav.clone(), model: Some(run.world.other_final.clone()), vad_model: Some("basic".into()), diarize: Some(true), speakers: Some(2), ..Default::default() }).unwrap();
        let mut trace = Trace::default();
        trace.add(&until_end(&run.events, run.limit, job));
        trace
    };
    let (sayso, local) = in_both_modes(&world, scenario);
    assert_eq!(sayso, local);
    assert!(sayso.kinds().contains(&"speakers".to_string()), "{sayso:?}");
}

// --- The source `system`, with a fake capture ---

impl Run<'_> {
    /// Tell the fake capture of this mode what to do.
    fn source(&mut self, command: &str) -> String {
        match self.sayso.as_mut() {
            Some(sayso) => sayso.tell(&format!("source {command}")),
            None => self.world.sources.command(&command.split_whitespace().collect::<Vec<_>>()),
        }
    }

    fn session_with(&self, sources: &[Source]) -> SessionStart {
        SessionStart { sources: sources.to_vec(), vad_model: Some("basic".into()), ..Default::default() }
    }
}

fn is_source_event(event: &Event) -> bool {
    matches!(event, Event::Message(protocol::Event::SourceGap { .. } | protocol::Event::SourceEnded { .. }))
}

/// The segments of each source, without their times.
fn texts_of(events: &[Event], source: &Source) -> Vec<String> {
    events.iter().filter_map(|event| match event {
        Event::Message(protocol::Event::Segment(segment)) if segment.source.as_ref() == Some(source) => Some(segment.segment.text.clone()),
        _ => None,
    }).collect()
}

fn system_messages(events: &[Event]) -> Vec<Value> {
    events.iter().filter(|event| is_source_event(event)).map(|event| if let Event::Message(message) = event { normalized(message) } else { Value::Null }).collect()
}

#[test]
fn a_gap_of_the_source_system_reaches_the_app_in_both_modes() {
    let world = World::fake();
    let scenario = |run: &mut Run<'_>| {
        let started = run.speech.start_session(run.session_with(&[Source::System])).unwrap();
        run.source("tone system 400");
        run.source("gap system 240");
        let seen = wait_for(&run.events, LONG, is_source_event);
        assert!(seen.contains(&Event::Message(protocol::Event::SourceGap { session: started.session, source: Source::System, ms: 240 })), "{seen:?}");
        // A gap is not an end: the session goes on, and ends once.
        run.source("tone system 400");
        run.source("silence system 1000");
        run.speech.stop_session(started.session).unwrap();
        std::thread::sleep(A_MOMENT);
        let later: Vec<Event> = run.events.try_iter().collect();
        assert!(!later.iter().any(|event| matches!(event, Event::Message(protocol::Event::SessionEnded { .. }))), "{later:?}");
        system_messages(&seen)
    };
    let (sayso, local) = in_both_modes(&world, scenario);
    assert_eq!(sayso, local);
}

#[test]
fn a_source_that_ends_with_permission_leaves_the_mic_running_in_both_modes() {
    let world = World::fake();
    let scenario = |run: &mut Run<'_>| {
        let started = run.speech.start_session(run.session_with(&[Source::Mic, Source::System])).unwrap();
        run.source("end system permission");
        let seen = wait_for(&run.events, LONG, is_source_event);
        assert!(seen.contains(&Event::Message(protocol::Event::SourceEnded { session: started.session, source: Source::System, reason: protocol::SourceEndReason::Permission })), "{seen:?}");
        assert!(!seen.iter().any(|event| matches!(event, Event::Message(protocol::Event::SessionEnded { .. }))), "the session goes on: {seen:?}");

        run.source("tone mic 1000");
        run.source("silence mic 1500");
        let segment = wait_for(&run.events, LONG, |event| matches!(event, Event::Message(protocol::Event::Segment(segment)) if segment.source == Some(Source::Mic)));
        assert!(!texts_of(&segment, &Source::Mic).is_empty(), "the mic goes on: {segment:?}");
        let text = run.speech.stop_session(started.session).unwrap();
        assert!(!text.is_empty());
        std::thread::sleep(A_MOMENT);
        let later: Vec<Event> = run.events.try_iter().collect();
        assert!(!later.iter().any(|event| is_source_event(event) || matches!(event, Event::Message(protocol::Event::SessionEnded { .. }))), "one end for the source, one for the session: {later:?}");
        assert_eq!(run.speech.stop_session(started.session).unwrap_err().code, Some(ErrorCode::BadRequest));
        system_messages(&seen)
    };
    let (sayso, local) = in_both_modes(&world, scenario);
    assert_eq!(sayso, local);
}

#[test]
fn a_capture_that_fails_ends_the_source_with_the_reason_failed_in_both_modes() {
    let world = World::fake();
    let scenario = |run: &mut Run<'_>| {
        let started = run.speech.start_session(run.session_with(&[Source::System])).unwrap();
        run.source("end system failed");
        let seen = wait_for(&run.events, LONG, is_source_event);
        run.speech.stop_session(started.session).unwrap();
        system_messages(&seen)
    };
    let (sayso, local) = in_both_modes(&world, scenario);
    assert_eq!(sayso, local);
    assert_eq!(sayso[0]["reason"], "failed");
}

#[test]
fn a_refused_permission_at_the_start_gives_the_code_permission_in_both_modes() {
    let world = World::fake();
    let scenario = |run: &mut Run<'_>| {
        run.source("refuse system");
        let error = run.speech.start_session(run.session_with(&[Source::System])).unwrap_err();
        assert_eq!(error.code, Some(ErrorCode::Permission), "{error:?}");
        let also_with_a_mic = run.speech.start_session(run.session_with(&[Source::Mic, Source::System])).unwrap_err();
        assert_eq!(also_with_a_mic.code, Some(ErrorCode::Permission));
        // The refusal ended the request. No event and no open work stay, and the next start works.
        std::thread::sleep(A_MOMENT);
        let events: Vec<Event> = run.events.try_iter().collect();
        assert!(!events.iter().any(|event| matches!(event, Event::Message(protocol::Event::SessionEnded { .. }))), "{events:?}");
        run.source("allow system");
        let started = run.speech.start_session(run.session_with(&[Source::System])).unwrap();
        run.source("silence system 200");
        run.speech.stop_session(started.session).unwrap();
        error.code
    };
    let (sayso, local) = in_both_modes(&world, scenario);
    assert_eq!(sayso, local);
}

#[test]
fn excludes_come_in_the_ok_of_session_start() {
    let world = World::fake();
    {
        let mut run = world.run(ActiveMode::Sayso);
        let started = run.speech.start_session(run.session_with(&[Source::System])).unwrap();
        assert_eq!(started.excludes, ["sayso", "app"]);
        assert_eq!(run.source("app_pid"), format!("Some({})", std::process::id()), "Sayso leaves out the process of the app");
        run.speech.stop_session(started.session).unwrap();
        let client = run.speech.start_session(run.session_with(&[Source::Client])).unwrap();
        assert!(client.excludes.is_empty(), "no source system");
        run.speech.stop_session(client.session).unwrap();
    }
    let mut run = world.run(ActiveMode::Local);
    let started = run.speech.start_session(run.session_with(&[Source::System])).unwrap();
    assert_eq!(started.excludes, ["app"], "the app and the capture are one process");
    assert_eq!(run.source("app_pid"), "None");
    assert_ne!(started.session, 0);
    run.speech.stop_session(started.session).unwrap();
    let client = run.speech.start_session(run.session_with(&[Source::Client])).unwrap();
    assert!(client.excludes.is_empty());
}

/// Without headphones the microphone hears the call. The same audio on both sources at one time
/// gives one text, so the microphone segments go. A session with only `mic` proves that they
/// are there without the rule.
#[test]
fn the_echo_rule_drops_the_mic_segments_in_both_modes() {
    let world = World::fake();
    // A short segment in a long text is a real reply by the rule, so both sources get a long one.
    for folder in [&world.sayso_models, &world.app_models] {
        std::fs::write(folder.join("one-segment"), "").unwrap();
    }
    let scenario = |run: &mut Run<'_>| {
        let mic_only = run.speech.start_session(run.session_with(&[Source::Mic])).unwrap();
        run.source("tone mic 1000");
        run.source("silence mic 1500");
        run.speech.stop_session(mic_only.session).unwrap();
        std::thread::sleep(A_MOMENT);
        let without_rule: Vec<Event> = run.events.try_iter().collect();
        assert!(!texts_of(&without_rule, &Source::Mic).is_empty(), "{without_rule:?}");

        let both = run.speech.start_session(run.session_with(&[Source::Mic, Source::System])).unwrap();
        for source in ["system", "mic"] {
            run.source(&format!("tone {source} 1000"));
            run.source(&format!("silence {source} 1500"));
        }
        run.speech.stop_session(both.session).unwrap();
        std::thread::sleep(A_MOMENT);
        let events: Vec<Event> = run.events.try_iter().collect();
        assert!(!texts_of(&events, &Source::System).is_empty(), "{events:?}");
        assert!(texts_of(&events, &Source::Mic).is_empty(), "the echo goes: {events:?}");
        (texts_of(&events, &Source::System), texts_of(&events, &Source::Mic))
    };
    let (sayso, local) = in_both_modes(&world, scenario);
    assert_eq!(sayso, local);
}

/// A question of macOS can take minutes: the lock stays free, the app can work, and the
/// question is active work.
#[test]
fn a_capture_that_waits_for_a_person_does_not_hold_the_lock_and_stops_a_change_of_mode() {
    let world = World::fake();
    let speech = Arc::new(world.speech(Mode::Auto));
    let events = speech.events();
    world.sources.command(&["block"]);
    let starting = {
        let (speech, request) = (speech.clone(), SessionStart { sources: vec![Source::System], vad_model: Some("basic".into()), ..Default::default() });
        std::thread::spawn(move || speech.start_session(request))
    };
    eventually("the capture waits", || world.sources.command(&["opening"]) == "1");
    assert_eq!(speech.mode(), ActiveMode::Local);
    assert!(speech.features().system_audio);
    speech.list_models(None).unwrap();

    let mut sayso = world.start_sayso();
    sayso.wait_count("connected", 1);
    std::thread::sleep(A_MOMENT);
    assert_eq!(speech.mode(), ActiveMode::Local, "the request has no reply yet");
    assert!(modes_in(&events.try_iter().collect::<Vec<_>>()).is_empty());

    world.sources.command(&["unblock"]);
    let started = starting.join().unwrap().unwrap();
    assert_eq!(started.excludes, ["app"]);
    std::thread::sleep(A_MOMENT);
    assert_eq!(speech.mode(), ActiveMode::Local, "the session runs");
    speech.stop_session(started.session).unwrap();
    wait_change(&events, ActiveMode::Sayso);
}

/// A question that takes longer than the limit of `hello`: the event of the host keeps the SDK waiting.
#[test]
fn the_sdk_waits_for_a_person_longer_than_the_limit_of_hello() {
    let mut world = World::fake();
    // The host has this long to send its event, also on a machine with much to do.
    let limit = Duration::from_secs(5);
    world.timing.hello_limit = limit;
    std::fs::write(&world.setup.apps, "").unwrap();
    let mut sayso = world.start_sayso();
    let speech = {
        let options = world.speech_options(Mode::Sayso { start: false });
        std::thread::spawn(move || Speech::connect(options))
    };
    sayso.wait_count("questions", 1);
    std::thread::sleep(limit + Duration::from_secs(1));
    assert!(!speech.is_finished(), "the SDK waits for the person");
    sayso.tell("answer yes");
    assert_eq!(speech.join().unwrap().unwrap().mode(), ActiveMode::Sayso);
}

// --- What differs between modes ---

#[test]
fn downloads_differ_sayso_asks_and_local_mode_starts_at_once() {
    let world = World::fake();
    let model = world.missing.clone();
    {
        let mut run = world.run(ActiveMode::Sayso);
        let sayso = run.sayso.as_mut().unwrap();
        sayso.tell("answer no");
        assert_eq!(run.speech.download_model(&model, "a test").unwrap_err().code, Some(ErrorCode::Declined));
        let sayso = run.sayso.as_mut().unwrap();
        assert_eq!(sayso.count("questions"), 1);
        assert_eq!(world.requests(ActiveMode::Sayso, "download"), 0);
        assert!(run.speech.features().download_question);

        sayso.tell("answer yes");
        run.speech.download_model(&model, "a test").unwrap();
        assert_eq!(world.requests(ActiveMode::Sayso, "download"), 1);
    }
    let run = world.run(ActiveMode::Local);
    assert!(!run.speech.features().download_question);
    run.speech.download_model(&model, "a test").unwrap();
    assert_eq!(world.requests(ActiveMode::Local, "download"), 1, "no question");
    // The same events as in Sayso mode, before the reply.
    let events: Vec<Event> = run.events.try_iter().collect();
    assert!(events.iter().any(|event| matches!(event, Event::Message(protocol::Event::DownloadProgress(progress)) if progress.model == model)), "{events:?}");
    assert!(events.iter().any(|event| matches!(event, Event::Message(protocol::Event::ModelState(state)) if state.model == model && state.state == protocol::ModelState::Downloaded)), "{events:?}");
    // On disk: done at once, with no second download.
    run.speech.download_model(&model, "a test").unwrap();
    assert_eq!(world.requests(ActiveMode::Local, "download"), 1);
}

#[test]
fn a_download_that_fails_in_local_mode_gives_the_failed_state_and_an_error() {
    let world = World::fake();
    std::fs::write(world.app_models.join("fail-download"), "network").unwrap();
    let run = world.run(ActiveMode::Local);
    let error = run.speech.download_model(&world.missing, "a test").unwrap_err();
    assert_eq!(error.message, "download stopped");
    let events: Vec<Event> = run.events.try_iter().collect();
    assert!(events.iter().any(|event| matches!(event, Event::Message(protocol::Event::ModelState(state)) if state.state == protocol::ModelState::Failed)), "{events:?}");
}

#[test]
fn models_differ_sayso_has_the_choice_of_the_user_and_local_mode_the_choice_of_the_app() {
    let world = World::fake();
    let text_of = |speech: &Speech, events: &Receiver<Event>| {
        let job = speech.transcribe_file(world.job()).unwrap();
        let seen = until_end(events, LONG, job);
        seen.iter().find_map(|event| if let Event::Message(protocol::Event::JobDone { text, .. }) = event { Some(text.clone()) } else { None }).unwrap()
    };
    let requested = |mode| std::fs::read_to_string(world.folder(mode).join("requests.log")).unwrap();
    let transcribed = |mode, model: &str| requested(mode).lines().any(|line| line.contains("\"type\":\"transcribe\"") && line.contains(&format!("\"model\":\"{model}\"")));

    // The app names another model than the user. Sayso uses that of the user.
    let mut options = world.speech_options(Mode::Sayso { start: false });
    options.models.final_model = Some(world.other_final.clone());
    let _sayso = world.start_sayso();
    let speech = Speech::connect(options.clone()).unwrap();
    text_of(&speech, &speech.events());
    assert!(transcribed(ActiveMode::Sayso, &world.final_model), "the choice of the user");
    assert!(!transcribed(ActiveMode::Sayso, &world.other_final));
    drop(speech);

    options.mode = Mode::Local;
    let speech = Speech::connect(options.clone()).unwrap();
    text_of(&speech, &speech.events());
    assert!(transcribed(ActiveMode::Local, &world.other_final), "the choice of the app");
    assert!(!transcribed(ActiveMode::Local, &world.final_model));
    let listed = speech.list_models(Some(protocol::Role::Final)).unwrap();
    assert!(listed.iter().find(|model| model.id == world.other_final).unwrap().choice.contains(&protocol::Role::Final), "{listed:?}");
    drop(speech);

    // No choice of the app: the first suitable model on disk.
    options.models = ModelChoices::default();
    let speech = Speech::connect(options).unwrap();
    let on_disk = |id: &ModelId| world.app_models.join(format!("downloaded-{id}")).exists();
    let first = models::local_models().iter().find(|model| is_speech(model) && model.final_pass && on_disk(&model.id)).unwrap().id.0.clone();
    text_of(&speech, &speech.events());
    assert!(transcribed(ActiveMode::Local, &first), "the first final model on disk: {first}");
}

#[test]
fn queues_differ_sayso_is_for_all_apps_and_local_mode_is_for_this_app() {
    let world = World::fake();
    let options = world.speech_options(Mode::Sayso { start: false });
    {
        let mut run = world.run(ActiveMode::Sayso);
        let other_app = Speech::connect(options.clone()).unwrap();
        let hold = world.hold(ActiveMode::Sayso);
        let first = other_app.transcribe_file(world.job()).unwrap();
        world.wait_requests(ActiveMode::Sayso, "transcribe", 1);
        let second = run.speech.transcribe_file(world.job()).unwrap();
        std::thread::sleep(A_MOMENT);
        assert!(!run.events.try_iter().any(|event| is_end_of(second, &event)), "it waits for the other app");

        let session = other_app.start_session(world.session()).unwrap().session;
        let error = run.speech.start_session(world.session()).unwrap_err();
        assert_eq!(error.code, Some(ErrorCode::Busy), "one live session for all apps");
        other_app.stop_session(session).ok();

        drop(hold);
        until_end(&other_app.events(), LONG, first);
        until_end(&run.events, LONG, second);
        run.sayso.as_mut().unwrap();
    }

    // Two local apps have two engines. The second app has its own folder, and a hold in the first.
    let run = world.run(ActiveMode::Local);
    let other_folder = world.setup.dir.path().join("other-app-models");
    std::fs::create_dir_all(&other_folder).unwrap();
    for entry in std::fs::read_dir(&world.app_models).unwrap().flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("downloaded-") || name == "catalog.json" || name == "more-features" {
            std::fs::copy(entry.path(), other_folder.join(name)).unwrap();
        }
    }
    let other_app = Speech::connect(world.speech_options(Mode::Local).engine(Engine::from_program(&world.engine).store(&world.store).models_dir(&other_folder))).unwrap();
    let hold = world.hold(ActiveMode::Local);
    let before = world.requests(ActiveMode::Local, "transcribe");
    let held = run.speech.transcribe_file(world.job()).unwrap();
    world.wait_requests(ActiveMode::Local, "transcribe", before + 1);
    let free = other_app.transcribe_file(world.job()).unwrap();
    until_end(&other_app.events(), LONG, free);
    let session = other_app.start_session(world.session()).unwrap().session;
    let mine = run.speech.start_session(world.session()).unwrap().session;
    other_app.stop_session(session).unwrap();
    run.speech.stop_session(mine).unwrap();
    assert!(!run.events.try_iter().any(|event| is_end_of(held, &event)), "the hold is still on");
    drop(hold);
    until_end(&run.events, LONG, held);
}

#[test]
fn priority_differs_sayso_has_dictation_and_both_put_live_sessions_before_jobs() {
    let world = World::fake();
    // Sayso: a job waits while a dictation runs.
    {
        let mut run = world.run(ActiveMode::Sayso);
        run.sayso.as_mut().unwrap().tell("dictation begin");
        let job = run.speech.transcribe_file(world.job()).unwrap();
        std::thread::sleep(A_MOMENT);
        assert_eq!(world.requests(ActiveMode::Sayso, "transcribe"), 0, "the job waits for the dictation");
        run.sayso.as_mut().unwrap().tell("dictation end");
        until_end(&run.events, LONG, job);
    }
    // Both modes: a job waits while a live session transcribes a part.
    for mode in [ActiveMode::Sayso, ActiveMode::Local] {
        let run = world.run(mode);
        let hold = world.hold(mode);
        let before = world.requests(mode, "transcribe");
        let session = run.speech.start_session(world.session()).unwrap().session;
        for chunk in world.pcm.chunks(16 * 320) {
            run.speech.session_audio(session, chunk).unwrap();
        }
        // A part of the session is at the engine, and waits.
        world.wait_requests(mode, "transcribe", before + 1);
        let at_part = world.requests(mode, "transcribe");
        let job = run.speech.transcribe_file(TranscribeFile { path: world.wav.clone(), vad_model: Some("basic".into()), ..Default::default() }).unwrap();
        std::thread::sleep(A_MOMENT);
        let job_ended = run.events.try_iter().any(|event| is_end_of(job, &event));
        assert!(!job_ended, "{mode:?}: the job waits");
        // Only the parts of the session reach the engine. The job would ask for a text with a path of the job.
        let log = std::fs::read_to_string(world.folder(mode).join("requests.log")).unwrap();
        assert_eq!(log.lines().filter(|line| line.contains("\"type\":\"transcribe\"")).count(), at_part, "{mode:?}: no engine call of the job");
        drop(hold);
        run.speech.stop_session(session).unwrap();
        until_end(&run.events, LONG, job);
    }
}

#[test]
fn features_differ_the_engine_of_the_app_can_have_less_than_the_engine_of_sayso() {
    let world = World::fake();
    std::fs::write(world.app_models.join("more-features"), "catalog").unwrap();
    std::fs::write(world.app_models.join("no-timing"), "").unwrap();
    let job = TranscribeFile { path: world.wav.clone(), diarization_model: Some(world.diarization.clone()), ..Default::default() };
    {
        let run = world.run(ActiveMode::Sayso);
        assert_eq!(run.speech.features().engine, None, "Sayso mode has the host features");
        assert!(!run.speech.features().host.is_empty() || run.speech.features().enhance);
        let id = run.speech.transcribe_file(job.clone()).unwrap();
        until_end(&run.events, LONG, id);
    }
    let run = world.run(ActiveMode::Local);
    assert_eq!(run.speech.features().engine, None, "the engine has not started");
    assert_eq!(run.speech.transcribe_file(job).unwrap_err().code, Some(ErrorCode::Unsupported));
    let engine = run.speech.features().engine.unwrap();
    assert!(engine.contains(&"catalog".to_string()) && !engine.contains(&"diarization".to_string()), "{engine:?}");
}

// --- Mode::Auto ---

fn wait_change(events: &Receiver<Event>, mode: ActiveMode) -> (Vec<Event>, Features) {
    let seen = wait_for(events, LONG, |event| matches!(event, Event::ModeChanged { mode: new, .. } if *new == mode));
    let features = match seen.last() {
        Some(Event::ModeChanged { features, .. }) => features.clone(),
        _ => unreachable!(),
    };
    (seen, features)
}

#[test]
fn auto_moves_to_sayso_when_sayso_starts_and_to_local_when_it_quits() {
    let world = World::fake();
    let speech = world.speech(Mode::Auto);
    let events = speech.events();
    assert_eq!(speech.mode(), ActiveMode::Local);
    assert!(!speech.features().enhance);

    let mut sayso = world.start_sayso();
    let (_, features) = wait_change(&events, ActiveMode::Sayso);
    assert!(features.enhance && features.subscribe);
    assert_eq!(speech.mode(), ActiveMode::Sayso);
    speech.subscribe(&["dictation_done"]).unwrap();

    sayso.tell("close");
    let (_, features) = wait_change(&events, ActiveMode::Local);
    assert!(!features.enhance);
    assert_eq!(speech.mode(), ActiveMode::Local);
    assert_eq!(speech.subscribe(&["dictation_done"]).unwrap_err().code, Some(ErrorCode::Unsupported));
    speech.list_models(None).unwrap();
    assert!(world.requests(ActiveMode::Local, "list_models") > 0, "the engine of the app starts at the first request");

    // And back, on a new socket.
    let _again = world.start_sayso();
    wait_change(&events, ActiveMode::Sayso);
}

#[test]
fn auto_does_not_move_during_a_job_and_stops_its_engine_after_the_move() {
    let world = World::fake();
    let speech = world.speech(Mode::Auto);
    let events = speech.events();
    let hold = world.hold(ActiveMode::Local);
    let job = speech.transcribe_file(world.job()).unwrap();
    world.wait_requests(ActiveMode::Local, "transcribe", 1);
    let pid = world.engine_pid(ActiveMode::Local);

    let mut sayso = world.start_sayso();
    sayso.wait_count("connected", 1);
    std::thread::sleep(A_MOMENT);
    assert_eq!(speech.mode(), ActiveMode::Local, "hello finished, and a job runs");
    assert!(modes_in(&events.try_iter().collect::<Vec<_>>()).is_empty());

    drop(hold);
    let seen = wait_for(&events, LONG, |event| matches!(event, Event::ModeChanged { .. }));
    assert!(seen.iter().any(|event| matches!(event, Event::Message(protocol::Event::JobDone { job: id, .. }) if *id == job)), "the job ends in local mode: {seen:?}");
    assert_eq!(world.requests(ActiveMode::Sayso, "transcribe"), 0, "a change never moves work");
    eventually("the engine of the app stops, so the model is in memory once", || !process_runs(pid));

    let next = speech.transcribe_file(world.job()).unwrap();
    until_end(&events, LONG, next);
    assert!(world.requests(ActiveMode::Sayso, "transcribe") > 0, "the next job goes to Sayso");
    assert_ne!(job, next);
}

#[test]
fn auto_does_not_move_during_a_live_session() {
    let world = World::fake();
    let speech = world.speech(Mode::Auto);
    let events = speech.events();
    let session = speech.start_session(world.session()).unwrap().session;
    speech.session_audio(session, &world.pcm[..16 * 320]).unwrap();
    let mut sayso = world.start_sayso();
    sayso.wait_count("connected", 1);
    std::thread::sleep(A_MOMENT);
    assert_eq!(speech.mode(), ActiveMode::Local);
    speech.session_audio(session, &world.pcm[16 * 320..16 * 640]).unwrap();
    speech.stop_session(session).unwrap();
    wait_change(&events, ActiveMode::Sayso);
}

#[test]
fn auto_does_not_move_before_the_answer_to_the_approval_question() {
    let world = World::fake();
    std::fs::write(&world.setup.apps, "").unwrap();
    let speech = world.speech(Mode::Auto);
    let events = speech.events();
    let mut sayso = world.start_sayso();
    sayso.wait_count("questions", 1);
    // The question shows. Work runs in local mode meanwhile.
    let job = speech.transcribe_file(world.job()).unwrap();
    until_end(&events, LONG, job);
    assert_eq!(speech.mode(), ActiveMode::Local);
    assert!(world.requests(ActiveMode::Local, "transcribe") > 0);

    sayso.tell("answer yes");
    wait_change(&events, ActiveMode::Sayso);
}

#[test]
fn auto_does_not_move_during_a_download() {
    let world = World::fake();
    let speech = Arc::new(world.speech(Mode::Auto));
    let events = speech.events();
    let hold = world.app_models.join("hold-download");
    std::fs::write(&hold, "").unwrap();
    let downloading = {
        let (speech, model) = (speech.clone(), world.missing.clone());
        std::thread::spawn(move || speech.download_model(&model, "a test"))
    };
    world.wait_requests(ActiveMode::Local, "download", 1);
    let mut sayso = world.start_sayso();
    sayso.wait_count("connected", 1);
    std::thread::sleep(A_MOMENT);
    assert_eq!(speech.mode(), ActiveMode::Local, "a request has no reply yet");
    assert!(modes_in(&events.try_iter().collect::<Vec<_>>()).is_empty());
    std::fs::remove_file(&hold).unwrap();
    downloading.join().unwrap().unwrap();
    wait_change(&events, ActiveMode::Sayso);
}

#[test]
fn a_refused_app_stays_local_and_does_not_ask_again() {
    let world = World::fake();
    std::fs::write(&world.setup.apps, "").unwrap();
    let speech = world.speech(Mode::Auto);
    let events = speech.events();
    let mut sayso = world.start_sayso();
    sayso.wait_count("questions", 1);
    sayso.tell("answer no");
    let job = speech.transcribe_file(world.job()).unwrap();
    until_end(&events, LONG, job);
    std::thread::sleep(A_MOMENT);
    assert_eq!(speech.mode(), ActiveMode::Local);
    assert_eq!(sayso.count("questions"), 1);
    assert!(modes_in(&events.try_iter().collect::<Vec<_>>()).is_empty());
}

#[test]
fn killing_sayso_during_a_job_gives_connection_lost_and_no_request_runs_twice() {
    let world = World::fake();
    let mut sayso = world.start_sayso();
    let speech = world.speech(Mode::Auto);
    let events = speech.events();
    assert_eq!(speech.mode(), ActiveMode::Sayso);
    let hold = world.hold(ActiveMode::Sayso);
    let job = speech.transcribe_file(world.job()).unwrap();
    let session = speech.start_session(world.session()).unwrap().session;
    world.wait_requests(ActiveMode::Sayso, "transcribe", 1);
    let sent = world.requests(ActiveMode::Sayso, "transcribe");
    sayso.kill();

    let (seen, _) = wait_change(&events, ActiveMode::Local);
    assert!(matches!(seen.iter().filter(|event| is_end_of(job, event)).collect::<Vec<_>>()[..], [Event::Message(protocol::Event::JobFailed { code: Some(ErrorCode::ConnectionLost), .. })]), "{seen:?}");
    assert!(seen.contains(&Event::Message(protocol::Event::SessionEnded { session, text: String::new(), code: Some(ErrorCode::ConnectionLost) })), "{seen:?}");
    drop(hold);

    // The first request after the change runs in local mode, and nothing went out again.
    assert_eq!(world.requests(ActiveMode::Local, "transcribe"), 0);
    speech.list_models(None).unwrap();
    assert!(world.requests(ActiveMode::Local, "list_models") > 0);
    std::thread::sleep(A_MOMENT);
    assert_eq!(world.requests(ActiveMode::Sayso, "transcribe"), sent, "no request runs twice");
    assert_eq!(world.requests(ActiveMode::Local, "transcribe"), 0, "no request runs twice");
    assert!(!events.try_iter().any(|event| is_end_of(job, &event)), "one end");

    let next = speech.transcribe_file(world.job()).unwrap();
    assert_ne!(next, job);
    until_end(&events, LONG, next);
    assert!(world.requests(ActiveMode::Local, "transcribe") > 0);
}

#[test]
fn sayso_mode_after_a_kill_stays_gone_until_reconnect() {
    let world = World::fake();
    let mut sayso = world.start_sayso();
    let speech = world.speech(Mode::Sayso { start: false });
    let events = speech.events();
    sayso.kill();
    wait_for(&events, LONG, |event| matches!(event, Event::Disconnected { code: ErrorCode::ConnectionLost }));
    assert_eq!(speech.list_models(None).unwrap_err().code, Some(ErrorCode::HostNotRunning));
    assert_eq!(speech.reconnect().unwrap_err().code, Some(ErrorCode::HostNotRunning));
    let _again = world.start_sayso();
    speech.reconnect().unwrap();
    assert!(!speech.list_models(None).unwrap().is_empty());
    assert!(world.requests(ActiveMode::Local, "list_models") == 0, "Mode::Sayso never uses the engine of the app");
}

// --- Local mode ---

#[test]
fn the_engine_gets_the_store_only_when_it_lists_store_v1() {
    let world = World::fake();
    let first = world.speech(Mode::Local);
    first.list_models(None).unwrap();
    // The fake engine makes the empty file at each start, and writes the store when it has one.
    assert_eq!(std::fs::read_to_string(world.app_models.join("store.log")).unwrap(), "", "an older engine uses its own folder");
    drop(first);
    std::fs::write(world.app_models.join("more-features"), "catalog concurrent_streams store_v1").unwrap();
    let second = world.speech(Mode::Local);
    second.list_models(None).unwrap();
    let log = std::fs::read_to_string(world.app_models.join("store.log")).unwrap();
    assert_eq!(log.trim(), world.store.display().to_string());
}

#[test]
fn an_engine_that_fails_hello_or_has_no_catalog_gives_unsupported() {
    let world = World::fake();
    std::fs::write(world.app_models.join("exit-at-hello"), "").unwrap();
    let speech = world.speech(Mode::Local);
    assert_eq!(speech.list_models(None).unwrap_err().code, Some(ErrorCode::Unsupported));
    std::fs::remove_file(world.app_models.join("exit-at-hello")).unwrap();
    std::fs::write(world.app_models.join("more-features"), "segments").unwrap();
    assert_eq!(speech.list_models(None).unwrap_err().code, Some(ErrorCode::Unsupported), "no catalog");
}

#[test]
fn local_mode_starts_the_engine_at_the_first_request_and_stops_it_with_the_speech() {
    let world = World::fake();
    let speech = world.speech(Mode::Local);
    assert!(!world.app_models.join("pid").exists(), "no request yet");
    speech.list_models(None).unwrap();
    let pid = world.engine_pid(ActiveMode::Local);
    assert!(process_runs(pid));
    drop(speech);
    eventually("the engine stops", || !process_runs(pid));
}

#[test]
fn an_engine_that_crashed_starts_again_at_the_next_request() {
    let world = World::fake();
    let speech = world.speech(Mode::Local);
    speech.list_models(None).unwrap();
    let pid = world.engine_pid(ActiveMode::Local);
    assert!(Command::new("kill").arg("-9").arg(pid.to_string()).status().unwrap().success());
    eventually("the next request starts a new engine", || speech.list_models(None).is_ok());
    assert_ne!(world.engine_pid(ActiveMode::Local), pid);
}

#[test]
fn local_mode_unloads_a_model_after_the_keep_time_and_not_while_work_uses_it() {
    let mut world = World::fake();
    world.timing.keep_model = Duration::from_millis(30);
    let speech = world.speech(Mode::Local);
    let events = speech.events();
    let hold = world.hold(ActiveMode::Local);
    let job = speech.transcribe_file(world.job()).unwrap();
    world.wait_requests(ActiveMode::Local, "transcribe", 1);
    std::thread::sleep(A_MOMENT);
    assert_eq!(world.requests(ActiveMode::Local, "unload"), 0, "work uses it");
    drop(hold);
    until_end(&events, LONG, job);
    eventually("the model goes out of memory", || world.requests(ActiveMode::Local, "unload") > 0);
}

#[test]
fn a_cancelled_job_ends_once_in_both_modes() {
    let world = World::fake();
    let scenario = |run: &mut Run<'_>| {
        let hold = run.world.hold(run.mode);
        let before = run.world.requests(run.mode, "transcribe");
        let job = run.speech.transcribe_file(run.world.job()).unwrap();
        run.world.wait_requests(run.mode, "transcribe", before + 1);
        run.speech.cancel_job(job).unwrap();
        let seen = until_end(&run.events, run.limit, job);
        drop(hold);
        std::thread::sleep(A_MOMENT);
        assert!(!run.events.try_iter().any(|event| is_end_of(job, &event)), "one end");
        run.speech.cancel_job(job).unwrap();
        seen.iter().filter(|event| is_end_of(job, event)).map(|event| if let Event::Message(message) = event { normalized(message) } else { Value::Null }).collect::<Vec<_>>()
    };
    let (sayso, local) = in_both_modes(&world, scenario);
    assert_eq!(sayso, local);
}

// --- The real engine ---

/// Run this by hand: see the top of this file. It downloads the smallest final model
/// into a temporary store, once, and both modes use it.
#[test]
#[ignore = "needs the real engine and the network"]
fn real_engine_check() {
    let engine = EngineClient::default_engine_path();
    assert!(engine.exists(), "build the engine first: {}", engine.display());
    let mut world = World::base(Flavor::Real, engine);
    let wav = world.setup.dir.path().join("say.wav");
    let said = Command::new("say").args(["-o"]).arg(&wav).args(["--data-format=LEI16@16000", "Hello from the Sayso SDK. This is a test of both modes. The quick brown fox jumps over the lazy dog."]).status().unwrap();
    assert!(said.success(), "say makes the audio");
    world.wav = wav;
    let bytes = std::fs::read(&world.wav).unwrap();
    let data = bytes.windows(4).position(|window| window == b"data").unwrap() + 8;
    world.pcm = bytes[data..].as_chunks::<2>().0.iter().map(|pair| i16::from_le_bytes(*pair)).chain(std::iter::repeat_n(0, 16_000)).collect();

    // The smallest final model with files in the store.
    let local = Speech::connect(world.speech_options(Mode::Local).models(ModelChoices { vad_model: Some("basic".into()), ..Default::default() })).unwrap();
    let listed = local.list_models(Some(protocol::Role::Final)).unwrap();
    let asked = std::env::var("SAYSO_SDK_REAL_MODEL").ok();
    let smallest = listed.iter().filter(|model| model.size_bytes > 0 && asked.as_ref().is_none_or(|id| *id == model.id)).min_by_key(|model| model.size_bytes).unwrap();
    println!("final models: {:?}", listed.iter().map(|model| (&model.id, model.size_bytes)).collect::<Vec<_>>());
    println!("engine features: {:?}", local.features().engine);
    println!("download {} ({} bytes)", smallest.id, smallest.size_bytes);
    let started = Instant::now();
    local.download_model(&smallest.id, "a test").unwrap();
    println!("download took {} s", started.elapsed().as_secs());
    world.final_model = smallest.id.clone();
    drop(local);
    assert!(std::fs::read_dir(&world.store).unwrap().next().is_some(), "the download went to the store of this test");

    let (sayso, local) = in_both_modes(&world, file_job);
    println!("file job, Sayso: {:?}", sayso.messages.last());
    println!("file job, local: {:?}", local.messages.last());
    assert_eq!(sayso.kinds(), local.kinds());
    assert_eq!(sayso.messages.last().map(kind), Some("job_done"));
    assert_eq!(sayso.messages.last(), local.messages.last(), "one model and one file give one text");

    let (sayso, local) = in_both_modes(&world, live_session);
    let partials = |trace: &Trace| trace.messages.iter().filter(|message| kind(message) == "partial").count();
    println!("live, Sayso: {:?} and {} partials, local: {:?} and {} partials", sayso.kinds(), partials(&sayso), local.kinds(), partials(&local));
    assert_eq!(sayso.kinds(), local.kinds());

    let (sayso, local) = in_both_modes(&world, refused_download);
    assert_eq!(sayso, local);
    {
        // The user says no in Sayso mode, and nothing reaches the engine.
        let mut run = world.run(ActiveMode::Sayso);
        run.sayso.as_mut().unwrap().tell("answer no");
        let other = listed.iter().find(|model| model.id != smallest.id && model.size_bytes > 0).unwrap();
        assert_eq!(run.speech.download_model(&other.id, "a test").unwrap_err().code, Some(ErrorCode::Declined));
    }

    let (sayso, local) = in_both_modes(&world, queued_jobs);
    println!("queue, Sayso: {:?}, local: {:?}", sayso.kinds(), local.kinds());
    assert_eq!(sayso.kinds(), local.kinds());

    in_both_modes(&world, missing_features);
    in_both_modes(&world, grant_taken_back);
    println!("the real engine gives the same messages and ends in both modes");
}
