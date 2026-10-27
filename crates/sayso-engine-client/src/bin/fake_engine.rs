//! A scripted stand-in for the SaysoEngine sidecar. Tests spawn it instead of the real engine.
//!
//! It speaks protocol v1 and records every request line in `$SAYSO_MODELS_DIR/requests.log`.
//! Model ids steer it:
//! - `slow-*`: `transcribe` answers after 300 ms (other requests may overtake it).
//! - `crash-once`: `transcribe` kills the process the first time (marker file `crashed`).
//!
//! `transcribe` with `segments: true` answers with three fixed segments, spread over the length of
//! the audio (the first has two words). `vad` answers with two speech parts: the first half and
//! the last quarter of the audio. `vad_start` opens a VAD session that takes `stream_audio`
//! like a stream: the first frame of each pair starts speech and the second ends it, and
//! `stream_end` ends speech that is still open. `diarize` answers with `speakers` turns (2 when
//! the request has no `speakers`), each of the same length. `never-*` models never answer
//! `transcribe`, `vad`, or `diarize`. The files `no-timing` (no `segments`, `vad`, and
//! `diarization` features; the requests get an `error`) and `no-features` (like `no-timing`,
//! and `hello` lists no features at all) turn the new requests off.
//!
//! `generate` answers with its prompt in upper case. The prompt steers it: `refuse` and
//! `unavailable` give an error with that code, and `slow` never answers. `language_model`
//! reports the model `Fake LM`, or "not available" when the file `no-language-model` exists. The
//! text of that file is the `reason` code of the reply. An empty file sends no `reason`, like an
//! older engine.
//!
//! The file `fail-download` makes `download` end with `model_state failed` and then an `error`.
//! The file `fail-load` does the same for `load`. The text of the file is the `reason` code
//! (empty: no `reason`).
//!
//! The file `slow-audio` makes it read each `stream_audio` line slowly: it waits that many
//! milliseconds (the text of the file) after each one, like an engine that falls behind.
//!
//! The file `max-streams` makes `stream_start` and `vad_start` fail with the code
//! `too_many_streams` when that many streams and VAD sessions (the text of the file) are open.
//!
//! `download` reports 50 % at once, waits 300 ms, then reports `downloaded`.
//!
//! The file `hello-delay-ms` makes `hello` wait that many milliseconds. The file
//! `exit-at-hello` makes the process end at `hello`, with no answer.
//!
//! It passes the core and ready levels of `sayso-engine-conformance`. It does
//! not keep download or load state for `load`, `transcribe`, and
//! `stream_start`, so it does not pass the model level.

