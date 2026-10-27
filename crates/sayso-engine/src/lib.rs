//! The speech engine for Linux and Windows.
//!
//! `sayso-engine` is a sidecar process. It speaks the same NDJSON protocol (v1)
//! as the Swift engine on macOS, so `sayso-engine-client` works with it
//! unchanged. It runs sherpa-onnx models on the CPU (speech, VAD, and diarization),
//! `parakeet_cpp` models on a GPU with parakeet.cpp (Linux), and `whisper_cpp`
//! models on the CPU with whisper.cpp (Windows).
//!
//! Modules:
//! - [`protocol`]: request parsing and the output writer.
//! - [`spec`]: the `engine` field of a request (recipe and archives).
//! - [`store`]: the model folders on disk.
//! - [`download`]: download and extract a model archive, or download one file.
//! - [`language`]: the language that a model finds in the speech.
//! - [`parakeet`]: the final pass of a `parakeet_cpp` model, on parakeet.cpp.
//! - [`whisper_cpp`]: the final pass of a `whisper_cpp` model, on whisper.cpp.
//! - [`vad`]: the rules of voice activity detection, on top of a detector.
//! - [`diarization`]: the turns of the protocol, made from the turns of the model.
//! - [`redecode`]: a live preview that decodes the recording again with the final pass.
//! - [`engine`]: the request handlers and the stream worker.
//! - [`recognizer`]: the traits that hide sherpa-onnx, so tests can use a fake.
//! - [`sherpa`]: the sherpa-onnx implementation of the recognizer traits.
//! - [`sherpa_c`]: Whisper and SenseVoice on the C API of sherpa-onnx, for the language and the segment times.
//!
//! On macOS this crate is empty. Sayso uses the Swift engine there.

#![cfg(not(target_os = "macos"))]

// parakeet.cpp and whisper.cpp each link their own ggml, and two copies in one binary clash.
#[cfg(all(target_os = "linux", feature = "parakeet", feature = "whisper-cpp"))]
compile_error!("the features `parakeet` and `whisper-cpp` link two copies of ggml; use `--no-default-features --features whisper-cpp`");

pub mod audio;
pub mod diarization;
pub mod download;
pub mod engine;
pub mod language;
pub mod layout;
pub mod parakeet;
pub mod partials;
pub mod protocol;
pub mod recognizer;
pub mod redecode;
pub mod sherpa;
pub mod sherpa_c;
pub mod spec;
pub mod store;
pub mod timing;
pub mod vad;
pub mod whisper_cpp;

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
    // A load, a final pass or a download can still run on another thread, and a normal exit
    // runs the C++ global destructors under it: onnxruntime's CUDA provider then crashes.
    // Those threads end unanswered either way, and a download renames only complete files
    // into place, so the process ends here and skips the destructors.
    #[cfg(all(unix, not(target_os = "macos")))]
    // SAFETY: _exit has no preconditions.
    unsafe {
        libc::_exit(0)
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    ExitCode::SUCCESS
}
