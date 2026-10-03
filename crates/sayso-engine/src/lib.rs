//! The speech engine for Linux and Windows.
//!
//! `sayso-engine` is a sidecar process. It speaks the same NDJSON protocol (v1)
//! as the Swift engine on macOS, so `sayso-engine-client` works with it
//! unchanged. It runs sherpa-onnx models on the CPU.
//!
//! Modules:
//! - [`protocol`]: request parsing and the output writer.
//! - [`spec`]: the `engine` field of a request (recipe and archives).
//! - [`store`]: the model folders on disk.
//! - [`download`]: download and extract a model archive.
//! - [`engine`]: the request handlers and the stream worker.
//! - [`recognizer`]: the traits that hide sherpa-onnx, so tests can use a fake.
//! - [`sherpa`]: the sherpa-onnx implementation of the recognizer traits.
//!
//! On macOS this crate is empty. Sayso uses the Swift engine there.

#![cfg(not(target_os = "macos"))]

pub mod audio;
pub mod download;
pub mod engine;
pub mod layout;
pub mod partials;
pub mod protocol;
pub mod recognizer;
pub mod sherpa;
pub mod spec;
pub mod store;

use engine::{Config, Engine};
use protocol::Output;
use std::process::ExitCode;
use std::sync::Arc;

/// Run the engine on stdin and stdout until stdin closes or a `shutdown` request.
pub fn run() -> ExitCode {
    let out = Arc::new(Output::protocol_stdout());
    let Some(models_dir) = std::env::var_os("SAYSO_MODELS_DIR").filter(|v| !v.is_empty()) else {
        out.error("SAYSO_MODELS_DIR is not set", None);
        return ExitCode::from(2);
    };
    let models_dir = std::path::PathBuf::from(models_dir);
    if let Err(e) = std::fs::create_dir_all(&models_dir) {
        out.error(
            &format!("cannot create {}: {e}", models_dir.display()),
            None,
        );
        return ExitCode::from(2);
    }
    let config = Config::from_env(models_dir.clone());
    let loader = Arc::new(sherpa::SherpaLoader::new());
    let engine = Arc::new(Engine::new(config, Arc::clone(&out), loader));
    out.log(
        "info",
        &format!(
            "sayso-engine {} ready, models in {}, pid {}",
            env!("CARGO_PKG_VERSION"),
            models_dir.display(),
            std::process::id()
        ),
    );
    engine::serve(std::io::stdin().lock(), engine);
    ExitCode::SUCCESS
}