use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn main() {
    let dir = PathBuf::from(std::env::var("SAYSO_MODELS_DIR").expect("SAYSO_MODELS_DIR is set"));
    let out = Arc::new(Mutex::new(std::io::stdout()));
    fs::write(dir.join("pid"), std::process::id().to_string()).ok();
    send(
        &out,
        json!({"type": "log", "level": "info", "message": "fake engine ready"}),
    );

    let mut chunks = 0u64;
    // The open VAD sessions.
    let mut vad_sessions: HashMap<u64, Vad> = HashMap::new();
    // The text of each open stream session, for the reply of `stream_end`.
    let mut sessions: HashMap<u64, String> = HashMap::new();
    let loaded = Arc::new(Mutex::new(HashSet::new()));
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(mut log) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("requests.log"))
        {
            let _ = writeln!(log, "{line}");
        }
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            send(
                &out,
                json!({"type": "error", "message": "invalid JSON line"}),
            );
            continue;
        };
        let Some(kind) = request["type"].as_str().map(str::to_string) else {
            send(&out, json!({"type": "error", "message": "missing type"}));
            continue;
        };
        match kind.as_str() {
            // The stream requests run in order, like the real engines.
            "stream_start" | "vad_start" if streams_full(&dir, sessions.len() + vad_sessions.len()) => {
                reply(&out, &request, "error", json!({"message": "the streams are full; end one first", "code": "too_many_streams"}));
            }
            "stream_start" => match request["session"].as_u64() {
                Some(session) => {
                    sessions.insert(session, String::new());
                    reply(&out, &request, "ok", json!({}));
                }
                None => reply(&out, &request, "error", json!({"message": "missing integer field session"})),
            },
            "vad_start" => match request["session"].as_u64() {
                Some(_) if timing_off(&dir) => {
                    reply(&out, &request, "error", json!({"message": "unknown request type vad_start"}));
                }
                Some(session) => {
                    vad_sessions.insert(session, Vad::default());
                    reply(&out, &request, "ok", json!({}));
                }
                None => reply(&out, &request, "error", json!({"message": "missing integer field session"})),
            },
            "stream_audio" if request["session"].as_u64().is_some_and(|s| vad_sessions.contains_key(&s)) => {
                let session = request["session"].as_u64().unwrap_or_default();
                if let Some(ms) = fs::read_to_string(dir.join("slow-audio")).ok().and_then(|t| t.trim().parse().ok()) {
                    thread::sleep(Duration::from_millis(ms));
                }
                let vad = vad_sessions.get_mut(&session).expect("the session is open");
                // Base64 holds 3 bytes in 4 characters, and a sample has 2 bytes.
                let frame_ms = request["pcm"].as_str().map_or(0, str::len) as u64 * 3 / 4 / 2 / 16;
                vad.frames += 1;
                if vad.frames % 2 == 1 {
                    vad.open = Some(vad.at_ms);
                    send(&out, json!({"type": "speech_start", "session": session, "start_ms": vad.at_ms}));
                } else if let Some(start_ms) = vad.open.take() {
                    send(&out, json!({
                        "type": "speech_end", "session": session, "start_ms": start_ms, "end_ms": vad.at_ms + frame_ms,
                    }));
                }
                vad.at_ms += frame_ms;
            }
            "stream_end" if request["session"].as_u64().is_some_and(|s| vad_sessions.contains_key(&s)) => {
                let session = request["session"].as_u64().unwrap_or_default();
                let vad = vad_sessions.remove(&session).expect("the session is open");
                if let Some(start_ms) = vad.open {
                    send(&out, json!({"type": "speech_end", "session": session, "start_ms": start_ms, "end_ms": vad.at_ms}));
                }
                reply(&out, &request, "ok", json!({"session": session}));
            }
            "stream_audio" => {
                let session = request["session"].as_u64().unwrap_or_default();
                let Some(text) = sessions.get_mut(&session) else {
                    send(&out, json!({"type": "error", "message": format!("unknown stream session {session}")}));
                    continue;
                };
                chunks += 1;
                if let Some(ms) = fs::read_to_string(dir.join("slow-audio")).ok().and_then(|t| t.trim().parse().ok()) {
                    thread::sleep(Duration::from_millis(ms));
                }
                let len = request["pcm"].as_str().map_or(0, str::len);
                *text = format!("chunk {chunks} len {len}");
                send(
                    &out,
                    json!({
                        "type": "partial",
                        "session": session,
                        "committed": text,
                        "tentative": "",
                    }),
                );
            }
            "stream_end" => {
                let session = request["session"].as_u64().unwrap_or_default();
                match sessions.remove(&session) {
                    Some(text) => reply(&out, &request, "ok", json!({"session": session, "text": text})),
                    None => reply(&out, &request, "error", json!({"message": format!("unknown stream session {session}")})),
                }
            }
            "shutdown" => {
                reply(&out, &request, "ok", json!({}));
                return;
            }
            _ => {
                let out = Arc::clone(&out);
                let dir = dir.clone();
                let loaded = Arc::clone(&loaded);
                thread::spawn(move || handle(&out, &dir, &loaded, &request, &kind));
            }
        }
    }
}

/// A VAD session of the fake engine.
#[derive(Default)]
struct Vad {
    frames: u64,
    /// Milliseconds of audio received before the next frame.
    at_ms: u64,
    /// The start of the speech that is open.
    open: Option<u64>,
}

/// True when `open` streams fill the limit of the file `max-streams`.
fn streams_full(dir: &std::path::Path, open: usize) -> bool {
    fs::read_to_string(dir.join("max-streams")).ok().and_then(|t| t.trim().parse::<usize>().ok()).is_some_and(|max| open >= max)
}

/// The markers `no-timing` and `no-features` turn off `segments`, `vad`, and `diarization`.
fn timing_off(dir: &std::path::Path) -> bool {
    dir.join("no-timing").exists() || dir.join("no-features").exists()
}

