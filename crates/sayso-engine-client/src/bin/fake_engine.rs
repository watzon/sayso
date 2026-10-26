//! A scripted stand-in for the SaysoEngine sidecar. Tests spawn it instead of the real engine.
//!
//! It speaks protocol v1 and records every request line in `$SAYSO_MODELS_DIR/requests.log`.
//! Model ids steer it:
//! - `slow-*`: `transcribe` answers after 300 ms (other requests may overtake it).
//! - `crash-once`: `transcribe` kills the process the first time (marker file `crashed`).
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
            "stream_start" => match request["session"].as_u64() {
                Some(session) => {
                    sessions.insert(session, String::new());
                    reply(&out, &request, "ok", json!({}));
                }
                None => reply(&out, &request, "error", json!({"message": "missing integer field session"})),
            },
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

fn handle(
    out: &Arc<Mutex<std::io::Stdout>>,
    dir: &std::path::Path,
    loaded: &Mutex<HashSet<String>>,
    request: &Value,
    kind: &str,
) {
    let model = request["model"].as_str().unwrap_or_default().to_string();
    let needs_model = matches!(kind, "download" | "delete" | "load" | "unload" | "prepare" | "transcribe");
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
            reply(
                out,
                request,
                "hello",
                json!({
                    "version": "fake",
                    "protocol": 1,
                    "features": ["keep_preview", "language_model", "prepare", "unload"],
                }),
            )
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
            let path = request["path"].as_str().unwrap_or_default();
            match fs::read(path) {
                Ok(bytes) if bytes.starts_with(b"RIFF") => reply(
                    out,
                    request,
                    "final",
                    json!({"text": format!("model={model} bytes={}", bytes.len()), "elapsed_ms": 5}),
                ),
                Ok(_) => reply(out, request, "error", json!({"message": "not a WAV file"})),
                Err(e) => reply(
                    out,
                    request,
                    "error",
                    json!({"message": format!("cannot read {path}: {e}")}),
                ),
            }
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
