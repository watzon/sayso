//! The SDK against an engine with real models. This test is ignored, because it needs files that the
//! repository does not have. Run it with:
//!
//! ```text
//! cargo test -p sayso-engine-sdk --test models -- --ignored --nocapture
//! ```
//!
//! - `SAYSO_SDK_MODELS`: a models folder with `parakeet-unified-en`, `silero-vad`, and
//!   `speaker-diarization` on disk (default `/tmp/conf/swift-models`, the folder of the Swift engine).
//! - `SAYSO_SDK_AUDIO`: a WAV file with speech, 16 kHz mono 16-bit (default `/tmp/conf/audio/short.wav`).
//! - `SAYSO_ENGINE`: the engine program (see `common`).

mod common;

use common::engine_program;
use sayso_engine_sdk::protocol::ModelState;
use sayso_engine_sdk::protocol::engine::Event as EngineEvent;
use sayso_engine_sdk::{Engine, EngineOptions, Event, TranscribeOptions};
use std::path::PathBuf;

const SPEECH: &str = "parakeet-unified-en";
const VAD: &str = "silero-vad";
const DIARIZATION: &str = "speaker-diarization";

fn env_path(name: &str, default: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_else(|| default.into()))
}

/// The 16-bit samples of a 16 kHz mono WAV file: the data chunk, after any other chunks.
fn read_samples(path: &std::path::Path) -> Vec<i16> {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[0..4], b"RIFF");
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        if &bytes[at..at + 4] == b"data" {
            let end = (at + 8 + size).min(bytes.len());
            return bytes[at + 8..end].as_chunks::<2>().0.iter().map(|&pair| i16::from_le_bytes(pair)).collect();
        }
        at += 8 + size + size % 2;
    }
    panic!("{} has no data chunk", path.display());
}

#[test]
#[ignore = "needs models and audio, see the docs of this file"]
fn an_engine_with_real_models_loads_transcribes_detects_speech_and_streams() {
    let models = env_path("SAYSO_SDK_MODELS", "/tmp/conf/swift-models");
    let audio = env_path("SAYSO_SDK_AUDIO", "/tmp/conf/audio/short.wav");
    assert!(models.is_dir() && audio.is_file(), "missing {} or {}", models.display(), audio.display());
    let samples = read_samples(&audio);
    let engine = Engine::start(EngineOptions::new(engine_program(), &models)).unwrap();
    println!("engine {} with {:?}", engine.version(), engine.features());
    let events = engine.events();

    // The models are on disk.
    let states = engine.check_models(&[SPEECH.into(), VAD.into(), DIARIZATION.into()]).unwrap();
    for state in &states {
        assert_eq!(state.state, ModelState::Downloaded, "{} is not on disk in {}", state.model, models.display());
    }

    // The final pass, with segments and the timestamps of the engine.
    let loaded = engine.load(SPEECH).unwrap();
    assert!(loaded.load_ms.is_some());
    let transcript = engine.transcribe(SPEECH, &audio, &TranscribeOptions::default().with_segments()).unwrap();
    println!("transcript in {} ms: {}", transcript.elapsed_ms, transcript.text);
    assert!(!transcript.text.trim().is_empty());
    let segments = transcript.segments.as_ref().expect("segments");
    assert!(!segments.is_empty());
    assert!(segments.windows(2).all(|pair| pair[0].end_ms <= pair[1].start_ms), "in order, without overlaps: {segments:?}");
    // The same audio from memory gives the same text through the temporary file.
    let again = engine.transcribe_i16(SPEECH, &samples, &TranscribeOptions::default()).unwrap();
    assert_eq!(again.text, transcript.text);
    assert!(again.segments.is_none(), "no segments without the option");

    // A stream: chunks of 160 ms, then the last text.
    let session = engine.new_session();
    while events.try_recv().is_ok() {}
    engine.stream_start(session, SPEECH, Some("en")).unwrap();
    for chunk in samples.chunks(2560) {
        engine.stream_audio_i16(session, chunk).unwrap();
    }
    let ended = engine.stream_end(session).unwrap();
    println!("stream text: {:?}", ended.text);
    assert_eq!(ended.session, session);
    assert!(!ended.text.unwrap_or_default().trim().is_empty());
    let partials = events.try_iter().filter(|e| matches!(e, Event::Engine { event: EngineEvent::Partial(p), .. } if p.session == session)).count();
    assert!(partials > 0, "partial events on the channel");

    // VAD on the file, and on live audio.
    engine.load(VAD).unwrap();
    let speech = engine.vad(VAD, &audio).unwrap();
    println!("speech: {speech:?}");
    assert!(!speech.is_empty());
    assert!(speech.windows(2).all(|pair| pair[0].end_ms <= pair[1].start_ms));
    let vad_session = engine.new_session();
    while events.try_recv().is_ok() {}
    engine.vad_start(vad_session, VAD).unwrap();
    for chunk in samples.chunks(2560) {
        engine.stream_audio_i16(vad_session, chunk).unwrap();
    }
    engine.stream_end(vad_session).unwrap();
    let (mut starts, mut ends) = (0, 0);
    for event in events.try_iter() {
        match event {
            Event::Engine { event: EngineEvent::SpeechStart(e), .. } if e.session == vad_session => starts += 1,
            Event::Engine { event: EngineEvent::SpeechEnd(e), .. } if e.session == vad_session => ends += 1,
            _ => {}
        }
    }
    assert!(starts > 0 && starts == ends, "{starts} speech_start and {ends} speech_end events");

    // Diarization.
    engine.load(DIARIZATION).unwrap();
    let who = engine.diarize(DIARIZATION, &audio, None).unwrap();
    println!("diarization: {} speakers, {} turns", who.speakers, who.turns.len());
    assert!(who.speakers >= 1 && !who.turns.is_empty());
    let one = engine.diarize(DIARIZATION, &audio, Some(1)).unwrap();
    assert_eq!(one.speakers, 1);

    // Unload, then the engine ends.
    engine.unload(SPEECH, false).unwrap();
    engine.unload(VAD, false).unwrap();
    engine.unload(DIARIZATION, false).unwrap();
    let status = engine.shutdown().unwrap();
    assert!(status.success());
}