fn handle(
    out: &Arc<Mutex<std::io::Stdout>>,
    dir: &std::path::Path,
    loaded: &Mutex<HashSet<String>>,
    request: &Value,
    kind: &str,
) {
    let model = request["model"].as_str().unwrap_or_default().to_string();
    let needs_model = matches!(
        kind,
        "download" | "delete" | "load" | "unload" | "prepare" | "transcribe" | "vad" | "diarize"
    );
    if needs_model && request["model"].as_str().is_none() {
        reply(out, request, "error", json!({"message": "missing string field model"}));
        return;
    }
    match kind {
        "prepare" => {
            if dir.join("reject-prepare").exists() {
                reply(out, request, "error", json!({"message": "unknown request type prepare"}));
                return;
            }
            for part in ["preview", "dictionary"] {
                let wanted = if part == "preview" { request["preview"] == true }
                    else { request["vocabulary"].as_array().is_some_and(|v| !v.is_empty()) };
                if !wanted { continue; }
                for state in ["preparing", "ready"] {
                    if state == "ready" { thread::sleep(Duration::from_millis(100)); }
                    send(out, json!({"type":"part_state", "id":request["id"], "model":model, "part":part, "state":state}));
                }
            }
            reply(out, request, "ok", json!({}));
        }

        "hello" => {
            if dir.join("exit-at-hello").exists() {
                std::process::exit(3);
            }
            let delay = fs::read_to_string(dir.join("hello-delay-ms")).ok().and_then(|ms| ms.trim().parse().ok());
            if let Some(ms) = delay {
                thread::sleep(Duration::from_millis(ms));
            }
            let features = if dir.join("no-features").exists() {
                json!([])
            } else if timing_off(dir) {
                json!(["keep_preview", "language_model", "prepare", "unload"])
            } else {
                json!(["diarization", "keep_preview", "language_model", "prepare", "segments", "unload", "vad"])
            };
            reply(out, request, "hello", json!({"version": "fake", "protocol": 1, "features": features}))
        }
        "list_models" => {
            for entry in request["models"].as_array().into_iter().flatten() {
                let id = entry["id"].as_str().unwrap_or_default();
                let state = if dir.join(format!("downloaded-{id}")).exists() {
                    "downloaded"
                } else {
                    "not_downloaded"
                };
                reply(
                    out,
                    request,
                    "model_state",
                    json!({"model": id, "state": state}),
                );
            }
            reply(out, request, "ok", json!({}));
        }
        "download" => {
            let total = request["size_bytes"].as_u64().unwrap_or(0);
            let progress = json!({"model": model, "fraction": 0.5, "bytes_done": total / 2, "bytes_total": total});
            reply(out, request, "download_progress", progress.clone());
            reply(
                out,
                request,
                "model_state",
                with_state(&progress, "downloading"),
            );
            thread::sleep(Duration::from_millis(300));
            if fail(out, dir, request, &model, "fail-download", "download stopped") {
                return;
            }
            fs::write(dir.join(format!("downloaded-{model}")), "").ok();
            reply(
                out,
                request,
                "model_state",
                json!({"model": model, "state": "downloaded"}),
            );
            reply(out, request, "ok", json!({}));
        }
        "delete" => {
            loaded.lock().unwrap().remove(&model);
            fs::remove_file(dir.join(format!("downloaded-{model}"))).ok();
            reply(
                out,
                request,
                "model_state",
                json!({"model": model, "state": "not_downloaded"}),
            );
            reply(out, request, "ok", json!({}));
        }
        "load" => {
            reply(
                out,
                request,
                "model_state",
                json!({"model": model, "state": "optimizing"}),
            );
            if fail(out, dir, request, &model, "fail-load", "load stopped") {
                return;
            }
            loaded.lock().unwrap().insert(model.clone());
            reply(
                out,
                request,
                "model_state",
                json!({"model": model, "state": "ready", "load_ms": 1}),
            );
            reply(out, request, "ok", json!({}));
        }
        "unload" => {
            // Like the Linux engine: with `keep_preview`, the live preview stays.
            // A model that is not in memory gets only `ok`.
            if loaded.lock().unwrap().remove(&model) {
                let mut state = json!({"model": model, "state": "downloaded"});
                if request["keep_preview"] == true {
                    state["preview"] = json!(true);
                }
                reply(out, request, "model_state", state);
            }
            reply(out, request, "ok", json!({}));
        }
        "transcribe" => {
            if model == "crash-once" && !dir.join("crashed").exists() {
                fs::write(dir.join("crashed"), "").ok();
                std::process::exit(7);
            }
            if model.starts_with("slow-") {
                thread::sleep(Duration::from_millis(300));
            }
            if model.starts_with("never-") {
                return;
            }
            let Some(bytes) = read_wav(out, request) else {
                return;
            };
            let mut body = json!({"text": format!("model={model} bytes={}", bytes.len()), "elapsed_ms": 5});
            if request["segments"] == true && !timing_off(dir) {
                let d = audio_ms(&bytes);
                body["segments"] = json!([
                    {"start_ms": 0, "end_ms": d / 2, "text": "Hello there.", "words": [
                        {"start_ms": 0, "end_ms": d / 4, "text": "Hello"},
                        {"start_ms": d / 4, "end_ms": d / 2, "text": "there."},
                    ]},
                    {"start_ms": d / 2, "end_ms": d * 3 / 4, "text": "Bye.", "words": []},
                    {"start_ms": d * 3 / 4, "end_ms": d, "text": "Later"},
                ]);
            }
            reply(out, request, "final", body);
        }
        "vad" | "diarize" if timing_off(dir) => reply(
            out,
            request,
            "error",
            json!({"message": format!("unknown request type {kind}")}),
        ),
        "vad" => {
            if model.starts_with("never-") {
                return;
            }
            let Some(bytes) = read_wav(out, request) else {
                return;
            };
            let d = audio_ms(&bytes);
            reply(
                out,
                request,
                "ok",
                json!({"speech": [{"start_ms": 0, "end_ms": d / 2}, {"start_ms": d * 3 / 4, "end_ms": d}]}),
            );
        }
        "diarize" => {
            if model.starts_with("never-") {
                return;
            }
            let Some(bytes) = read_wav(out, request) else {
                return;
            };
            let d = audio_ms(&bytes);
            let speakers = request["speakers"].as_u64().unwrap_or(2).max(1);
            let turns: Vec<Value> = (0..speakers)
                .map(|k| json!({"start_ms": d * k / speakers, "end_ms": d * (k + 1) / speakers, "speaker": k}))
                .collect();
            reply(out, request, "ok", json!({"speakers": speakers, "turns": turns}));
        }
        "language_model" => {
            let body = if dir.join("no-language-model").exists() {
                let mut body = json!({"available": false, "message": "Apple Intelligence is off"});
                let reason = fs::read_to_string(dir.join("no-language-model")).unwrap_or_default();
                if !reason.trim().is_empty() {
                    body["reason"] = json!(reason.trim());
                }
                body
            } else {
                json!({"available": true, "model": "Fake LM"})
            };
            reply(out, request, "ok", body);
        }
        "generate" => match request["prompt"].as_str().unwrap_or_default() {
            "slow" => {}
            "refuse" => reply(
                out,
                request,
                "error",
                json!({"message": "the model refused", "code": "refused"}),
            ),
            "unavailable" => reply(
                out,
                request,
                "error",
                json!({"message": "the model is not ready", "code": "unavailable"}),
            ),
            prompt => reply(
                out,
                request,
                "ok",
                json!({"text": prompt.to_uppercase(), "model": "Fake LM"}),
            ),
        },
        other => reply(
            out,
            request,
            "error",
            json!({"message": format!("unknown request type {other}")}),
        ),
    }
}

