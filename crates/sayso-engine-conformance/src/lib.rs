//! Conformance tests for the engine protocol (`docs/engine-protocol.md`).
//!
//! The suite starts the engine as a child process, sends requests on stdin,
//! and checks the lines on stdout against the spec. Each check starts its own
//! engine, so one failure does not hide the others. It has three levels:
//!
//! - **core** needs no model. Every engine passes it, `sayso-fake-engine` too.
//! - **model** needs a catalog entry that is not on disk ([`Model`]). It checks
//!   the replies for a model that is not downloaded or not loaded. The fake
//!   engine does not keep that state, so only real engines run this level.
//! - **ready** needs a models folder with the model on disk, and a 16 kHz mono
//!   WAV file ([`Ready`]). It loads the model, transcribes, streams, and
//!   unloads. With the features `segments` and `concurrent_streams`, it also
//!   checks the segments of a transcription and three streams at the same
//!   time. It does not download or delete.
//!
//! Every line of every engine also gets these checks: it is a JSON object
//! with `"v":1` and a string `type`, and no request gets two replies.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// A model of the catalog: its id and the `engine` object of its entry.
#[derive(Debug, Clone)]
pub struct Model {
    pub id: String,
    pub engine: Value,
}

/// A model that is on disk, for the ready level.
#[derive(Debug, Clone)]
pub struct Ready {
    /// The models folder (`SAYSO_MODELS_DIR`) that holds the model.
    pub models_dir: PathBuf,
    pub model: Model,
    /// A 16 kHz mono 16-bit WAV file with speech.
    pub audio: PathBuf,
    /// The model has a live preview, so the suite also streams the audio.
    pub stream: bool,
}

/// What to test.
#[derive(Debug, Clone)]
pub struct Suite {
    /// The engine executable.
    pub engine: PathBuf,
    /// Extra environment for the engine.
    pub env: Vec<(String, String)>,
    pub model: Option<Model>,
    pub ready: Option<Ready>,
    /// The longest wait for a quick reply.
    pub timeout: Duration,
    /// The longest wait for `load` and `transcribe` of a real model.
    pub load_timeout: Duration,
}

impl Suite {
    pub fn new(engine: impl Into<PathBuf>) -> Self {
        Suite {
            engine: engine.into(),
            env: Vec::new(),
            model: None,
            ready: None,
            timeout: Duration::from_secs(10),
            load_timeout: Duration::from_secs(600),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Passed,
    Failed(String),
    Skipped(String),
}

/// The result of one check.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub name: String,
    pub status: Status,
}

/// The results of a run.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub outcomes: Vec<Outcome>,
}

impl Report {
    pub fn failures(&self) -> Vec<&Outcome> {
        self.outcomes
            .iter()
            .filter(|o| matches!(o.status, Status::Failed(_)))
            .collect()
    }

    pub fn passed(&self) -> bool {
        self.failures().is_empty()
    }

    fn record(&mut self, name: &str, result: Result<(), String>) {
        let status = match result {
            Ok(()) => Status::Passed,
            Err(why) => Status::Failed(why),
        };
        self.outcomes.push(Outcome {
            name: name.to_string(),
            status,
        });
    }

    fn skip(&mut self, name: &str, why: &str) {
        self.outcomes.push(Outcome {
            name: name.to_string(),
            status: Status::Skipped(why.to_string()),
        });
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for outcome in &self.outcomes {
            match &outcome.status {
                Status::Passed => writeln!(f, "ok    {}", outcome.name)?,
                Status::Failed(why) => writeln!(f, "FAIL  {}: {why}", outcome.name)?,
                Status::Skipped(why) => writeln!(f, "skip  {}: {why}", outcome.name)?,
            }
        }
        let failed = self.failures().len();
        write!(f, "{} checks, {failed} failed", self.outcomes.len())
    }
}

