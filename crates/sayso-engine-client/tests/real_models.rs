//! Download, load, and run any catalog model with the real Swift sidecar. Ignored by
//! default, because it downloads the models (220 MB to 2.2 GB each):
//!
//! ```text
//! (cd native/macos/SaysoEngine && swift build -c release)
//! SAYSO_REAL_MODELS=parakeet-tdt-ctc-110m,whisper-tiny \
//!   cargo test -p sayso-engine-client --test real_models -- --ignored --nocapture
//! ```
//!
//! `SAYSO_REAL_MODELS=all` runs every model. The models go to `target/engine-it-models`
//! and stay there, so a second run downloads nothing. `SAYSO_REAL_DELETE=1` deletes
//! each model after its run.

use sayso_core::models::{self, ModelInfo};
use sayso_core::stt::{EngineEvent, ModelStatus, SessionOptions};
use sayso_engine_client::EngineClient;
use sayso_platform::SttBackend;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// Samples of a 16 kHz mono 16-bit WAV file, as floats.
fn read_wav(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[0..4], b"RIFF");
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

/// The English test clips, each with a word every model must hear in it.
const CLIPS: [(&str, &str); 2] = [("short.wav", "dictation"), ("long30.wav", "deployment")];

fn heard(text: &str, word: &str) -> bool {
    text.to_lowercase().contains(word)
}

fn wait_for_download(client: &EngineClient, events: &crossbeam_channel::Receiver<EngineEvent>, info: &ModelInfo) {
    let started = Instant::now();
    let mut last_print = Instant::now();
    loop {
        match events.recv_timeout(Duration::from_secs(600)).expect("download made progress") {
            EngineEvent::ModelStatus { model, status } if model == info.id => match status {
                ModelStatus::Downloading { fraction, .. } => {
                    if last_print.elapsed() > Duration::from_secs(10) {
                        last_print = Instant::now();
                        println!("  downloading {:.0}%", fraction * 100.0);
                    }
                }
                ModelStatus::Downloaded => break,
                ModelStatus::Failed { message } => panic!("{}: download failed: {message}", info.id),
                _ => {}
            },
            _ => {}
        }
    }
    println!("  download: {} s", started.elapsed().as_secs());
    assert_eq!(client.status(&info.id), ModelStatus::Downloaded);
}

fn run_model(client: &EngineClient, events: &crossbeam_channel::Receiver<EngineEvent>, info: &ModelInfo, root: &Path) {
    println!("== {} ({})", info.id, info.name);
    if !client.status(&info.id).is_on_disk() {
        client.download(&info.id).unwrap();
        wait_for_download(client, events, info);
    }

    let t = Instant::now();
    client.load(&info.id).unwrap();
    println!("  load: {} ms", t.elapsed().as_millis());
    assert_eq!(client.status(&info.id), ModelStatus::Ready);

    // A model that cannot hear English (Japanese, Mandarin) must still return without
    // an error. Its text is printed and not checked.
    let english = info.languages.iter().any(|l| l == "en");
    let options = SessionOptions {
        final_model: info.id.clone(),
        preview_model: info.live_preview.then(|| info.id.clone()),
        language: "en".into(),
        vocabulary: vec![],
    };

    if info.final_pass {
        for (clip, word) in CLIPS {
            let samples = read_wav(&root.join("spikes/engine/audio").join(clip));
            let seconds = samples.len() as f64 / 16_000.0;
            let result = client.transcribe(&samples, &options).unwrap();
            println!(
                "  {clip}: {seconds:.1} s of audio in {} ms ({:.0}x real time)\n    {:?}",
                result.elapsed_ms,
                seconds * 1000.0 / result.elapsed_ms.max(1) as f64,
                result.text
            );
            if english {
                assert!(heard(&result.text, word), "{}: {clip}: {:?}", info.id, result.text);
            }
        }
    }

    if info.live_preview {
        let samples = read_wav(&root.join("spikes/engine/audio/short.wav"));
        let session = 7;
        client.start_stream(session, &options).unwrap();
        for chunk in samples.chunks(2560) {
            client.push_audio(session, chunk).unwrap();
        }
        client.end_stream(session);
        let mut last = String::new();
        let mut partials = 0;
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            match events.recv_timeout(Duration::from_secs(4)) {
                Ok(EngineEvent::Partial { session: s, committed, .. }) if s == session => {
                    partials += 1;
                    last = committed;
                }
                Ok(_) => {}
                Err(_) => break, // No partial for 4 s: the stream is done.
            }
        }
        println!("  stream: {partials} partials\n    {last:?}");
        assert!(partials > 0, "{}: no partial arrived", info.id);
        if english {
            assert!(heard(&last, CLIPS[0].1), "{}: stream: {last:?}", info.id);
        }
        // A streaming model must do a final pass right after its own stream.
        if info.final_pass {
            let result = client.transcribe(&samples, &options).unwrap();
            assert!(!english || heard(&result.text, CLIPS[0].1), "{}: final after stream: {:?}", info.id, result.text);
        }
    }

    if std::env::var("SAYSO_REAL_DELETE").is_ok_and(|v| v == "1") {
        client.delete(&info.id).unwrap();
        assert_eq!(client.status(&info.id), ModelStatus::NotDownloaded);
        println!("  deleted");
    }
}

#[test]
#[ignore = "needs the real sidecar and downloads models"]
fn real_models_download_load_and_transcribe() {
    let Ok(wanted) = std::env::var("SAYSO_REAL_MODELS") else {
        println!("set SAYSO_REAL_MODELS to a list of model ids, or to \"all\"");
        return;
    };
    let root = workspace();
    let engine = EngineClient::default_engine_path();
    assert!(engine.exists(), "build the sidecar first: {}", engine.display());
    let models_dir = root.join("target/engine-it-models");
    std::fs::create_dir_all(&models_dir).unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let client = EngineClient::spawn(engine, models_dir, cache.path().to_path_buf()).unwrap();
    let events = client.subscribe();

    let catalog = models::catalog();
    let chosen: Vec<ModelInfo> = if wanted == "all" {
        catalog
    } else {
        wanted
            .split(',')
            .map(|id| {
                let id = id.trim();
                catalog.iter().find(|m| m.id.as_str() == id).unwrap_or_else(|| panic!("unknown model {id}")).clone()
            })
            .collect()
    };
    for info in &chosen {
        run_model(&client, &events, info, &root);
    }
}