/// The bytes of the WAV file of a request. Sends the `error` reply and gives None when it cannot.
fn read_wav(out: &Arc<Mutex<std::io::Stdout>>, request: &Value) -> Option<Vec<u8>> {
    let path = request["path"].as_str().unwrap_or_default();
    match fs::read(path) {
        Ok(bytes) if bytes.starts_with(b"RIFF") => Some(bytes),
        Ok(_) => {
            reply(out, request, "error", json!({"message": "not a WAV file"}));
            None
        }
        Err(e) => {
            reply(out, request, "error", json!({"message": format!("cannot read {path}: {e}")}));
            None
        }
    }
}

/// The length of a 16 kHz mono 16-bit WAV file in milliseconds.
fn audio_ms(wav: &[u8]) -> u64 {
    (wav.len() as u64).saturating_sub(44) / 2 / 16
}

fn with_state(value: &Value, state: &str) -> Value {
    let mut copy = value.clone();
    copy["state"] = json!(state);
    copy
}

/// Send a message that echoes the request id.
/// When the marker file exists, send `model_state failed` (with the `reason` from the file)
/// and then the `error` reply, like the real engines. Returns whether it did.
fn fail(
    out: &Arc<Mutex<std::io::Stdout>>,
    dir: &std::path::Path,
    request: &Value,
    model: &str,
    marker: &str,
    message: &str,
) -> bool {
    let Ok(reason) = fs::read_to_string(dir.join(marker)) else {
        return false;
    };
    let mut state = json!({"model": model, "state": "failed", "message": message});
    if !reason.trim().is_empty() {
        state["reason"] = json!(reason.trim());
    }
    reply(out, request, "model_state", state);
    reply(out, request, "error", json!({"message": message}));
    true
}

fn reply(out: &Arc<Mutex<std::io::Stdout>>, request: &Value, kind: &str, mut body: Value) {
    body["type"] = json!(kind);
    if let Some(id) = request.get("id") {
        body["id"] = id.clone();
    }
    send(out, body);
}

fn send(out: &Arc<Mutex<std::io::Stdout>>, mut body: Value) {
    body["v"] = json!(1);
    let mut out = out.lock().unwrap();
    let _ = writeln!(out, "{body}");
    let _ = out.flush();
}
