//! Run the conformance suite against an engine executable.
//!
//! ```text
//! sayso-engine-conformance <engine> [--model <id> --engine-json <json>]
//!     [--models-dir <dir> --audio <wav> [--stream]]
//! ```
//!
//! `--model` and `--engine-json` add the model level: a catalog entry that is
//! not on disk. `--models-dir` and `--audio` add the ready level: the same
//! model, on disk in that folder. The exit code is 1 when a check fails.

use sayso_engine_conformance::{Model, Ready, Suite, run};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: sayso-engine-conformance <engine> [--model <id> --engine-json <json>] [--models-dir <dir> --audio <wav> [--stream]]";

fn main() -> ExitCode {
    match parse(std::env::args().skip(1).collect()) {
        Ok(suite) => {
            let report = run(&suite);
            println!("{report}");
            if report.passed() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        Err(why) => {
            eprintln!("{why}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn parse(args: Vec<String>) -> Result<Suite, String> {
    let mut args = args.into_iter();
    let engine = args.next().ok_or("no engine given")?;
    let mut suite = Suite::new(engine);
    let (mut id, mut engine_json, mut models_dir, mut audio, mut stream) = (None, None, None, None, false);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--model" => id = Some(value()?),
            "--engine-json" => engine_json = Some(value()?),
            "--models-dir" => models_dir = Some(PathBuf::from(value()?)),
            "--audio" => audio = Some(PathBuf::from(value()?)),
            "--stream" => stream = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let model = match (id, engine_json) {
        (Some(id), Some(json)) => {
            let engine = serde_json::from_str(&json).map_err(|e| format!("--engine-json is not JSON: {e}"))?;
            Some(Model { id, engine })
        }
        (None, None) => None,
        _ => return Err("--model and --engine-json go together".into()),
    };
    match (models_dir, audio, &model) {
        (Some(models_dir), Some(audio), Some(model)) => {
            suite.ready = Some(Ready { models_dir, model: model.clone(), audio, stream });
        }
        (None, None, _) => {}
        _ => return Err("--models-dir and --audio go together, and need --model".into()),
    }
    suite.model = model;
    Ok(suite)
}
