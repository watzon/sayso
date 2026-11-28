//! Helpers for the tests that run the Go test programs: `sayso-testhost` (the real Host API of
//! Sayso on WAV files), `sayso-fake-engine`, and `sayso-fake-cleanup`.
//!
//! Each program comes from an environment variable (`SAYSO_TESTHOST`, `SAYSO_FAKE_ENGINE`,
//! `SAYSO_FAKE_CLEANUP`), else `go build` makes it in a folder of the system temporary folder.
//! A test that needs one and cannot get it skips with a note, unless `SAYSO_TEST_REQUIRE_FAKES=1`
//! (set in CI): then it fails.

#![allow(dead_code)]

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../protocol/testdata")
}

pub fn require() -> bool {
    std::env::var("SAYSO_TEST_REQUIRE_FAKES").is_ok_and(|v| v == "1")
}

/// None, with a note, when `what` cannot be had and the run does not require it.
pub fn missing(what: &str) -> Option<PathBuf> {
    assert!(
        !require(),
        "SAYSO_TEST_REQUIRE_FAKES=1 and {what} is not available"
    );
    eprintln!("skipped: {what} is not available");
    None
}

#[derive(Clone, Copy)]
enum Program {
    TestHost,
    FakeEngine,
    FakeCleanup,
}

impl Program {
    fn env(self) -> &'static str {
        match self {
            Program::TestHost => "SAYSO_TESTHOST",
            Program::FakeEngine => "SAYSO_FAKE_ENGINE",
            Program::FakeCleanup => "SAYSO_FAKE_CLEANUP",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Program::TestHost => "sayso-testhost",
            Program::FakeEngine => "sayso-fake-engine",
            Program::FakeCleanup => "sayso-fake-cleanup",
        }
    }
    /// The folder where `go build` runs, and the package.
    fn source(self) -> (PathBuf, &'static str) {
        match self {
            Program::TestHost => (repo_root(), "./cmd/sayso-testhost"),
            Program::FakeEngine => (repo_root().join("sdk/go"), "./cmd/sayso-fake-engine"),
            Program::FakeCleanup => (repo_root().join("sdk/go"), "./cmd/sayso-fake-cleanup"),
        }
    }
}