/// Run every level that `suite` has the data for.
pub fn run(suite: &Suite) -> Report {
    let mut report = Report::default();
    let features = match hello(suite) {
        Ok(features) => {
            report.record("hello", Ok(()));
            features
        }
        Err(why) => {
            report.record("hello", Err(why));
            return report;
        }
    };
    core(suite, &features, &mut report);
    match &suite.model {
        Some(model) => model_level(suite, model, &mut report),
        None => report.skip("model level", "no model given"),
    }
    match &suite.ready {
        Some(ready) => ready_level(suite, ready, &features, &mut report),
        None => report.skip("ready level", "no models folder given"),
    }
    report
}

// ---------------------------------------------------------------------------
// Core
// ---------------------------------------------------------------------------

/// `hello`: the protocol version, the engine version, and the features.
fn hello(suite: &Suite) -> Result<Vec<String>, String> {
    let mut engine = Engine::start(suite, None)?;
    let reply = engine.request("hello", json!({}), suite.timeout)?;
    reply.expect_type("hello")?;
    let body = &reply.reply;
    if body["protocol"] != json!(1) {
        return Err(format!("protocol is {}, not 1", body["protocol"]));
    }
    match body["version"].as_str() {
        Some(v) if !v.is_empty() => {}
        _ => return Err(format!("version is not a string: {}", body["version"])),
    }
    let list = body["features"]
        .as_array()
        .ok_or_else(|| format!("features is not an array: {}", body["features"]))?;
    let mut features = Vec::new();
    for feature in list {
        let name = feature
            .as_str()
            .ok_or_else(|| format!("a feature is not a string: {feature}"))?;
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            return Err(format!("feature {name:?} is not snake_case"));
        }
        if features.iter().any(|f| f == name) {
            return Err(format!("feature {name} is listed twice"));
        }
        features.push(name.to_string());
    }
    engine.finish(suite.timeout)?;
    Ok(features)
}

