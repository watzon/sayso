//! Segments, VAD, and diarization with the real Swift sidecar and models that are already
//! on disk. Ignored by default:
//!
//! ```text
//! (cd native/macos/SaysoEngine && swift build -c release)
//! SAYSO_CONF_MODELS=/tmp/conf/swift-models SAYSO_CONF_AUDIO=/tmp/conf/audio/short.wav \
//!   cargo test -p sayso-engine-client --test real_timing -- --ignored --nocapture
//! ```
//!
//! The folder needs `parakeet-unified-en-0.6b`, `silero-vad`, and `speaker-diarization`. The
//! engine is found as in the app (`EngineClient::default_engine_path`, so `SAYSO_ENGINE_PATH` works).
#![cfg(target_os = "macos")]

use sayso_core::models::{self, ModelId};
use sayso_core::stt::{EngineEvent, SessionOptions};
use sayso_engine_client::EngineClient;
use sayso_platform::SttBackend;
use std::path::{Path, PathBuf};
use std::time::Duration;

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

#[test]
#[ignore = "needs the real sidecar and models on disk"]
fn real_sidecar_gives_segments_speech_and_speakers() {
    let models_dir = PathBuf::from(std::env::var_os("SAYSO_CONF_MODELS").expect("set SAYSO_CONF_MODELS"));
    let audio = PathBuf::from(std::env::var_os("SAYSO_CONF_AUDIO").expect("set SAYSO_CONF_AUDIO"));
    let engine = EngineClient::default_engine_path();
    assert!(engine.exists(), "build the sidecar first: {}", engine.display());
    let cache = tempfile::TempDir::new().unwrap();
    let client = EngineClient::spawn(engine, models_dir, cache.path().to_path_buf()).unwrap();
    println!("features: {:?}", client.features());
    for feature in ["segments", "vad", "diarization"] {
        assert!(client.features().iter().any(|f| f == feature), "the engine lacks {feature}");
    }
    let samples = read_wav(&audio);
    println!("audio: {:.2} s", samples.len() as f64 / 16_000.0);

    let speech_model = models::default_model();
    let vad = ModelId::new("silero-vad");
    let diarization = ModelId::new("speaker-diarization");
    for model in [&speech_model, &vad, &diarization] {
        client.load(model).unwrap_or_else(|e| panic!("cannot load {model}: {e}"));
    }

    let options = SessionOptions {
        final_model: speech_model,
        preview_model: None,
        language: "en".into(),
        vocabulary: Vec::new(),
    };
    let transcript = client.transcribe_segments(&samples, &options).unwrap();
    println!("text: {:?} ({} ms)", transcript.text, transcript.elapsed_ms);
    for segment in &transcript.segments {
        println!("  segment {}..{} {:?} ({} words)", segment.start_ms, segment.end_ms, segment.text, segment.words.len());
    }
    assert!(!transcript.segments.is_empty());
    let mut last_end = 0;
    for segment in &transcript.segments {
        assert!(segment.start_ms >= last_end && segment.end_ms >= segment.start_ms, "{segment:?}");
        last_end = segment.end_ms;
    }

    let parts = client.detect_speech(&vad, &samples).unwrap();
    println!("speech: {parts:?}");
    assert!(!parts.is_empty());

    let found = client.diarize(&diarization, &samples, None).unwrap();
    println!("diarization: {} speakers, {:?}", found.speakers, found.turns);
    assert!(found.speakers >= 1 && !found.turns.is_empty());

    // The same audio through a live VAD session: 20 ms frames.
    let events = client.subscribe();
    client.start_vad(1, &vad).unwrap();
    for frame in samples.chunks(320) {
        client.push_audio(1, frame).unwrap();
    }
    client.end_stream(1);
    let mut live = Vec::new();
    while let Ok(event) = events.recv_timeout(Duration::from_secs(5)) {
        match event {
            EngineEvent::SpeechStart { session: 1, start_ms } => live.push((start_ms, None)),
            EngineEvent::SpeechEnd { session: 1, start_ms, end_ms } => live.push((start_ms, Some(end_ms))),
            _ => {}
        }
    }
    println!("live VAD: {live:?}");
    assert!(live.iter().any(|(_, end)| end.is_some()), "the session reported no speech_end");
}