fn build(program: Program) -> Option<PathBuf> {
    static LOCK: Mutex<()> = Mutex::new(());
    static BUILT: OnceLock<Mutex<Vec<(&'static str, PathBuf)>>> = OnceLock::new();
    if let Some(path) = std::env::var_os(program.env()).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let _one = LOCK.lock().unwrap();
    let built = BUILT.get_or_init(|| Mutex::new(Vec::new()));
    if let Some((_, path)) = built
        .lock()
        .unwrap()
        .iter()
        .find(|(name, _)| *name == program.name())
    {
        return Some(path.clone());
    }
    let (folder, package) = program.source();
    if !folder.join("go.mod").is_file() {
        return None;
    }
    let out_dir = std::env::temp_dir().join(format!(
        "sayso-sdk-rust-bins-{}",
        std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&out_dir).ok()?;
    let name = if cfg!(windows) {
        format!("{}-{}.exe", program.name(), std::process::id())
    } else {
        format!("{}-{}", program.name(), std::process::id())
    };
    let temp = out_dir.join(name);
    let status = Command::new(std::env::var_os("GO").unwrap_or_else(|| "go".into()))
        .args(["build", "-o"])
        .arg(&temp)
        .arg(package)
        .current_dir(&folder)
        .env("CGO_ENABLED", "0")
        .status();
    match status {
        Ok(status) if status.success() => {}
        other => {
            eprintln!("go build {package} failed: {other:?}");
            return None;
        }
    }
    built.lock().unwrap().push((program.name(), temp.clone()));
    Some(temp)
}

fn get(program: Program) -> Option<PathBuf> {
    build(program).or_else(|| missing(program.name()))
}

pub fn testhost() -> Option<PathBuf> {
    get(Program::TestHost)
}

pub fn fake_engine() -> Option<PathBuf> {
    get(Program::FakeEngine)
}

pub fn fake_cleanup() -> Option<PathBuf> {
    get(Program::FakeCleanup)
}

/// Write the id of a scenario that passed to `SAYSO_SCENARIO_REPORT`, when it is set.
pub fn passed(id: &str) {
    if let Some(report) = std::env::var_os("SAYSO_SCENARIO_REPORT") {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(report)
            .unwrap();
        // One write per line: parallel tests append to the same file.
        file.write_all(format!("{id}\n").as_bytes()).unwrap();
    }
}

static COUNT: AtomicU64 = AtomicU64::new(0);

/// A socket path for a test. On Unix the folder is short (a socket path has a small limit)
/// and 0700, as the host wants. On Windows it is a pipe name.
pub struct SocketPath {
    pub path: PathBuf,
    _folder: Option<tempfile::TempDir>,
}

impl SocketPath {
    pub fn new() -> SocketPath {
        let n = COUNT.fetch_add(1, Ordering::SeqCst);
        if cfg!(windows) {
            return SocketPath {
                path: PathBuf::from(format!(
                    r"\\.\pipe\sayso-sdk-rust-{}-{n}",
                    std::process::id()
                )),
                _folder: None,
            };
        }
        let folder = tempfile::Builder::new()
            .prefix("srt")
            .tempdir_in("/tmp")
            .unwrap();
        SocketPath {
            path: folder.path().join("s").join("host.sock"),
            _folder: Some(folder),
        }
    }
}

/// A running `sayso-testhost`.
pub struct TestHost {
    pub socket: PathBuf,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<Value>,
    pub data: tempfile::TempDir,
    seen: Vec<Value>,
}

#[derive(Default)]
pub struct HostOptions {
    pub args: Vec<String>,
    pub script: Option<String>,
    pub with_engine: bool,
}

impl TestHost {
    /// Start on `socket` with the fake engine, which runs any model id.
    pub fn start(socket: &Path, args: &[&str]) -> Option<TestHost> {
        Self::start_with(
            socket,
            HostOptions {
                args: args.iter().map(|a| a.to_string()).collect(),
                script: Some(r#"{"accept_any":true}"#.into()),
                with_engine: true,
            },
        )
    }

    pub fn start_with(socket: &Path, options: HostOptions) -> Option<TestHost> {
        let program = testhost()?;
        let engine = if options.with_engine {
            Some(fake_engine()?)
        } else {
            None
        };
        let data = tempfile::tempdir().unwrap();
        let mut command = Command::new(program);
        command
            .arg("--socket")
            .arg(socket)
            .arg("--data")
            .arg(data.path());
        if let Some(engine) = engine {
            command.arg("--engine").arg(engine);
        }
        if let Some(script) = &options.script {
            command
                .arg("--engine-env")
                .arg(format!("SAYSO_FAKE_SCRIPT={script}"));
        }
        command.args(&options.args);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = command.spawn().expect("cannot start sayso-testhost");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(value) = serde_json::from_str::<Value>(&line)
                    && tx.send(value).is_err()
                {
                    break;
                }
            }
        });
        let mut host = TestHost {
            socket: socket.to_path_buf(),
            child,
            stdin,
            lines,
            data,
            seen: Vec::new(),
        };
        host.await_key("ready", None);
        Some(host)
    }

    /// The next line of the host with `key` (and the string `value` for it, when given).
    pub fn await_key(&mut self, key: &str, value: Option<&str>) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    let found = line
                        .get(key)
                        .is_some_and(|v| value.is_none_or(|want| v.as_str() == Some(want)));
                    self.seen.push(line.clone());
                    if found {
                        return line;
                    }
                }
                Err(RecvTimeoutError::Timeout) => panic!(
                    "the test host printed no {key} {value:?}; seen {:?}",
                    self.seen
                ),
                Err(RecvTimeoutError::Disconnected) => panic!(
                    "the test host ended while the test waited for {key}; seen {:?}",
                    self.seen
                ),
            }
        }
    }

    pub fn send(&mut self, command: Value) {
        let stdin = self.stdin.as_mut().expect("the host stdin is open");
        writeln!(stdin, "{command}").unwrap();
        stdin.flush().unwrap();
    }

    /// Quit like Sayso: `host_closing`, then exit.
    pub fn close(&mut self) {
        if self.stdin.is_some() {
            self.send(serde_json::json!({"close": true}));
        }
        self.wait_exit();
    }

    /// Exit at once, without `host_closing`.
    pub fn kill(&mut self) {
        self.send(serde_json::json!({"kill": true}));
        self.wait_exit();
    }

    fn wait_exit(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(Some(_)) = self.child.try_wait() {
                break;
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        self.stdin = None;
    }
}

impl Drop for TestHost {
    fn drop(&mut self) {
        if self.stdin.is_some() {
            self.close();
        }
    }
}

/// 16-bit mono PCM of a tone from `from_ms` to `to_ms` inside `total_ms` of silence, at `rate`.
pub fn tone(rate: u32, total_ms: u32, from_ms: u32, to_ms: u32) -> Vec<i16> {
    (0..rate * total_ms / 1000)
        .map(|i| {
            let t = i as f32 / rate as f32;
            if t >= from_ms as f32 / 1000.0 && t < to_ms as f32 / 1000.0 {
                ((2.0 * std::f32::consts::PI * 440.0 * t).sin() * 16000.0) as i16
            } else {
                0
            }
        })
        .collect()
}

