//! The built `SaysoEngine` binary over raw NDJSON: no models needed.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

struct Engine {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
}

fn spawn(models: &std::path::Path, endpoint: Option<&str>) -> Engine {
    let mut command = Command::new(env!("CARGO_BIN_EXE_SaysoEngine"));
    command
        .env("SAYSO_MODELS_DIR", models)
        .env("SAYSO_ENGINE_LOG", "debug")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(endpoint) = endpoint {
        command.env("SAYSO_HF_ENDPOINT", endpoint);
    }
    let mut child = command.spawn().expect("the engine starts");
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, lines) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    std::thread::spawn(move || for _ in BufReader::new(stderr).lines().map_while(Result::ok) {});
    let stdin = child.stdin.take();
    Engine {
        child,
        stdin,
        lines,
    }
}

impl Engine {
    fn send_raw(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    }

    fn send(&mut self, mut request: Value) {
        request["v"] = json!(1);
        self.send_raw(&request.to_string());
    }

    /// Every message up to and including the reply (hello, ok, final, error) with `id`.
    fn until_reply(&self, id: &str) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut messages = Vec::new();
        loop {
            let line = self
                .lines
                .recv_timeout(deadline - Instant::now())
                .expect("a reply");
            let message: Value = serde_json::from_str(&line)
                .unwrap_or_else(|_| panic!("stdout line is not JSON: {line:?}"));
            assert_eq!(message["v"], 1, "{line}");
            let done = message["id"] == id
                && matches!(
                    message["type"].as_str(),
                    Some("hello" | "ok" | "final" | "error")
                );
            messages.push(message);
            if done {
                return messages;
            }
        }
    }

    /// The next message that is not a log line.
    fn next(&self) -> Value {
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(10))
                .expect("a message");
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["type"] != "log" {
                return message;
            }
        }
    }

    fn exits_within(&mut self, limit: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "the engine did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn hello_errors_and_exit_on_end_of_input() {
    let dir = tempfile::tempdir().unwrap();
    let mut engine = spawn(dir.path(), None);
    engine.send(json!({"type": "hello", "id": "r1"}));
    let hello = engine.until_reply("r1").pop().unwrap();
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["protocol"], 1);
    assert_eq!(hello["version"], env!("CARGO_PKG_VERSION"));

    engine.send_raw("this is not json");
    let error = engine.next();
    assert_eq!(error["type"], "error");
    assert_eq!(error["message"], "invalid JSON line");
    assert!(error.get("id").is_none());

    // Bytes that are not UTF-8.
    engine
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&[0xff, 0xfe, b'{', b'\n'])
        .unwrap();
    engine.stdin.as_mut().unwrap().flush().unwrap();
    assert_eq!(engine.next()["type"], "error");

    engine.send(json!({"type": "warp", "id": "r2"}));
    let reply = engine.until_reply("r2").pop().unwrap();
    assert_eq!(reply["message"], "unknown request type warp");

    engine.send(
        json!({"type": "download", "id": "r3", "model": "m", "engine": {"kind": "teleport"}}),
    );
    assert_eq!(
        engine.until_reply("r3").pop().unwrap()["message"],
        "unknown engine kind teleport"
    );

    engine.send(json!({"type": "load", "id": "r4", "model": "whisper-tiny", "engine": {"kind": "whisper_cpp", "file": "ggml-tiny.bin"}}));
    assert_eq!(
        engine.until_reply("r4").pop().unwrap()["message"],
        "model whisper-tiny is not downloaded"
    );

    engine.send(json!({"type": "stream_end", "id": "r5", "session": 4}));
    assert_eq!(
        engine.until_reply("r5").pop().unwrap()["message"],
        "unknown stream session 4"
    );

    engine.send(json!({"type": "transcribe", "id": "r6", "model": "nope", "path": "x.wav"}));
    assert_eq!(engine.until_reply("r6").pop().unwrap()["type"], "error");

    // stdin closes: the engine exits.
    engine.stdin.take();
    assert!(engine.exits_within(Duration::from_secs(5)).success());
}

#[test]
fn shutdown_replies_then_exits() {
    let dir = tempfile::tempdir().unwrap();
    let mut engine = spawn(dir.path(), None);
    engine.send(json!({"type": "shutdown", "id": "s"}));
    assert_eq!(engine.until_reply("s").pop().unwrap()["type"], "ok");
    assert!(engine.exits_within(Duration::from_secs(5)).success());
}

#[test]
fn missing_models_dir_is_an_error() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_SaysoEngine"))
        .env_remove("SAYSO_MODELS_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let message: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(message["message"], "SAYSO_MODELS_DIR is not set");
    assert_eq!(child.wait().unwrap().code(), Some(2));
}

/// Download from a local server, then a load of a file that is not a model
/// fails with an error (and no crash), and delete removes it.
#[test]
fn download_a_fake_model_from_a_local_server() {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", server.server_addr().to_ip().unwrap());
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let log = std::sync::Arc::clone(&seen);
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            log.lock()
                .unwrap()
                .push(format!("{} {}", request.method(), request.url()));
            let body = vec![b'x'; 300_000];
            let response = tiny_http::Response::from_data(body).with_chunked_threshold(usize::MAX);
            let _ = request.respond(response);
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let mut engine = spawn(dir.path(), Some(&endpoint));
    let whisper = json!({"kind": "whisper_cpp", "file": "ggml-tiny.bin"});
    engine.send(json!({"type": "download", "id": "d", "model": "whisper-tiny", "engine": whisper, "size_bytes": 77_691_713}));
    let messages = engine.until_reply("d");
    assert_eq!(messages.last().unwrap()["type"], "ok", "{messages:?}");
    let progress: Vec<&Value> = messages
        .iter()
        .filter(|m| m["type"] == "download_progress")
        .collect();
    assert!(!progress.is_empty());
    assert_eq!(progress.last().unwrap()["bytes_total"], 300_000);
    assert_eq!(progress.last().unwrap()["fraction"], 1.0);
    assert!(
        messages
            .iter()
            .any(|m| m["type"] == "model_state" && m["state"] == "downloaded")
    );
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|r| r == "GET /ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin")
    );
    let folder = dir.path().join("whisper-tiny");
    assert!(folder.join(".sayso-downloaded").exists());
    assert_eq!(
        std::fs::metadata(folder.join("ggml-tiny.bin"))
            .unwrap()
            .len(),
        300_000
    );

    engine.send(json!({"type": "list_models", "id": "l", "models": [{"id": "whisper-tiny", "engine": whisper}]}));
    let list = engine.until_reply("l");
    assert_eq!(
        list.iter().find(|m| m["type"] == "model_state").unwrap()["state"],
        "downloaded"
    );

    engine.send(json!({"type": "load", "id": "x", "model": "whisper-tiny", "engine": whisper}));
    let load = engine.until_reply("x");
    assert_eq!(load.last().unwrap()["type"], "error", "{load:?}");
    assert!(load.iter().any(|m| m["state"] == "failed"));

    engine.send(json!({"type": "delete", "id": "del", "model": "whisper-tiny", "engine": whisper}));
    assert_eq!(engine.until_reply("del").pop().unwrap()["type"], "ok");
    assert!(!folder.exists());

    engine.send(json!({"type": "hello", "id": "alive"}));
    assert_eq!(engine.until_reply("alive").pop().unwrap()["type"], "hello");
}
