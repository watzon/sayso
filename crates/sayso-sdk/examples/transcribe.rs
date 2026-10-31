//! A small program on the SDK: transcribe a file with Sayso when it runs, else with an
//! engine that you name.
//!
//! ```text
//! cargo run -p sayso-sdk --example transcribe -- talk.wav path/to/engine-folder
//! ```
//!
//! The engine folder is where you unpacked the release file of the engine. Local mode needs a
//! model on disk. With `--download <model>`, it downloads that model first, into the store of the user.

use sayso_sdk::protocol::{self, Capability, TranscribeFile};
use sayso_sdk::{App, Engine, Event, Mode, Options, Speech};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let download = args.iter().position(|arg| arg == "--download").map(|at| {
        args.remove(at);
        args.remove(at)
    });
    let [file, engine] = args.as_slice() else {
        eprintln!("usage: transcribe <file> <engine folder> [--download <model>]");
        return ExitCode::from(2);
    };
    match run(Path::new(file), PathBuf::from(engine), download.as_deref()) {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn run(file: &Path, engine: PathBuf, download: Option<&str>) -> Result<String, String> {
    let path = std::fs::canonicalize(file).map_err(|e| format!("cannot find {}: {e}", file.display()))?;
    let options = Options::new(Mode::Auto, App::new("Transcribe example"))
        .capabilities([Capability::ModelsRead, Capability::ModelsDownload, Capability::TranscribeFile])
        .engine(Engine::from_release(engine));
    let speech = Speech::connect(options).map_err(|e| e.to_string())?;
    eprintln!("mode: {:?}", speech.mode());
    if let Some(model) = download {
        speech.download_model(model, "To transcribe a file in this example").map_err(|e| e.to_string())?;
    }
    let job = speech.transcribe_file(TranscribeFile { path, ..Default::default() }).map_err(|e| e.to_string())?;
    for event in speech.events() {
        match event {
            Event::Message(protocol::Event::JobProgress { job: id, fraction }) if id == job => eprintln!("{:.0} %", fraction * 100.0),
            Event::Message(protocol::Event::JobDone { job: id, text }) if id == job => return Ok(text),
            Event::Message(protocol::Event::JobFailed { job: id, message, .. }) if id == job => return Err(message),
            Event::ModeChanged { mode, .. } => eprintln!("mode: {mode:?}"),
            _ => {}
        }
    }
    Err("the events of the SDK ended".into())
}
