//! Download, load, and run catalog models with the real client and the real
//! engine. Ignored by default, because it downloads models (57 MB to 563 MB each):
//!
//! ```text
//! cargo build -p sayso-engine --release
//! SAYSO_ENGINE_PATH=target/release/sayso-engine SAYSO_IT_AUDIO_DIR=<clips> \
//!   SAYSO_REAL_MODELS=kroko-en,moonshine-base-en \
//!   cargo test -p sayso-engine --test real_models -- --ignored --nocapture
//! ```
//!
//! On Windows (PowerShell), the default models are the Windows ones:
//!
//! ```text
//! cargo build -p sayso-engine --release
//! $env:SAYSO_ENGINE_PATH = "target\release\sayso-engine.exe"
//! $env:SAYSO_IT_AUDIO_DIR = "<clips>"
//! cargo test -p sayso-engine --release --test real_models -- --ignored --nocapture
//! ```
//!
//! The clips come from `SAYSO_IT_AUDIO_DIR`: 16 kHz mono WAV files `short.wav`
//! with the word "dictation" and `long30.wav` (about 30 s) with "deployment".
//!
//! `SAYSO_REAL_MODELS=all` runs every model. The models stay in
//! `SAYSO_IT_MODELS_DIR` (default `~/.cache/sayso-engine-it-models`), so a
//! second run downloads nothing. Without `SAYSO_ENGINE_PATH`, the test uses
//! the debug build of the engine.

// The client reads the catalog of the system. This engine runs it on Linux and Windows.
#![cfg(any(target_os = "linux", windows))]

use crossbeam_channel::Receiver;
use sayso_core::models::{self, EngineKind, ModelInfo};
use sayso_core::stt::{EngineEvent, ModelStatus, SessionOptions};
use sayso_engine_client::EngineClient;
use sayso_platform::SttBackend;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Samples of a 16 kHz mono 16-bit WAV file, as floats.
fn read_wav(path: &Path) -> Vec<f32> {
    sayso_engine::audio::read_wav(path).unwrap()
}

/// The English test clips, each with a word every final pass must hear in it.
const CLIPS: [(&str, &str); 2] = [("short.wav", "dictation"), ("long30.wav", "deployment")];

fn wait_for_download(client: &EngineClient, events: &Receiver<EngineEvent>, info: &ModelInfo) {
    let started = Instant::now();
    let mut last_print = Instant::now();
    loop {
        match events
            .recv_timeout(Duration::from_secs(600))
            .expect("download made progress")
        {
            EngineEvent::ModelStatus { model, status } if model == info.id => match status {
                ModelStatus::Downloading { fraction, .. } => {
                    if last_print.elapsed() > Duration::from_secs(10) {
                        last_print = Instant::now();
                        println!("  downloading {:.0}%", fraction * 100.0);
                    }
                }
                ModelStatus::Downloaded => break,
                ModelStatus::Failed { message, .. } => {
                    panic!("{}: download failed: {message}", info.id)
                }
                _ => {}
            },
            _ => {}
        }
    }
    println!("  download: {} s", started.elapsed().as_secs());
    assert_eq!(client.status(&info.id), ModelStatus::Downloaded);
}

fn ensure_loaded(client: &EngineClient, events: &Receiver<EngineEvent>, info: &ModelInfo) {
    if !client.status(&info.id).is_on_disk() {
        client.download(&info.id).unwrap();
        wait_for_download(client, events, info);
    }
    let t = Instant::now();
    client.load(&info.id).unwrap();
    println!("  load {}: {} ms", info.id, t.elapsed().as_millis());
    assert_eq!(client.status(&info.id), ModelStatus::Ready);
}

