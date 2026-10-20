//! Accuracy and speed of every local model, with the real sidecar. Ignored by default:
//! it downloads every model and runs for hours.
//!
//! ```text
//! uv run scripts/benchmark/prepare.py                  # the clips and the manifest
//! (cd native/macos/SaysoEngine && swift build -c release)
//! SAYSO_BENCH_MODELS=all cargo test --release -p sayso-engine-client --test benchmark -- --ignored --nocapture
//! uv run scripts/benchmark/score.py                    # WER and speed per model
//! ```
//!
//! `SAYSO_BENCH_MODELS` is "all" or a list of model ids. `SAYSO_BENCH_DIR` is the folder of
//! `prepare.py` (default `~/.cache/sayso-bench`). Each model writes `results/<id>.jsonl`
//! there, one line per clip. A second run skips the clips that have a line, so a run that
//! stops can continue. The models go to `models/` there, and each is deleted after its run.

use sayso_core::models::{self, ModelInfo};
use sayso_core::stt::{EngineEvent, ModelStatus, SessionOptions};
use sayso_engine_client::EngineClient;
use sayso_platform::SttBackend;
use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn bench_dir() -> PathBuf {
    match std::env::var("SAYSO_BENCH_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => PathBuf::from(std::env::var("HOME").unwrap()).join(".cache/sayso-bench"),
    }
}

/// Samples of a 16 kHz mono 16-bit WAV file, as floats.
fn read_wav(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap();
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        if &bytes[at..at + 4] == b"data" {
            let end = (at + 8 + size).min(bytes.len());
            let (pairs, _) = bytes[at + 8..end].as_chunks::<2>();
            return pairs.iter().map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0).collect();
        }
        at += 8 + size + (size & 1);
    }
    panic!("no data chunk in {}", path.display());
}

struct Clip {
    id: String,
    path: PathBuf,
}

fn manifest(dir: &Path) -> Vec<Clip> {
    let file = std::fs::File::open(dir.join("manifest.jsonl")).expect("run scripts/benchmark/prepare.py first");
    std::io::BufReader::new(file)
        .lines()
        .map(|line| {
            let v: serde_json::Value = serde_json::from_str(&line.unwrap()).unwrap();
            Clip { id: v["id"].as_str().unwrap().into(), path: v["path"].as_str().unwrap().into() }
        })
        .collect()
}

/// The models to measure: local models with a final pass that understand English.
fn english_final_models() -> Vec<ModelInfo> {
    models::catalog().iter().filter(|m| !m.is_remote() && m.final_pass && m.languages.contains(&"en")).cloned().collect()
}

fn download(client: &EngineClient, events: &crossbeam_channel::Receiver<EngineEvent>, info: &ModelInfo) {
    let started = Instant::now();
    client.download(&info.id).unwrap();
    loop {
        match events.recv_timeout(Duration::from_secs(900)).expect("download made progress") {
            EngineEvent::ModelStatus { model, status } if model == info.id => match status {
                ModelStatus::Downloaded => break,
                ModelStatus::Failed { message, .. } => panic!("{}: download failed: {message}", info.id),
                _ => {}
            },
            _ => {}
        }
    }
    println!("  download: {} s", started.elapsed().as_secs());
}

fn run_model(client: &EngineClient, events: &crossbeam_channel::Receiver<EngineEvent>, info: &ModelInfo, clips: &[Clip], results: &Path) {
    let out_path = results.join(format!("{}.jsonl", info.id));
    let done: HashSet<String> = std::fs::read_to_string(&out_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v["id"].as_str().map(String::from))
        .collect();
    let todo: Vec<&Clip> = clips.iter().filter(|c| !done.contains(&c.id)).collect();
    println!("== {} ({}): {} of {} clips to go", info.id, info.name, todo.len(), clips.len());
    if todo.is_empty() {
        return;
    }
    if !client.status(&info.id).is_on_disk() {
        download(client, events, info);
    }
    let t = Instant::now();
    client.load(&info.id).unwrap();
    println!("  load: {} ms", t.elapsed().as_millis());

    let options = SessionOptions { final_model: info.id.clone(), preview_model: None, language: "en".into(), vocabulary: vec![] };
    // The first call can compile or warm the model. It is not timed.
    let _ = client.transcribe(&read_wav(&todo[0].path), &options);

    let mut out = std::fs::OpenOptions::new().create(true).append(true).open(&out_path).unwrap();
    let started = Instant::now();
    for (n, clip) in todo.iter().enumerate() {
        let samples = read_wav(&clip.path);
        let seconds = samples.len() as f64 / 16_000.0;
        let line = match client.transcribe(&samples, &options) {
            Ok(r) => serde_json::json!({"id": clip.id, "text": r.text, "elapsed_ms": r.elapsed_ms, "seconds": seconds}),
            Err(e) => serde_json::json!({"id": clip.id, "error": e.to_string(), "seconds": seconds}),
        };
        writeln!(out, "{line}").unwrap();
        if (n + 1) % 100 == 0 {
            println!("  {} clips, {} s", n + 1, started.elapsed().as_secs());
        }
    }
    println!("  done in {} s", started.elapsed().as_secs());
    // macOS owns the files of Apple Speech, so it cannot be deleted.
    if let Err(e) = client.delete(&info.id) {
        println!("  not deleted: {e}");
    }
}