pub fn wav_bytes(rate: u32, samples: &[i16]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// A 16 kHz WAV file with a tone in it, in `folder`.
pub fn write_wav(folder: &Path, name: &str, total_ms: u32, from_ms: u32, to_ms: u32) -> PathBuf {
    let path = folder.join(name);
    std::fs::write(
        &path,
        wav_bytes(16000, &tone(16000, total_ms, from_ms, to_ms)),
    )
    .unwrap();
    path
}

/// The fake engine started with an empty models folder. Keep the folder as long as the engine runs.
pub fn start_empty() -> Option<(sayso_sdk::engine::Engine, tempfile::TempDir)> {
    let program = fake_engine()?;
    let models = tempfile::tempdir().unwrap();
    let engine = sayso_sdk::engine::Engine::start(sayso_sdk::engine::EngineOptions::new(
        &program,
        models.path(),
    ))
    .unwrap_or_else(|e| panic!("cannot start {}: {e}", program.display()));
    Some((engine, models))
}

/// The engine program for the tests with real models: `SAYSO_ENGINE`.
pub fn engine_program() -> PathBuf {
    PathBuf::from(std::env::var_os("SAYSO_ENGINE").expect("set SAYSO_ENGINE to the engine program"))
}

/// 16 kHz mono audio: 500 ms of silence, then a tone for each length in ms, each with 1 s of
/// silence after it. The fake engine hears a word for each 300 ms of tone.
pub fn speech(lengths: &[u32]) -> Vec<i16> {
    let mut samples = vec![0i16; 8000];
    for ms in lengths {
        for i in 0..(*ms as usize * 16) {
            samples.push(
                (9000.0 * (2.0 * std::f64::consts::PI * 220.0 * i as f64 / 16000.0).sin()) as i16,
            );
        }
        samples.extend(std::iter::repeat_n(0i16, 16000));
    }
    samples
}

/// `speech(lengths)` in a WAV file of `folder`.
pub fn speech_wav(folder: &Path, name: &str, lengths: &[u32]) -> PathBuf {
    let path = folder.join(name);
    std::fs::write(&path, wav_bytes(16000, &speech(lengths))).unwrap();
    path
}

/// Wait for the first event of `speech` that `pick` takes. The events seen on the way go to `seen`.
pub fn until<T>(
    speech: &sayso_sdk::Speech,
    seen: &mut Vec<sayso_sdk::Event>,
    mut pick: impl FnMut(&sayso_sdk::Event) -> Option<T>,
) -> T {
    let events = speech.events();
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(event) => {
                let found = pick(&event);
                seen.push(event);
                if let Some(found) = found {
                    return found;
                }
            }
            Err(e) => panic!("the event did not come ({e}); seen {seen:#?}"),
        }
    }
}

/// The type name of an event of the host, for messages.
pub fn kind(event: &sayso_sdk::Event) -> &'static str {
    match event {
        sayso_sdk::Event::Message(message) => sayso_sdk::protocol::Kind::kind(message),
        sayso_sdk::Event::ModeChanged { .. } => "mode_changed",
        sayso_sdk::Event::Error(_) => "error",
        sayso_sdk::Event::Disconnected { .. } => "disconnected",
    }
}

pub const ALL_CAPABILITIES: [sayso_sdk::protocol::Capability; 7] = {
    use sayso_sdk::protocol::Capability::*;
    [
        ModelsRead,
        ModelsDownload,
        TranscribeFile,
        SessionLive,
        AudioMic,
        AiEnhance,
        DictationObserve,
    ]
};

/// `Mode::Sayso` on `socket` with every capability but the system audio.
pub fn sayso_options(socket: &Path) -> sayso_sdk::Options {
    sayso_sdk::Options::new(
        sayso_sdk::Mode::Sayso { start: false },
        sayso_sdk::App::new("Notes"),
    )
    .capabilities(ALL_CAPABILITIES)
    .socket(socket)
}

/// Download the final model of the user and give its id.
pub fn ready(speech: &sayso_sdk::Speech) -> String {
    let models = speech
        .list_models(Some(sayso_sdk::protocol::Role::Final))
        .unwrap();
    let model = models
        .iter()
        .find(|m| m.choice.contains(&sayso_sdk::protocol::Role::Final))
        .unwrap_or_else(|| panic!("no final model of the user in {models:#?}"));
    speech.download_model(&model.id, "tests").unwrap();
    model.id.clone()
}
