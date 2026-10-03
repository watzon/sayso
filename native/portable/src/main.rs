//! SaysoEngine: NDJSON requests on stdin, NDJSON replies and events on stdout,
//! logs on stderr. Exits when stdin closes.

use sayso_engine::engine::{Config, Engine};
use sayso_engine::protocol::{ENGINE_VERSION, Output, Request};
use sayso_engine::{recognizer, stdio};
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::Arc;

fn main() {
    // First, before any library can print: stdout is for protocol lines only.
    let out = Output::new(stdio::protocol_writer());
    env_logger::Builder::from_env(
        env_logger::Env::new().filter_or("SAYSO_ENGINE_LOG", "info,ort=warn,transcribe_rs=warn"),
    )
    .target(env_logger::Target::Stderr)
    .init();

    let Some(models_dir) = std::env::var_os("SAYSO_MODELS_DIR").filter(|d| !d.is_empty()) else {
        out.error(None, "SAYSO_MODELS_DIR is not set");
        std::process::exit(2);
    };
    let models_dir = PathBuf::from(models_dir);
    if let Err(e) = std::fs::create_dir_all(&models_dir) {
        out.error(
            None,
            &format!("cannot create {}: {e}", models_dir.display()),
        );
    }
    let mut config = Config::new(models_dir.clone());
    if let Some(endpoint) = std::env::var("SAYSO_HF_ENDPOINT")
        .ok()
        .filter(|e| !e.is_empty())
    {
        config.hf_endpoint = endpoint;
    }
    let engine = Engine::new(out.clone(), config, Box::new(recognizer::load));
    out.log(
        "info",
        &format!(
            "SaysoEngine {ENGINE_VERSION} ready, models in {}, pid {}",
            models_dir.display(),
            std::process::id()
        ),
    );

    // Streaming requests run in arrival order on one thread.
    let (stream_tx, stream_rx) = std::sync::mpsc::channel::<Request>();
    {
        let engine = Arc::clone(&engine);
        std::thread::Builder::new()
            .name("sayso-stream".into())
            .spawn(move || {
                for request in stream_rx {
                    engine.handle(&request);
                }
            })
            .expect("cannot start the stream thread");
    }

    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
        let text = String::from_utf8_lossy(&line);
        if text.trim().is_empty() {
            continue;
        }
        let request = match Request::parse(text.trim()) {
            Ok(request) => request,
            Err(message) => {
                out.error(None, &message);
                continue;
            }
        };
        if request.kind == "shutdown" {
            engine.handle(&request);
            std::process::exit(0);
        }
        if Engine::is_stream(&request.kind) {
            if stream_tx.send(request).is_err() {
                break;
            }
            continue;
        }
        let engine = Arc::clone(&engine);
        let spawned = std::thread::Builder::new()
            .name(format!("sayso-{}", request.kind))
            .spawn(move || engine.handle(&request));
        if let Err(e) = spawned {
            out.error(None, &format!("cannot start a request thread: {e}"));
        }
    }
    // stdin closed: the app is gone.
    std::process::exit(0);
}