fn run_model(client: &EngineClient, events: &Receiver<EngineEvent>, info: &ModelInfo, audio: &Path) {
    println!("== {} ({})", info.id, info.name);
    ensure_loaded(client, events, info);
    let preview = if info.live_preview {
        info.clone()
    } else {
        let fallback = models::find(&models::preview_fallback()).unwrap();
        ensure_loaded(client, events, &fallback);
        fallback
    };
    // A redecode preview decodes about every 600 ms, so it gets the audio in real time.
    let redecode = matches!(preview.engine, EngineKind::Sherpa { redecode_preview: true, .. });
    let (pause_ms, min_partials) = if redecode { (160, 3) } else { (40, 5) };
    let options = SessionOptions {
        final_model: info.id.clone(),
        preview_model: Some(preview.id.clone()),
        language: "en".into(),
        vocabulary: vec!["Sayso".into()],
    };

    if info.final_pass {
        for (clip, word) in CLIPS {
            let samples = read_wav(&audio.join(clip));
            let t = Instant::now();
            let transcript = client.transcribe(&samples, &options).unwrap();
            println!(
                "  {clip} ({:.1} s audio): {} ms in engine, {} ms round trip\n    {:?}",
                samples.len() as f64 / 16_000.0,
                transcript.elapsed_ms,
                t.elapsed().as_millis(),
                transcript.text
            );
            assert!(
                transcript.text.to_lowercase().contains(word),
                "{}: {clip}: {}",
                info.id,
                transcript.text
            );
        }
    }

    // Live preview: the short clip in 160 ms chunks, at four times real time (a redecode
    // preview: in real time).
    let samples = read_wav(&audio.join("short.wav"));
    while events.try_recv().is_ok() {}
    let session = 1;
    client.start_stream(session, &options).unwrap();
    let t = Instant::now();
    for chunk in samples.chunks(2560) {
        client.push_audio(session, chunk).unwrap();
        std::thread::sleep(Duration::from_millis(pause_ms));
    }
    client.end_stream(session);
    let mut partials = 0;
    let mut first_ms = None;
    let mut last = String::new();
    // The stream is over when no event comes for 3 seconds.
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        match events.recv_timeout(Duration::from_secs(3)) {
            Ok(EngineEvent::Partial {
                session: s,
                committed,
                tentative,
            }) if s == session => {
                partials += 1;
                first_ms.get_or_insert(t.elapsed().as_millis());
                last = format!("{committed} | {tentative}");
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    println!(
        "  stream: {partials} partials, first after {} ms\n    {last:?}",
        first_ms.unwrap_or_default()
    );
    assert!(partials >= min_partials, "{}: {partials} partials", info.id);
    assert!(
        last.to_lowercase().contains("dictation"),
        "{}: {last}",
        info.id
    );
}

#[test]
#[ignore = "downloads real models"]
fn real_models_transcribe_and_stream() {
    let audio = PathBuf::from(std::env::var_os("SAYSO_IT_AUDIO_DIR").expect("SAYSO_IT_AUDIO_DIR is set"));
    let engine = std::env::var_os("SAYSO_ENGINE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_sayso-engine")));
    let models_dir = std::env::var_os("SAYSO_IT_MODELS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
            PathBuf::from(home.expect("HOME or USERPROFILE is set")).join(".cache/sayso-engine-it-models")
        });
    // On Windows: Parakeet v3 with its redecode preview, and whisper.cpp.
    let default_models = if cfg!(windows) { "parakeet-tdt-v3,whisper-base" } else { "kroko-en,moonshine-base-en" };
    let wanted = std::env::var("SAYSO_REAL_MODELS").unwrap_or_else(|_| default_models.into());
    let cache = tempfile::tempdir().unwrap();
    println!(
        "engine {}, models in {}",
        engine.display(),
        models_dir.display()
    );
    let client = EngineClient::spawn(engine, models_dir, cache.path().to_path_buf()).unwrap();
    let events = client.subscribe();

    let selected: Vec<ModelInfo> = client
        .catalog()
        .into_iter()
        .filter(|m| wanted == "all" || wanted.split(',').any(|w| w.trim() == m.id.as_str()))
        .collect();
    assert!(!selected.is_empty(), "no catalog model matches {wanted}");
    for info in &selected {
        run_model(&client, &events, info, &audio);
    }
    assert_eq!(client.dropped_audio_frames(), 0);
}