fn core(suite: &Suite, features: &[String], report: &mut Report) {
    let has = |name: &str| features.iter().any(|f| f == name);

    report.record(
        "a line that is not JSON gets an error without id",
        with_engine(suite, None, |engine| {
            engine.send_line("this is not JSON")?;
            engine.error_without_id(suite.timeout)?;
            engine.request("hello", json!({}), suite.timeout)?.expect_type("hello")
        }),
    );
    report.record(
        "a line without type gets an error without id",
        with_engine(suite, None, |engine| {
            engine.send_line(r#"{"v":1,"id":"no-type"}"#)?;
            engine.error_without_id(suite.timeout)?;
            engine.request("hello", json!({}), suite.timeout)?.expect_type("hello")
        }),
    );
    report.record(
        "an unknown request type gets an error with its id",
        with_engine(suite, None, |engine| {
            engine
                .request("conformance_unknown", json!({}), suite.timeout)?
                .expect_error()
        }),
    );
    report.record(
        "a request without a required field gets an error with its id",
        with_engine(suite, None, |engine| {
            engine.request("load", json!({}), suite.timeout)?.expect_error()
        }),
    );
    report.record(
        "list_models with no models replies ok",
        with_engine(suite, None, |engine| {
            let reply = engine.request("list_models", json!({"models": []}), suite.timeout)?;
            reply.expect_type("ok")?;
            match reply.events.is_empty() {
                true => Ok(()),
                false => Err(format!("events for no models: {:?}", reply.events)),
            }
        }),
    );
    report.record(
        "requests that overlap get one reply each",
        with_engine(suite, None, |engine| {
            let ids: Vec<String> = (0..5)
                .map(|_| engine.send("hello", json!({})))
                .collect::<Result<_, _>>()?;
            for id in ids {
                engine.wait_reply(&id, suite.timeout)?.expect_type("hello")?;
            }
            Ok(())
        }),
    );
    report.record(
        "stream_audio for an unknown session gets an error without id",
        with_engine(suite, None, |engine| {
            let pcm = STANDARD.encode([0u8; 3200]);
            engine.send_line(&line("stream_audio", None, json!({"session": 987_654, "pcm": pcm})))?;
            engine.error_without_id(suite.timeout)
        }),
    );
    report.record(
        "stream_end for an unknown session gets an error with its id",
        with_engine(suite, None, |engine| {
            engine
                .request("stream_end", json!({"session": 987_654}), suite.timeout)?
                .expect_error()
        }),
    );
    if has("unload") {
        report.record(
            "unload of a model that is not in memory replies ok and sends no model_state",
            with_engine(suite, None, |engine| {
                let reply = engine.request("unload", json!({"model": "conformance-not-loaded"}), suite.timeout)?;
                reply.expect_type("ok")?;
                reply.expect_no_event("model_state")
            }),
        );
    } else {
        report.skip("unload", "the engine does not list the feature unload");
    }
    if has("language_model") {
        report.record(
            "language_model replies ok with available",
            with_engine(suite, None, |engine| {
                let reply = engine.request("language_model", json!({}), suite.timeout)?;
                reply.expect_type("ok")?;
                match reply.reply["available"] {
                    Value::Bool(true) => reply.expect_string("model"),
                    Value::Bool(false) => reply.expect_string("message"),
                    ref other => Err(format!("available is not a boolean: {other}")),
                }
            }),
        );
    } else {
        report.skip("language_model", "the engine does not list the feature language_model");
    }
    report.record(
        "shutdown replies ok, then the engine exits",
        with_engine(suite, None, |engine| {
            engine.request("shutdown", json!({}), suite.timeout)?.expect_type("ok")?;
            engine.wait_exit(Duration::from_secs(5))
        }),
    );
    report.record(
        "the engine exits when stdin closes",
        Engine::start(suite, None).and_then(|mut engine| {
            engine.stdin = None;
            engine.wait_exit(Duration::from_secs(5))?;
            engine.check_lines()
        }),
    );
}

// ---------------------------------------------------------------------------
// Model level: a model that is not on disk
// ---------------------------------------------------------------------------

fn model_level(suite: &Suite, model: &Model, report: &mut Report) {
    let entry = json!([{ "id": model.id, "engine": model.engine }]);
    report.record(
        "list_models reports not_downloaded with the request id, then ok",
        with_engine(suite, None, |engine| {
            let reply = engine.request("list_models", json!({ "models": entry }), suite.timeout)?;
            reply.expect_type("ok")?;
            reply.expect_state(&model.id, "not_downloaded")
        }),
    );
    report.record(
        "load of a model that is not downloaded gets an error",
        with_engine(suite, None, |engine| {
            engine
                .request("load", json!({ "model": model.id, "engine": model.engine }), suite.timeout)?
                .expect_error()
        }),
    );
    report.record(
        "transcribe with a model that is not loaded gets an error",
        with_engine(suite, None, |engine| {
            let wav = engine.models.path().join("silence.wav");
            write_wav(&wav, &[0; 16_000]).map_err(|e| e.to_string())?;
            let fields = json!({ "model": model.id, "path": wav, "language": "en", "vocabulary": [] });
            engine.request("transcribe", fields, suite.timeout)?.expect_error()
        }),
    );
    report.record(
        "stream_start with a model that is not loaded gets an error",
        with_engine(suite, None, |engine| {
            engine
                .request("stream_start", json!({ "session": 1, "model": model.id }), suite.timeout)?
                .expect_error()
        }),
    );
    report.record(
        "delete of a model that is not downloaded reports not_downloaded, then ok",
        with_engine(suite, None, |engine| {
            let reply = engine.request("delete", json!({ "model": model.id, "engine": model.engine }), suite.timeout)?;
            reply.expect_type("ok")?;
            reply.expect_state(&model.id, "not_downloaded")
        }),
    );
}

// ---------------------------------------------------------------------------
// Ready level: a model on disk
// ---------------------------------------------------------------------------

fn ready_level(suite: &Suite, ready: &Ready, features: &[String], report: &mut Report) {
    let model = &ready.model;
    let samples = match read_wav(&ready.audio) {
        Ok(samples) => samples,
        Err(why) => {
            report.record("read the audio file", Err(why));
            return;
        }
    };
    let mut engine = match Engine::start(suite, Some(&ready.models_dir)) {
        Ok(engine) => engine,
        Err(why) => {
            report.record("start the engine with the models folder", Err(why));
            return;
        }
    };
    let entry = json!([{ "id": model.id, "engine": model.engine }]);
    report.record("list_models reports downloaded", (|| {
        let reply = engine.request("list_models", json!({ "models": entry }), suite.timeout)?;
        reply.expect_type("ok")?;
        reply.expect_state(&model.id, "downloaded")
    })());

    let loaded = (|| {
        let reply = engine.request("load", json!({ "model": model.id, "engine": model.engine }), suite.load_timeout)?;
        reply.expect_type("ok")?;
        let ready_event = reply
            .events
            .iter()
            .find(|e| e["type"] == "model_state" && e["model"] == json!(model.id) && e["state"] == "ready")
            .ok_or_else(|| format!("no model_state ready before ok: {:?}", reply.events))?;
        match ready_event["load_ms"].as_u64() {
            Some(_) => Ok(()),
            None => Err(format!("model_state ready has no integer load_ms: {ready_event}")),
        }
    })();
    let load_failed = loaded.is_err();
    report.record("load reports ready with load_ms, then ok", loaded);
    if load_failed {
        report.skip("transcribe, stream, and unload", "the load failed");
        return;
    }

    report.record("transcribe replies final with text and elapsed_ms", (|| {
        let fields = json!({ "model": model.id, "path": ready.audio, "language": "en", "vocabulary": [] });
        let reply = engine.request("transcribe", fields, suite.load_timeout)?;
        reply.expect_type("final")?;
        reply.expect_string("text")?;
        match reply.reply["elapsed_ms"].as_u64() {
            Some(_) => Ok(()),
            None => Err(format!("elapsed_ms is not an integer: {}", reply.reply)),
        }
    })());
    report.record("transcribe of a missing file gets an error", (|| {
        let missing = engine.models.path().join("missing.wav");
        let fields = json!({ "model": model.id, "path": missing, "language": "en", "vocabulary": [] });
        engine.request("transcribe", fields, suite.load_timeout)?.expect_error()
    })());

    let has = |name: &str| features.iter().any(|f| f == name);
    if has("segments") {
        report.record("transcribe with segments gives timed segments in order", (|| {
            let fields = json!({ "model": model.id, "path": ready.audio, "language": "en", "vocabulary": [], "segments": true });
            let reply = engine.request("transcribe", fields, suite.load_timeout)?;
            reply.expect_type("final")?;
            let audio_ms = samples.len() as u64 * 1000 / 16_000;
            check_segments(&reply.reply["segments"], audio_ms)
        })());
    } else {
        report.skip("transcribe with segments", "the engine does not list the feature segments");
    }

    if ready.stream {
        report.record(
            "a stream sends partial events and ends with ok and text",
            stream(suite, &mut engine, &model.id, &samples),
        );
        if has("concurrent_streams") {
            report.record(
                "three streams on one model run at the same time",
                concurrent_streams(suite, &mut engine, &model.id, &samples),
            );
        } else {
            report.skip("concurrent streams", "the engine does not list the feature concurrent_streams");
        }
    } else {
        report.skip("stream", "the model has no live preview");
    }

    if features.iter().any(|f| f == "unload") {
        report.record("unload reports downloaded, then ok", (|| {
            let reply = engine.request("unload", json!({ "model": model.id }), suite.timeout)?;
            reply.expect_type("ok")?;
            reply.expect_state(&model.id, "downloaded")
        })());
    } else {
        report.skip("unload after load", "the engine does not list the feature unload");
    }
    report.record("the ready level ends with valid lines", engine.finish(suite.timeout));
}

/// Stream the audio in 160 ms chunks at eight times real time.
fn stream(suite: &Suite, engine: &mut Engine, model: &str, samples: &[i16]) -> Result<(), String> {
    const SESSION: u64 = 41;
    engine
        .request("stream_start", json!({ "session": SESSION, "model": model }), suite.load_timeout)?
        .expect_type("ok")?;
    for chunk in samples.chunks(2560) {
        let bytes: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
        let fields = json!({ "session": SESSION, "pcm": STANDARD.encode(bytes) });
        engine.send_line(&line("stream_audio", None, fields))?;
        std::thread::sleep(Duration::from_millis(20));
    }
    let reply = engine.request("stream_end", json!({ "session": SESSION }), suite.load_timeout)?;
    reply.expect_type("ok")?;
    reply.expect_string("text")?;
    if reply.reply["session"] != json!(SESSION) {
        return Err(format!("stream_end reply has session {}, not {SESSION}", reply.reply["session"]));
    }
    for partial in engine.other.iter().filter(|m| m["type"] == "partial") {
        if partial["session"] != json!(SESSION) || !partial["committed"].is_string() || !partial["tentative"].is_string() {
            return Err(format!("partial without session, committed, and tentative: {partial}"));
        }
    }
    // No partial of the session after the reply of stream_end.
    std::thread::sleep(Duration::from_millis(300));
    engine.drain();
    let late = engine.other.iter().skip(reply.other_seen).find(|m| m["type"] == "partial" && m["session"] == json!(SESSION));
    match late {
        Some(partial) => Err(format!("a partial came after stream_end: {partial}")),
        None => Ok(()),
    }
}

/// Three streams on one model, with their audio sent in turns. Each ends with
/// its own reply and text.
fn concurrent_streams(suite: &Suite, engine: &mut Engine, model: &str, samples: &[i16]) -> Result<(), String> {
    const SESSIONS: [u64; 3] = [51, 52, 53];
    for session in SESSIONS {
        engine
            .request("stream_start", json!({ "session": session, "model": model }), suite.load_timeout)?
            .expect_type("ok")
            .map_err(|why| format!("stream_start of session {session}: {why}"))?;
    }
    for chunk in samples.chunks(2560) {
        let bytes: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
        let pcm = STANDARD.encode(bytes);
        for session in SESSIONS {
            engine.send_line(&line("stream_audio", None, json!({ "session": session, "pcm": pcm })))?;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    for session in SESSIONS {
        let reply = engine.request("stream_end", json!({ "session": session }), suite.load_timeout)?;
        reply.expect_type("ok").map_err(|why| format!("stream_end of session {session}: {why}"))?;
        if reply.reply["session"] != json!(session) {
            return Err(format!("stream_end reply has session {}, not {session}", reply.reply["session"]));
        }
        match reply.reply["text"].as_str() {
            Some(text) if !text.trim().is_empty() => {}
            _ => return Err(format!("session {session} ended without text: {}", reply.reply)),
        }
    }
    Ok(())
}

/// The `segments` of a `final` reply: a list in time order, without overlaps,
/// inside the audio, with text. Words, when there, are inside their segment.
fn check_segments(segments: &Value, audio_ms: u64) -> Result<(), String> {
    // Times may pass the end of the audio by a little, for rounding.
    const SLACK_MS: u64 = 100;
    let list = segments.as_array().ok_or_else(|| format!("segments is not an array: {segments}"))?;
    if list.is_empty() {
        return Err("no segments for audio with speech".into());
    }
    let span = |item: &Value| -> Result<(u64, u64), String> {
        let (Some(start), Some(end)) = (item["start_ms"].as_u64(), item["end_ms"].as_u64()) else {
            return Err(format!("start_ms and end_ms must be integers: {item}"));
        };
        if start > end || end > audio_ms + SLACK_MS {
            return Err(format!("times are not inside the audio of {audio_ms} ms: {item}"));
        }
        match item["text"].as_str() {
            Some(text) if !text.trim().is_empty() => Ok((start, end)),
            _ => Err(format!("no text: {item}")),
        }
    };
    let mut last_end = 0;
    for segment in list {
        let (start, end) = span(segment)?;
        if start < last_end {
            return Err(format!("segment starts before the one in front of it ends ({last_end} ms): {segment}"));
        }
        last_end = end;
        if segment.get("words").is_none() {
            continue;
        }
        let words = segment["words"].as_array().ok_or_else(|| format!("words is not an array: {segment}"))?;
        for word in words {
            let (word_start, word_end) = span(word)?;
            if word_start < start || word_end > end {
                return Err(format!("word is not inside its segment: {word} in {segment}"));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The engine process
// ---------------------------------------------------------------------------

/// One reply and the events with the same id that came before it.
#[derive(Debug)]
struct Reply {
    reply: Value,
    events: Vec<Value>,
    /// How many messages without this id had arrived when the reply came.
    other_seen: usize,
}

impl Reply {
    fn kind(&self) -> &str {
        self.reply["type"].as_str().unwrap_or_default()
    }

    fn expect_type(&self, kind: &str) -> Result<(), String> {
        match self.kind() == kind {
            true => Ok(()),
            false => Err(format!("expected {kind}, got {}", self.reply)),
        }
    }

    fn expect_error(&self) -> Result<(), String> {
        self.expect_type("error")?;
        self.expect_string("message")
    }

    fn expect_string(&self, key: &str) -> Result<(), String> {
        match self.reply[key].is_string() {
            true => Ok(()),
            false => Err(format!("{key} is not a string: {}", self.reply)),
        }
    }

    fn expect_state(&self, model: &str, state: &str) -> Result<(), String> {
        let found = self
            .events
            .iter()
            .any(|e| e["type"] == "model_state" && e["model"] == json!(model) && e["state"] == json!(state));
        match found {
            true => Ok(()),
            false => Err(format!("no model_state {state} for {model} before the reply: {:?}", self.events)),
        }
    }

    fn expect_no_event(&self, kind: &str) -> Result<(), String> {
        match self.events.iter().find(|e| e["type"] == json!(kind)) {
            Some(event) => Err(format!("unexpected event {event}")),
            None => Ok(()),
        }
    }
}

struct Engine {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    /// The models folder. A temporary folder unless the ready level gives one.
    models: Folder,
    next_id: u64,
    /// Messages that no request has taken: events without id, errors without id, logs.
    other: Vec<Value>,
    /// Messages with an id that no request waits for (yet).
    pending: Vec<Value>,
    answered: HashSet<String>,
    problems: Vec<String>,
}

enum Folder {
    Temp(tempfile::TempDir),
    Given(PathBuf),
}

impl Folder {
    fn path(&self) -> &Path {
        match self {
            Folder::Temp(dir) => dir.path(),
            Folder::Given(path) => path,
        }
    }
}

/// Start an engine, run `check`, then check every line it wrote.
fn with_engine(
    suite: &Suite,
    models: Option<&Path>,
    check: impl FnOnce(&mut Engine) -> Result<(), String>,
) -> Result<(), String> {
    let mut engine = Engine::start(suite, models)?;
    check(&mut engine)?;
    engine.finish(suite.timeout)
}

fn line(kind: &str, id: Option<&str>, fields: Value) -> String {
    let mut object = match fields {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    object.insert("v".into(), json!(1));
    object.insert("type".into(), json!(kind));
    if let Some(id) = id {
        object.insert("id".into(), json!(id));
    }
    Value::Object(object).to_string()
}

impl Engine {
    fn start(suite: &Suite, models: Option<&Path>) -> Result<Engine, String> {
        let models = match models {
            Some(path) => Folder::Given(path.to_path_buf()),
            None => Folder::Temp(tempfile::tempdir().map_err(|e| format!("cannot make a models folder: {e}"))?),
        };
        let mut child = Command::new(&suite.engine)
            .env("SAYSO_MODELS_DIR", models.path())
            .envs(suite.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", suite.engine.display()))?;
        let stdout = child.stdout.take().expect("stdout is piped");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Engine {
            stdin: child.stdin.take(),
            child,
            lines,
            models,
            next_id: 0,
            other: Vec::new(),
            pending: Vec::new(),
            answered: HashSet::new(),
            problems: Vec::new(),
        })
    }

    fn send_line(&mut self, line: &str) -> Result<(), String> {
        let stdin = self.stdin.as_mut().ok_or("stdin is closed")?;
        writeln!(stdin, "{line}")
            .and_then(|()| stdin.flush())
            .map_err(|e| format!("cannot write to the engine: {e}"))
    }

    /// Send a request and return its id.
    fn send(&mut self, kind: &str, fields: Value) -> Result<String, String> {
        self.next_id += 1;
        let id = format!("conformance-{}", self.next_id);
        self.send_line(&line(kind, Some(&id), fields))?;
        Ok(id)
    }

    fn request(&mut self, kind: &str, fields: Value, timeout: Duration) -> Result<Reply, String> {
        let id = self.send(kind, fields)?;
        self.wait_reply(&id, timeout)
    }

    /// Wait for the reply to `id`, and keep the events with that id.
    fn wait_reply(&mut self, id: &str, timeout: Duration) -> Result<Reply, String> {
        let deadline = Instant::now() + timeout;
        let mut events = Vec::new();
        loop {
            let mine: Vec<Value> = {
                let (mine, rest) = std::mem::take(&mut self.pending)
                    .into_iter()
                    .partition(|m| m["id"] == json!(id));
                self.pending = rest;
                mine
            };
            for message in mine {
                if is_reply(&message) {
                    return Ok(Reply {
                        reply: message,
                        events,
                        other_seen: self.other.len(),
                    });
                }
                events.push(message);
            }
            self.receive(deadline)
                .map_err(|why| format!("no reply to {id}: {why}"))?;
        }
    }

    /// Wait for an `error` that has no id.
    fn error_without_id(&mut self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(error) = self.other.iter().find(|m| m["type"] == "error") {
                return match error["message"].is_string() {
                    true => Ok(()),
                    false => Err(format!("error without a message: {error}")),
                };
            }
            if let Some(error) = self.pending.iter().find(|m| m["type"] == "error") {
                return Err(format!("the error has an id: {error}"));
            }
            self.receive(deadline)
                .map_err(|why| format!("no error without id: {why}"))?;
        }
    }

    /// Read one line, check it, and file it.
    fn receive(&mut self, deadline: Instant) -> Result<(), String> {
        let left = deadline.saturating_duration_since(Instant::now());
        let line = match self.lines.recv_timeout(left) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => return Err("timed out".into()),
            Err(RecvTimeoutError::Disconnected) => return Err("the engine closed stdout".into()),
        };
        self.file(&line);
        Ok(())
    }

    /// Read every line that is there now.
    fn drain(&mut self) {
        while let Ok(line) = self.lines.try_recv() {
            self.file(&line);
        }
    }

    fn file(&mut self, line: &str) {
        let message: Value = match serde_json::from_str(line) {
            Ok(message @ Value::Object(_)) => message,
            _ => {
                self.problems.push(format!("not a JSON object: {line}"));
                return;
            }
        };
        if message["v"] != json!(1) {
            self.problems.push(format!("no \"v\":1: {line}"));
        }
        if !message["type"].is_string() {
            self.problems.push(format!("no string type: {line}"));
        }
        match message["id"].as_str() {
            Some(id) => {
                if is_reply(&message) && !self.answered.insert(id.to_string()) {
                    self.problems.push(format!("a second reply to {id}: {line}"));
                }
                if !is_reply(&message) && self.answered.contains(id) {
                    self.problems.push(format!("an event after the reply to {id}: {line}"));
                }
                self.pending.push(message);
            }
            None => self.other.push(message),
        }
    }

    fn wait_exit(&mut self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return Ok(()),
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                Ok(None) => return Err(format!("the engine did not exit within {timeout:?}")),
                Err(e) => return Err(format!("cannot wait for the engine: {e}")),
            }
        }
    }

    /// Close stdin, wait for the exit, and check every line.
    fn finish(mut self, timeout: Duration) -> Result<(), String> {
        self.stdin = None;
        if self.wait_exit(timeout).is_err() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        // The reader thread ends with stdout. Take what it read.
        while let Ok(line) = self.lines.recv_timeout(Duration::from_millis(200)) {
            self.file(&line);
        }
        self.check_lines()
    }

    fn check_lines(&mut self) -> Result<(), String> {
        self.drain();
        match self.problems.is_empty() {
            true => Ok(()),
            false => Err(self.problems.join("; ")),
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn is_reply(message: &Value) -> bool {
    matches!(message["type"].as_str(), Some("hello" | "ok" | "final" | "error"))
}

// ---------------------------------------------------------------------------
// WAV
// ---------------------------------------------------------------------------

/// Write 16 kHz mono 16-bit PCM.
pub fn write_wav(path: &Path, samples: &[i16]) -> std::io::Result<()> {
    let data = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&16_000u32.to_le_bytes());
    bytes.extend_from_slice(&32_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes)
}

/// Read a 16 kHz mono 16-bit PCM WAV file.
pub fn read_wav(path: &Path) -> Result<Vec<i16>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(format!("{} is not a WAV file", path.display()));
    }
    let mut at = 12;
    let mut format_ok = false;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().expect("4 bytes")) as usize;
        let body = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
        if id == b"fmt " && body.len() >= 16 {
            let u16_at = |i: usize| u16::from_le_bytes([body[i], body[i + 1]]);
            let rate = u32::from_le_bytes(body[4..8].try_into().expect("4 bytes"));
            format_ok = u16_at(0) == 1 && u16_at(2) == 1 && rate == 16_000 && u16_at(14) == 16;
        }
        if id == b"data" {
            if !format_ok {
                return Err(format!("{} is not 16 kHz mono 16-bit PCM", path.display()));
            }
            return Ok(body.as_chunks::<2>().0.iter().map(|&b| i16::from_le_bytes(b)).collect());
        }
        at += 8 + size + size % 2;
    }
    Err(format!("{} has no data chunk", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write_wav(&path, &[0, 1, -1, i16::MAX, i16::MIN]).unwrap();
        assert_eq!(read_wav(&path).unwrap(), vec![0, 1, -1, i16::MAX, i16::MIN]);
    }

    #[test]
    fn segments_must_be_in_order_inside_the_audio_with_text() {
        let good = json!([
            {"start_ms": 0, "end_ms": 900, "text": "Hello there.", "words": [
                {"start_ms": 100, "end_ms": 400, "text": "Hello"},
                {"start_ms": 450, "end_ms": 900, "text": "there."}
            ]},
            {"start_ms": 1200, "end_ms": 2050, "text": "Bye."}
        ]);
        assert_eq!(check_segments(&good, 2_000), Ok(()));
        let bad = [
            json!([]),
            json!([{"start_ms": 0, "end_ms": 500, "text": " "}]),
            json!([{"start_ms": 0, "end_ms": 900, "text": "a"}, {"start_ms": 800, "end_ms": 1000, "text": "b"}]),
            json!([{"start_ms": 0, "end_ms": 5_000, "text": "a"}]),
            json!([{"start_ms": 0.5, "end_ms": 900, "text": "a"}]),
            json!([{"start_ms": 100, "end_ms": 900, "text": "a", "words": [{"start_ms": 0, "end_ms": 200, "text": "a"}]}]),
        ];
        for segments in bad {
            assert!(check_segments(&segments, 2_000).is_err(), "{segments}");
        }
    }

    #[test]
    fn a_missing_engine_fails_hello() {
        let report = run(&Suite::new("/nonexistent/engine"));
        assert!(!report.passed());
        assert_eq!(report.outcomes.len(), 1);
    }
}
