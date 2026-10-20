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

use serde_json::{Value, json};
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
        let kind = request["type"].as_str().unwrap_or_default().to_string();
        match kind.as_str() {
            "stream_audio" => {
                // Handled in order, like the real engine.
                chunks += 1;
                if let Some(ms) = fs::read_to_string(dir.join("slow-audio")).ok().and_then(|t| t.trim().parse().ok()) {
                    thread::sleep(Duration::from_millis(ms));
                }
                let len = request["pcm"].as_str().map_or(0, str::len);
                send(
                    &out,
                    json!({
                        "type": "partial",
                        "session": request["session"],
                        "committed": format!("chunk {chunks} len {len}"),
                        "tentative": "",
                    }),
                );
            }
            "shutdown" => {
                reply(&out, &request, "ok", json!({}));
                return;
            }
            _ => {
                let out = Arc::clone(&out);
                let dir = dir.clone();
                thread::spawn(move || handle(&out, &dir, &request, &kind));
            }
        }
    }
}

fn handle(out: &Arc<Mutex<std::io::Stdout>>, dir: &std::path::Path, request: &Value, kind: &str) {
    let model = request["model"].as_str().unwrap_or_default().to_string();
    match kind {
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
                json!({"version": "fake", "protocol": 1}),
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
            let mut state = json!({"model": model, "state": "downloaded"});
            if request["keep_preview"] == true {
                state["preview"] = json!(true);
            }
            reply(out, request, "model_state", state);
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
        "stream_start" | "stream_end" => reply(out, request, "ok", json!({})),
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
