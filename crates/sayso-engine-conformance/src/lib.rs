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
//!   time. With the features `vad` and `diarization`, and a VAD model or a
//!   diarization model in [`Ready`], it checks `vad`, a VAD session, and
//!   `diarize`. It does not download or delete.
//! - **store** needs a small VAD model and the feature `store_v1`. It runs two
//!   engines on one model store ([`store`]).
//!
//! Every line of every engine also gets these checks: it is a JSON object
//! with `"v":1` and a string `type`, and no request gets two replies.

mod store;

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
    /// A VAD model in the same folder. The suite checks it when the engine lists the feature `vad`.
    pub vad: Option<Model>,
    /// A diarization model in the same folder. The suite checks it when the engine lists the
    /// feature `diarization`.
    pub diarization: Option<Model>,
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
    /// A small VAD model of the catalog, for the store level. It uses the network once.
    pub store: Option<Model>,
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
            store: None,
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
    match &suite.store {
        Some(model) if features.iter().any(|f| f == "store_v1") => store::store_level(suite, model, &mut report),
        Some(_) => report.skip("store level", "the engine does not list store_v1"),
        None => report.skip("store level", "no VAD model given"),
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
    if has("catalog") {
        report.record(
            "list_models without models gives the catalog and a model_state for each model",
            with_engine(suite, None, |engine| {
                let reply = engine.request("list_models", json!({}), suite.timeout)?;
                reply.expect_type("ok")?;
                let ids = check_catalog(&reply.reply, has("detected_language"))?;
                for id in &ids {
                    if !reply.events.iter().any(|e| e["type"] == "model_state" && e["model"] == json!(id)) {
                        return Err(format!("no model_state for {id}"));
                    }
                }
                Ok(())
            }),
        );
        report.record(
            "a request without engine for an unknown id gets code unknown_model",
            with_engine(suite, None, |engine| {
                for kind in ["download", "load"] {
                    let reply = engine.request(kind, json!({"model": "conformance-no-such-model"}), suite.timeout)?;
                    reply.expect_error()?;
                    if reply.reply["code"] != "unknown_model" {
                        return Err(format!("{kind}: {}", reply.reply));
                    }
                }
                Ok(())
            }),
        );
    } else {
        report.skip("the built-in catalog", "the engine does not list the feature catalog");
    }
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

    let loaded = load_ready(suite, &mut engine, model);
    let load_failed = loaded.is_err();
    report.record("load reports ready with load_ms, then ok", loaded);
    if load_failed {
        report.skip("transcribe, stream, and unload", "the load failed");
        voice_models(suite, ready, features, &samples, &mut engine, report);
        report.record("the ready level ends with valid lines", engine.finish(suite.timeout));
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
    if has("detected_language") && has("catalog") {
        report.record(
            "transcribe with language auto reports the language when the catalog says so",
            detected_language(suite, &mut engine, model, &ready.audio),
        );
    } else {
        report.skip("the detected language", "the engine does not list the features detected_language and catalog");
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

    // The speech model is still loaded here, so the wrong-model checks of VAD and diarization use it.
    voice_models(suite, ready, features, &samples, &mut engine, report);

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

/// `load` of a model that is on disk: `model_state ready` with `load_ms`, then `ok`.
fn load_ready(suite: &Suite, engine: &mut Engine, model: &Model) -> Result<(), String> {
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

/// The catalog of a `list_models` reply without `models`: `models` with the
/// fields of the spec and unique ids, and `retired` with `id` and `message`.
/// Gives the ids.
fn check_catalog(reply: &Value, detected_language: bool) -> Result<Vec<String>, String> {
    const ROLES: [&str; 4] = ["final", "streaming", "vad", "diarization"];
    const LEVELS: [&str; 3] = ["word", "segment", "none"];
    let models = reply["models"].as_array().ok_or_else(|| format!("models is not an array: {reply}"))?;
    if models.is_empty() {
        return Err("the catalog has no models".into());
    }
    let mut ids = Vec::new();
    for model in models {
        let id = model["id"].as_str().filter(|id| !id.is_empty()).ok_or_else(|| format!("no id: {model}"))?;
        let strings = |key: &str| model[key].as_array().is_some_and(|list| list.iter().all(Value::is_string));
        let fine = model["name"].is_string()
            && model["engine"]["kind"].is_string()
            && model["size_bytes"].is_u64()
            && strings("languages")
            && strings("roles")
            && model["roles"].as_array().is_some_and(|roles| roles.iter().all(|r| ROLES.contains(&r.as_str().unwrap_or_default())))
            && LEVELS.contains(&model["timestamps"].as_str().unwrap_or_default())
            && model["recommended"].is_boolean()
            && (!detected_language || model["detects_language"].is_boolean());
        if !fine {
            return Err(format!("a catalog entry without the fields of the spec: {model}"));
        }
        if ids.iter().any(|known| known == id) {
            return Err(format!("{id} is in the catalog twice"));
        }
        ids.push(id.to_string());
    }
    let retired = reply["retired"].as_array().ok_or_else(|| format!("retired is not an array: {reply}"))?;
    for entry in retired {
        let (Some(id), Some(_)) = (entry["id"].as_str(), entry["message"].as_str()) else {
            return Err(format!("a retired entry without id and message: {entry}"));
        };
        if ids.iter().any(|known| known == id) {
            return Err(format!("{id} is in the catalog and retired"));
        }
    }
    Ok(ids)
}

/// The detected language (feature `detected_language`): with `language`
/// `auto`, `final` has `language` from the `languages` of the model when the
/// catalog says that the model finds it, and no `language` when it does not.
/// With a named language, `final` has no `language`.
fn detected_language(suite: &Suite, engine: &mut Engine, model: &Model, audio: &Path) -> Result<(), String> {
    let catalog = engine.request("list_models", json!({}), suite.timeout)?;
    catalog.expect_type("ok")?;
    let entry = catalog.reply["models"]
        .as_array()
        .and_then(|models| models.iter().find(|m| m["id"] == json!(model.id)))
        .ok_or_else(|| format!("{} is not in the catalog", model.id))?;
    let detects = entry["detects_language"]
        .as_bool()
        .ok_or_else(|| format!("detects_language is not a boolean: {entry}"))?;
    let languages: Vec<&str> = entry["languages"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();

    let fields = json!({ "model": model.id, "path": audio, "language": "auto", "vocabulary": [] });
    let reply = engine.request("transcribe", fields, suite.load_timeout)?;
    reply.expect_type("final")?;
    let found = reply.reply.get("language");
    let has_text = reply.reply["text"].as_str().is_some_and(|text| !text.trim().is_empty());
    match (detects && has_text, found) {
        (true, Some(Value::String(code))) if languages.contains(&code.as_str()) => {}
        (true, _) => return Err(format!("expected language from {languages:?} with language auto: {}", reply.reply)),
        (false, None) => {}
        (false, Some(_)) => return Err(format!("language for a model that does not find it: {}", reply.reply)),
    }

    let named = languages.first().copied().unwrap_or("en");
    let fields = json!({ "model": model.id, "path": audio, "language": named, "vocabulary": [] });
    let reply = engine.request("transcribe", fields, suite.load_timeout)?;
    reply.expect_type("final")?;
    match reply.reply.get("language") {
        None => Ok(()),
        Some(_) => Err(format!("language with the named language {named}: {}", reply.reply)),
    }
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
// Ready level: VAD and diarization
// ---------------------------------------------------------------------------

/// Times may pass the end of the audio, and the longest part, by a little, for rounding.
const SLACK_MS: u64 = 100;
/// The longest part of VAD (the spec, "VAD").
const MAX_PART_MS: u64 = 30_000;

/// The checks for the VAD model and the diarization model, when the engine lists the features.
fn voice_models(suite: &Suite, ready: &Ready, features: &[String], samples: &[i16], engine: &mut Engine, report: &mut Report) {
    let has = |name: &str| features.iter().any(|f| f == name);
    let audio_ms = samples.len() as u64 * 1000 / 16_000;
    match (&ready.vad, has("vad")) {
        (Some(model), true) => vad_checks(suite, ready, model, samples, features, engine, report),
        (None, true) => report.skip("VAD", "no VAD model given (--vad)"),
        (_, false) => report.skip("VAD", "the engine does not list the feature vad"),
    }
    match (&ready.diarization, has("diarization")) {
        (Some(model), true) => diarization_checks(suite, ready, model, audio_ms, features, engine, report),
        (None, true) => report.skip("diarization", "no diarization model given (--diarization)"),
        (_, false) => report.skip("diarization", "the engine does not list the feature diarization"),
    }
}

/// `list_models` says `downloaded`, then `load`. False when the model did not load.
fn load_extra(suite: &Suite, engine: &mut Engine, kind: &str, model: &Model, report: &mut Report) -> bool {
    let entry = json!([{ "id": model.id, "engine": model.engine }]);
    report.record(&format!("{kind}: list_models reports downloaded"), (|| {
        let reply = engine.request("list_models", json!({ "models": entry }), suite.timeout)?;
        reply.expect_type("ok")?;
        reply.expect_state(&model.id, "downloaded")
    })());
    let loaded = load_ready(suite, engine, model);
    let ok = loaded.is_ok();
    report.record(&format!("{kind}: load reports ready with load_ms, then ok"), loaded);
    ok
}

fn unload_extra(suite: &Suite, engine: &mut Engine, kind: &str, model: &Model, features: &[String], report: &mut Report) {
    if !features.iter().any(|f| f == "unload") {
        return;
    }
    report.record(&format!("{kind}: unload reports downloaded, then ok"), (|| {
        let reply = engine.request("unload", json!({ "model": model.id }), suite.timeout)?;
        reply.expect_type("ok")?;
        reply.expect_state(&model.id, "downloaded")
    })());
}

fn vad_checks(suite: &Suite, ready: &Ready, model: &Model, samples: &[i16], features: &[String], engine: &mut Engine, report: &mut Report) {
    let audio_ms = samples.len() as u64 * 1000 / 16_000;
    if !load_extra(suite, engine, "VAD", model, report) {
        report.skip("VAD: the other checks", "the load failed");
        return;
    }
    report.record("VAD: vad gives ordered parts inside the audio, none longer than 30 s", (|| {
        let reply = engine.request("vad", json!({ "model": model.id, "path": ready.audio }), suite.load_timeout)?;
        reply.expect_type("ok")?;
        check_speech_parts(&reply.reply["speech"], audio_ms)
    })());
    report.record("VAD: vad of a missing file gets an error", (|| {
        let missing = engine.models.path().join("missing.wav");
        engine
            .request("vad", json!({ "model": model.id, "path": missing }), suite.load_timeout)?
            .expect_error()
    })());
    report.record(
        "VAD: a session gives speech_start and speech_end in pairs, and stream_end replies ok",
        vad_session(suite, engine, &model.id, samples, audio_ms),
    );
    report.record("VAD: transcribe and stream_start with a VAD model get an error", (|| {
        let fields = json!({ "model": model.id, "path": ready.audio, "language": "en", "vocabulary": [] });
        engine.request("transcribe", fields, suite.load_timeout)?.expect_error()?;
        engine
            .request("stream_start", json!({ "session": 62, "model": model.id }), suite.timeout)?
            .expect_error()
    })());
    report.record("VAD: vad and vad_start with the speech model get an error", (|| {
        let speech = &ready.model.id;
        engine
            .request("vad", json!({ "model": speech, "path": ready.audio }), suite.load_timeout)?
            .expect_error()?;
        engine
            .request("vad_start", json!({ "session": 63, "model": speech }), suite.timeout)?
            .expect_error()
    })());
    unload_extra(suite, engine, "VAD", model, features, report);
}

/// A VAD session on the audio, in chunks of 160 ms at eight times real time.
fn vad_session(suite: &Suite, engine: &mut Engine, model: &str, samples: &[i16], audio_ms: u64) -> Result<(), String> {
    const SESSION: u64 = 61;
    engine
        .request("vad_start", json!({ "session": SESSION, "model": model }), suite.load_timeout)?
        .expect_type("ok")?;
    for chunk in samples.chunks(2560) {
        let bytes: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
        let fields = json!({ "session": SESSION, "pcm": STANDARD.encode(bytes) });
        engine.send_line(&line("stream_audio", None, fields))?;
        std::thread::sleep(Duration::from_millis(20));
    }
    let reply = engine.request("stream_end", json!({ "session": SESSION }), suite.load_timeout)?;
    reply.expect_type("ok")?;
    if reply.reply["session"] != json!(SESSION) {
        return Err(format!("stream_end reply has session {}, not {SESSION}", reply.reply["session"]));
    }
    let before: Vec<Value> = engine.other[..reply.other_seen]
        .iter()
        .filter(|m| m["session"] == json!(SESSION))
        .cloned()
        .collect();
    check_speech_events(&before, audio_ms)?;
    // No event of the session after the reply of stream_end.
    std::thread::sleep(Duration::from_millis(300));
    engine.drain();
    let late = engine.other.iter().skip(reply.other_seen).find(|m| {
        matches!(m["type"].as_str(), Some("speech_start" | "speech_end")) && m["session"] == json!(SESSION)
    });
    match late {
        Some(event) => Err(format!("an event came after stream_end: {event}")),
        None => Ok(()),
    }
}

fn diarization_checks(
    suite: &Suite,
    ready: &Ready,
    model: &Model,
    audio_ms: u64,
    features: &[String],
    engine: &mut Engine,
    report: &mut Report,
) {
    if !load_extra(suite, engine, "diarization", model, report) {
        report.skip("diarization: the other checks", "the load failed");
        return;
    }
    report.record("diarization: diarize gives numbered speakers and ordered turns inside the audio", (|| {
        let reply = engine.request("diarize", json!({ "model": model.id, "path": ready.audio }), suite.load_timeout)?;
        reply.expect_type("ok")?;
        check_turns(&reply.reply, audio_ms, None)
    })());
    report.record("diarization: diarize with speakers 1 finds exactly one speaker", (|| {
        let fields = json!({ "model": model.id, "path": ready.audio, "speakers": 1 });
        let reply = engine.request("diarize", fields, suite.load_timeout)?;
        reply.expect_type("ok")?;
        check_turns(&reply.reply, audio_ms, Some(1))
    })());
    report.record("diarization: diarize of a missing file gets an error", (|| {
        let missing = engine.models.path().join("missing.wav");
        engine
            .request("diarize", json!({ "model": model.id, "path": missing }), suite.load_timeout)?
            .expect_error()
    })());
    report.record("diarization: transcribe with a diarization model, and diarize with the speech model, get an error", (|| {
        let fields = json!({ "model": model.id, "path": ready.audio, "language": "en", "vocabulary": [] });
        engine.request("transcribe", fields, suite.load_timeout)?.expect_error()?;
        let fields = json!({ "model": ready.model.id, "path": ready.audio });
        engine.request("diarize", fields, suite.load_timeout)?.expect_error()
    })());
    unload_extra(suite, engine, "diarization", model, features, report);
}

/// The `speech` list of a `vad` reply: parts in time order, without overlaps, inside the
/// audio, at most 30 s long. The audio has speech, so there is at least one part.
fn check_speech_parts(speech: &Value, audio_ms: u64) -> Result<(), String> {
    let list = speech.as_array().ok_or_else(|| format!("speech is not an array: {speech}"))?;
    if list.is_empty() {
        return Err("no speech parts for audio with speech".into());
    }
    let mut last_end = 0;
    for part in list {
        let (Some(start), Some(end)) = (part["start_ms"].as_u64(), part["end_ms"].as_u64()) else {
            return Err(format!("start_ms and end_ms must be integers: {part}"));
        };
        check_part(start, end, audio_ms)?;
        if start < last_end {
            return Err(format!("part starts before the one in front of it ends ({last_end} ms): {part}"));
        }
        last_end = end;
    }
    Ok(())
}

/// One speech part: start before end, inside the audio, at most 30 s.
fn check_part(start: u64, end: u64, audio_ms: u64) -> Result<(), String> {
    if start > end || end > audio_ms + SLACK_MS {
        return Err(format!("{start} to {end} ms is not inside the audio of {audio_ms} ms"));
    }
    if end - start > MAX_PART_MS + SLACK_MS {
        return Err(format!("{start} to {end} ms is longer than 30 s"));
    }
    Ok(())
}

/// The events of one VAD session, in the order they came, up to the reply of `stream_end`:
/// `speech_start`, then `speech_end` with the same start, again and again. No speech is open at
/// the end, and there is at least one pair for audio with speech.
fn check_speech_events(events: &[Value], audio_ms: u64) -> Result<(), String> {
    let mut open: Option<u64> = None;
    let mut last_end = 0;
    let mut pairs = 0;
    for event in events {
        let kind = event["type"].as_str().unwrap_or_default();
        let time = |key: &str| event[key].as_u64().ok_or_else(|| format!("{key} is not an integer: {event}"));
        match kind {
            "speech_start" => {
                if open.is_some() {
                    return Err(format!("speech_start while speech is open: {event}"));
                }
                let start = time("start_ms")?;
                if start < last_end {
                    return Err(format!("speech starts before the last speech ended ({last_end} ms): {event}"));
                }
                open = Some(start);
            }
            "speech_end" => {
                let (start, end) = (time("start_ms")?, time("end_ms")?);
                if open != Some(start) {
                    return Err(format!("speech_end does not match a speech_start (open: {open:?}): {event}"));
                }
                check_part(start, end, audio_ms)?;
                open = None;
                last_end = end;
                pairs += 1;
            }
            _ => {}
        }
    }
    if open.is_some() {
        return Err("a speech_start has no speech_end before the reply of stream_end".into());
    }
    if pairs == 0 {
        return Err("no speech_start and speech_end for audio with speech".into());
    }
    Ok(())
}

/// The reply of `diarize` for audio with speech: `speakers` of 1 or more (exactly `expected`
/// when given), `turns` in the order of `start_ms`, inside the audio, with speaker numbers from 0 in
/// the order of first speech, and no overlap between two turns of one speaker.
fn check_turns(reply: &Value, audio_ms: u64, expected: Option<u64>) -> Result<(), String> {
    let speakers = reply["speakers"].as_u64().ok_or_else(|| format!("speakers is not an integer: {reply}"))?;
    if speakers == 0 {
        return Err("no speakers for audio with speech".into());
    }
    if let Some(expected) = expected
        && speakers != expected
    {
        return Err(format!("speakers is {speakers}, not {expected}"));
    }
    let turns = reply["turns"].as_array().ok_or_else(|| format!("turns is not an array: {reply}"))?;
    let mut next_speaker = 0;
    let mut last_start = 0;
    let mut last_end_of: Vec<u64> = Vec::new();
    for turn in turns {
        let (Some(start), Some(end), Some(speaker)) =
            (turn["start_ms"].as_u64(), turn["end_ms"].as_u64(), turn["speaker"].as_u64())
        else {
            return Err(format!("start_ms, end_ms, and speaker must be integers: {turn}"));
        };
        if start > end || end > audio_ms + SLACK_MS {
            return Err(format!("times are not inside the audio of {audio_ms} ms: {turn}"));
        }
        if start < last_start {
            return Err(format!("turns are not in the order of start_ms: {turn}"));
        }
        last_start = start;
        if speaker > next_speaker {
            return Err(format!("speaker {speaker} comes before speaker {next_speaker}: {turn}"));
        }
        if speaker == next_speaker {
            next_speaker += 1;
            last_end_of.push(0);
        }
        if start < last_end_of[speaker as usize] {
            return Err(format!("two turns of speaker {speaker} overlap: {turn}"));
        }
        last_end_of[speaker as usize] = end;
    }
    if next_speaker != speakers {
        return Err(format!("speakers is {speakers}, but the turns have {next_speaker} speakers"));
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
            Some(id) if matches!(message["type"].as_str(), Some("speech_start" | "speech_end")) => {
                self.problems.push(format!("a speech event with an id ({id}): {line}"));
                self.pending.push(message);
            }
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
    fn speech_parts_must_be_in_order_inside_the_audio_and_at_most_30_s() {
        let good = json!([{"start_ms": 100, "end_ms": 900}, {"start_ms": 900, "end_ms": 31_000}, {"start_ms": 31_000, "end_ms": 40_050}]);
        assert_eq!(check_speech_parts(&good, 40_000), Ok(()));
        let bad = [
            json!([]),
            json!({"start_ms": 0}),
            json!([{"start_ms": 0, "end_ms": 900}, {"start_ms": 800, "end_ms": 1000}]),
            json!([{"start_ms": 900, "end_ms": 100}]),
            json!([{"start_ms": 0, "end_ms": 40_500}]),
            json!([{"start_ms": 0, "end_ms": 31_000}]),
            json!([{"start_ms": 0.5, "end_ms": 900}]),
        ];
        for speech in bad {
            assert!(check_speech_parts(&speech, 40_000).is_err(), "{speech}");
        }
    }

    fn event(kind: &str, start_ms: u64, end_ms: Option<u64>) -> Value {
        let mut event = json!({"v": 1, "type": kind, "session": 61, "start_ms": start_ms});
        if let Some(end_ms) = end_ms {
            event["end_ms"] = json!(end_ms);
        }
        event
    }

    #[test]
    fn speech_events_come_in_pairs_in_order() {
        let good = [
            json!({"v": 1, "type": "partial", "session": 61, "committed": "", "tentative": ""}),
            event("speech_start", 100, None),
            event("speech_end", 100, Some(900)),
            event("speech_start", 1_500, None),
            event("speech_end", 1_500, Some(2_000)),
        ];
        assert_eq!(check_speech_events(&good, 2_000), Ok(()));
        let bad: [&[Value]; 7] = [
            &[],
            &[event("speech_start", 100, None)],
            &[event("speech_end", 100, Some(900))],
            &[event("speech_start", 100, None), event("speech_start", 200, None)],
            &[event("speech_start", 100, None), event("speech_end", 150, Some(900))],
            &[
                event("speech_start", 100, None),
                event("speech_end", 100, Some(900)),
                event("speech_start", 800, None),
                event("speech_end", 800, Some(1_000)),
            ],
            &[event("speech_start", 100, None), event("speech_end", 100, Some(5_000))],
        ];
        for events in bad {
            assert!(check_speech_events(events, 2_000).is_err(), "{events:?}");
        }
    }

    #[test]
    fn turns_have_numbered_speakers_in_order_without_overlap_for_one_speaker() {
        let good = json!({"speakers": 2, "turns": [
            {"start_ms": 0, "end_ms": 1_500, "speaker": 0},
            {"start_ms": 1_000, "end_ms": 2_000, "speaker": 1},
            {"start_ms": 1_800, "end_ms": 2_000, "speaker": 0}
        ]});
        assert_eq!(check_turns(&good, 2_000, None), Ok(()));
        assert_eq!(check_turns(&good, 2_000, Some(2)), Ok(()));
        assert!(check_turns(&good, 2_000, Some(1)).is_err());
        let bad = [
            json!({"speakers": 0, "turns": []}),
            json!({"speakers": 1, "turns": []}),
            json!({"speakers": 2, "turns": [{"start_ms": 0, "end_ms": 900, "speaker": 0}]}),
            json!({"speakers": 1, "turns": [{"start_ms": 0, "end_ms": 900, "speaker": 1}]}),
            json!({"speakers": 2, "turns": [
                {"start_ms": 0, "end_ms": 900, "speaker": 1},
                {"start_ms": 900, "end_ms": 1_000, "speaker": 0}
            ]}),
            json!({"speakers": 1, "turns": [
                {"start_ms": 500, "end_ms": 900, "speaker": 0},
                {"start_ms": 0, "end_ms": 400, "speaker": 0}
            ]}),
            json!({"speakers": 1, "turns": [
                {"start_ms": 0, "end_ms": 900, "speaker": 0},
                {"start_ms": 800, "end_ms": 1_000, "speaker": 0}
            ]}),
            json!({"speakers": 1, "turns": [{"start_ms": 0, "end_ms": 5_000, "speaker": 0}]}),
            json!({"speakers": 1.0, "turns": [{"start_ms": 0, "end_ms": 900, "speaker": 0}]}),
            json!({"speakers": 1, "turns": [{"start_ms": 0, "end_ms": 900}]}),
        ];
        for reply in bad {
            assert!(check_turns(&reply, 2_000, None).is_err(), "{reply}");
        }
    }

    #[test]
    fn a_catalog_needs_the_fields_of_the_spec() {
        let entry = json!({"id": "m", "name": "M", "engine": {"kind": "k"}, "size_bytes": 1, "languages": ["en"],
                           "roles": ["final"], "timestamps": "none", "recommended": false});
        let good = json!({"models": [entry], "retired": [{"id": "old", "message": "Use m."}]});
        assert_eq!(check_catalog(&good, false), Ok(vec!["m".to_string()]));
        assert!(check_catalog(&good, true).is_err(), "detected_language needs detects_language on each entry");
        let mut detects = good.clone();
        detects["models"][0]["detects_language"] = json!(false);
        assert_eq!(check_catalog(&detects, true), Ok(vec!["m".to_string()]));
        let mut bad_role = entry.clone();
        bad_role["roles"] = json!(["fly"]);
        let mut no_name = entry.clone();
        no_name.as_object_mut().unwrap().remove("name");
        for bad in [
            json!({"models": [], "retired": []}),
            json!({"models": [entry], "retired": [{"id": "m", "message": "x"}]}),
            json!({"models": [entry, entry], "retired": []}),
            json!({"models": [bad_role], "retired": []}),
            json!({"models": [no_name], "retired": []}),
            json!({"models": [entry]}),
        ] {
            assert!(check_catalog(&bad, false).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_missing_engine_fails_hello() {
        let report = run(&Suite::new("/nonexistent/engine"));
        assert!(!report.passed());
        assert_eq!(report.outcomes.len(), 1);
    }
}