#[test]
#[ignore = "needs the real sidecar, downloads every model, and runs for hours"]
fn benchmark_every_model() {
    let Ok(wanted) = std::env::var("SAYSO_BENCH_MODELS") else {
        println!("set SAYSO_BENCH_MODELS to a list of model ids, or to \"all\"");
        return;
    };
    let dir = bench_dir();
    let clips = manifest(&dir);
    let results = dir.join("results");
    let models_dir = dir.join("models");
    std::fs::create_dir_all(&results).unwrap();
    std::fs::create_dir_all(&models_dir).unwrap();

    let engine = EngineClient::default_engine_path();
    assert!(engine.exists(), "build the sidecar first: {}", engine.display());
    let cache = tempfile::TempDir::new().unwrap();
    let client = EngineClient::spawn(engine, models_dir, cache.path().to_path_buf()).unwrap();
    let events = client.subscribe();

    let candidates = english_final_models();
    let chosen: Vec<ModelInfo> = if wanted == "all" {
        candidates
    } else {
        wanted.split(',').map(|id| candidates.iter().find(|m| m.id.as_str() == id.trim()).unwrap_or_else(|| panic!("not an English final model: {id}")).clone()).collect()
    };
    for info in &chosen {
        run_model(&client, &events, info, &clips, &results);
    }
}

/// The live preview of one clip: stream all of it, end the stream, and take the text of
/// the last partial. Partials stop when the stream is done.
fn stream_clip(client: &EngineClient, events: &crossbeam_channel::Receiver<EngineEvent>, session: u64, options: &SessionOptions, samples: &[f32]) -> String {
    while events.try_recv().is_ok() {}
    client.start_stream(session, options).unwrap();
    for chunk in samples.chunks(2560) {
        client.push_audio(session, chunk).unwrap();
    }
    client.end_stream(session);
    let mut text = String::new();
    while let Ok(event) = events.recv_timeout(Duration::from_millis(1500)) {
        if let EngineEvent::Partial { session: s, committed, tentative } = event
            && s == session
        {
            text = [committed, tentative].join(" ").trim().to_string();
        }
    }
    text
}

/// The accuracy of the live preview, for a model whose preview runs another decoder
/// than its final pass (Parakeet Unified EN). Writes `results/<id>.preview.jsonl`.
///
/// ```text
/// SAYSO_BENCH_PREVIEW=parakeet-unified-en cargo test --release -p sayso-engine-client --test benchmark benchmark_preview -- --ignored --nocapture
/// ```
#[test]
#[ignore = "needs the real sidecar and runs for about an hour"]
fn benchmark_preview() {
    let Ok(id) = std::env::var("SAYSO_BENCH_PREVIEW") else {
        println!("set SAYSO_BENCH_PREVIEW to a streaming model id");
        return;
    };
    let dir = bench_dir();
    let clips = manifest(&dir);
    let models_dir = dir.join("models");
    std::fs::create_dir_all(&models_dir).unwrap();
    let info = models::catalog().iter().find(|m| m.id.as_str() == id && m.live_preview).expect("a streaming model");
    let engine = EngineClient::default_engine_path();
    let cache = tempfile::TempDir::new().unwrap();
    let client = EngineClient::spawn(engine, models_dir, cache.path().to_path_buf()).unwrap();
    let events = client.subscribe();

    let out_path = dir.join("results").join(format!("{id}.preview.jsonl"));
    let done: HashSet<String> = std::fs::read_to_string(&out_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v["id"].as_str().map(String::from))
        .collect();
    if !client.status(&info.id).is_on_disk() {
        download(&client, &events, info);
    }
    client.load(&info.id).unwrap();
    let options = SessionOptions { final_model: info.id.clone(), preview_model: Some(info.id.clone()), language: "en".into(), vocabulary: vec![] };
    let mut out = std::fs::OpenOptions::new().create(true).append(true).open(&out_path).unwrap();
    let started = Instant::now();
    for (n, clip) in clips.iter().filter(|c| !done.contains(&c.id)).enumerate() {
        let samples = read_wav(&clip.path);
        let t = Instant::now();
        let text = stream_clip(&client, &events, n as u64 + 1, &options, &samples);
        let line = serde_json::json!({"id": clip.id, "text": text, "elapsed_ms": t.elapsed().as_millis() as u64, "seconds": samples.len() as f64 / 16_000.0});
        writeln!(out, "{line}").unwrap();
        if (n + 1) % 100 == 0 {
            println!("  {} clips, {} s", n + 1, started.elapsed().as_secs());
        }
    }
    client.delete(&info.id).unwrap();
}
