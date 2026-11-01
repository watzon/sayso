//! A live session through [`Speech`], with a fake engine and with the real engine.

use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::Mutex;
use sayso_sdk::embedded::{
    App, DownloadProgress, EngineError, EngineEvent, EngineModels, EngineSource, Event, AudioFeed, OpenError,
    Microphone, Mode, ModelId, ModelInfo, ModelState, Options, Role, Segment, SessionOptions,
    Source, Speech, SpeechEngine, SpeechPart, Turn, VadRun,
};
use std::sync::Arc;
use std::time::Duration;

const RATE: usize = 16_000;

/// An engine that gives a partial for each piece of audio and "part N" for each final pass.
#[derive(Default)]
struct Fake {
    watchers: Mutex<Vec<Sender<EngineEvent>>>,
    finals: Mutex<u32>,
    downloads: Mutex<Vec<String>>,
}

impl SpeechEngine for Fake {
    fn load(&self, _: &ModelId) -> Result<(), EngineError> {
        Ok(())
    }
    fn transcribe(&self, _: &[f32], _: &ModelId, _: &str) -> Result<String, EngineError> {
        Err(EngineError::new("not in a live session"))
    }
    fn transcribe_segments(
        &self,
        samples: &[f32],
        _: &ModelId,
        _: &str,
    ) -> Result<Vec<Segment>, EngineError> {
        let mut finals = self.finals.lock();
        *finals += 1;
        Ok(vec![Segment {
            start_ms: 0,
            end_ms: samples.len() as u64 / 16,
            text: format!("part {finals}"),
            words: None,
        }])
    }
    fn detect_speech(&self, _: &ModelId, _: &[f32]) -> Result<Vec<SpeechPart>, EngineError> {
        Ok(Vec::new())
    }
    fn diarize(&self, _: &ModelId, _: &[f32], _: Option<u32>) -> Result<Vec<Turn>, EngineError> {
        Ok(Vec::new())
    }
    fn start_stream(&self, _: u64, _: &ModelId, _: &str) -> Result<(), EngineError> {
        Ok(())
    }
    fn start_vad(&self, _: u64, _: &ModelId) -> Result<(), EngineError> {
        Ok(())
    }
    fn push_audio(&self, session: u64, samples: &[f32]) -> Result<(), EngineError> {
        let event = EngineEvent::Partial {
            session,
            committed: String::new(),
            tentative: format!("{} samples", samples.len()),
        };
        self.watchers
            .lock()
            .retain(|watcher| watcher.send(event.clone()).is_ok());
        Ok(())
    }
    fn end_stream(&self, _: u64) {}
    fn end_vad(&self, _: u64) -> Result<(), EngineError> {
        Ok(())
    }
    fn events(&self) -> Receiver<EngineEvent> {
        let (tx, rx) = unbounded();
        self.watchers.lock().push(tx);
        rx
    }
}

impl EngineModels for Fake {
    fn catalog(&self) -> Result<Vec<ModelInfo>, EngineError> {
        Ok(vec![ModelInfo {
            id: "final".into(),
            name: "Final".into(),
            size_bytes: 1,
            languages: vec!["en".into()],
            roles: vec![Role::Final],
            state: ModelState::Downloaded,
            recommended: false,
        }])
    }
    fn download(
        &self,
        model: &str,
        _: Option<u64>,
        on_progress: &mut dyn FnMut(&DownloadProgress),
    ) -> Result<(), EngineError> {
        self.downloads.lock().push(model.to_string());
        on_progress(&DownloadProgress {
            model: model.into(),
            fraction: 1.0,
            bytes_done: 1,
            bytes_total: 1,
        });
        Ok(())
    }
    fn delete(&self, _: &str) -> Result<(), EngineError> {
        Ok(())
    }
    fn load(&self, _: &str) -> Result<(), EngineError> {
        Ok(())
    }
    fn unload(&self, _: &str) -> Result<(), EngineError> {
        Ok(())
    }
}

struct NoMic;

impl Microphone for NoMic {
    fn open(&self) -> Result<AudioFeed, OpenError> {
        Err(OpenError::Failed("no microphone in this test".into()))
    }
}

fn speech_with(temp: &std::path::Path) -> (Speech, Arc<Fake>) {
    let fake = Arc::new(Fake::default());
    let options = Options {
        mode: Mode::Local,
        app: App::new("Test"),
        engine: EngineSource::Custom {
            engine: fake.clone(),
            models: fake.clone(),
        },
        temp_dir: temp.to_path_buf(),
        microphone: Some(Arc::new(NoMic)),
    };
    (Speech::connect(options).unwrap(), fake)
}

fn client_session(limit: Option<Duration>) -> SessionOptions {
    SessionOptions {
        sources: vec![Source::Client],
        preview_model: Some(ModelId::new("final")),
        vad: VadRun::Basic,
        language: "en".into(),
        silence_limit: limit,
        ..SessionOptions::mic("final")
    }
}

/// Something like speech: a voice of 140 Hz in syllables.
fn voice(ms: usize) -> Vec<f32> {
    (0..ms * RATE / 1000)
        .map(|n| {
            let t = n as f32 / RATE as f32;
            let tone: f32 = (1..=5)
                .map(|h| (2.0 * std::f32::consts::PI * 140.0 * h as f32 * t).sin() / h as f32)
                .sum();
            if (t * 5.0).fract() < 0.7 {
                tone * 0.2
            } else {
                tone * 0.04
            }
        })
        .collect()
}

#[test]
fn a_session_gives_partials_and_segments_and_the_text() {
    let temp = tempfile::tempdir().unwrap();
    let (speech, _) = speech_with(temp.path());
    assert_eq!(speech.mode(), Mode::Local);
    assert_eq!(speech.models().unwrap()[0].id, "final");
    let events = speech.events();
    let session = speech.start_session(client_session(None)).unwrap();
    speech.session_audio(session, &vec![0.0; RATE / 2]).unwrap();
    speech.session_audio(session, &voice(1000)).unwrap();
    assert_eq!(speech.stop_session(session).unwrap(), "part 1");
    let got: Vec<Event> = events.try_iter().collect();
    assert!(
        got.iter().any(|e| matches!(e, Event::Partial(_))),
        "{got:?}"
    );
    assert!(
        got.iter()
            .any(|e| matches!(e, Event::Segment(s) if s.segment.text == "part 1")),
        "{got:?}"
    );
    assert!(
        std::fs::read_dir(temp.path().join(""))
            .unwrap()
            .flatten()
            .all(|e| !e.file_name().to_string_lossy().starts_with("sayso-live-")),
        "the recording goes with the session"
    );
}

#[test]
fn the_silence_limit_gives_one_event_and_the_session_still_ends_with_its_text() {
    let temp = tempfile::tempdir().unwrap();
    let (speech, _) = speech_with(temp.path());
    let events = speech.events();
    let session = speech
        .start_session(client_session(Some(Duration::from_secs(1))))
        .unwrap();
    speech.session_audio(session, &voice(1000)).unwrap();
    speech.session_audio(session, &vec![0.0; RATE * 2]).unwrap();
    let mut limits = 0;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while limits == 0 && std::time::Instant::now() < deadline {
        if let Ok(Event::SilenceLimit {
            session: id,
            silence_ms,
        }) = events.recv_timeout(Duration::from_millis(50))
        {
            assert_eq!((id, silence_ms), (session, 1000));
            limits += 1;
        }
    }
    assert_eq!(limits, 1);
    assert_eq!(speech.stop_session(session).unwrap(), "part 1");
    assert_eq!(
        events
            .try_iter()
            .filter(|e| matches!(e, Event::SilenceLimit { .. }))
            .count(),
        0,
        "one event"
    );
    // A new session can start after that.
    let again = speech.start_session(client_session(None)).unwrap();
    assert!(again > session);
    speech.cancel_session();
}

#[test]
fn a_download_reports_progress_on_the_thread_of_the_caller() {
    let temp = tempfile::tempdir().unwrap();
    let (speech, fake) = speech_with(temp.path());
    let mut seen = Vec::new();
    speech
        .download("final", Some(1), |progress| seen.push(progress.fraction))
        .unwrap();
    assert_eq!(seen, vec![1.0]);
    assert_eq!(fake.downloads.lock().clone(), vec!["final".to_string()]);
}

/// The Swift engine on macOS, or `SAYSO_ENGINE`. This test downloads nothing.
#[cfg(target_os = "macos")]
fn real_engine() -> Option<std::path::PathBuf> {
    let program = std::env::var_os("SAYSO_ENGINE")
        .map(Into::into)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../native/macos/SaysoEngine/.build/release/SaysoEngine")
        });
    program.is_file().then_some(program)
}

#[cfg(not(target_os = "macos"))]
fn real_engine() -> Option<std::path::PathBuf> {
    std::env::var_os("SAYSO_ENGINE").map(Into::into)
}

#[test]
fn the_real_engine_lists_the_models_of_dictation() {
    let Some(program) = real_engine() else {
        eprintln!("skipped: no engine. Build it, or set SAYSO_ENGINE");
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let options = Options {
        mode: Mode::Local,
        app: App::new("Test"),
        engine: EngineSource::program(program, temp.path().join("models")),
        temp_dir: temp.path().join("tmp"),
        microphone: Some(Arc::new(NoMic)),
    };
    let speech = Speech::connect(options).unwrap();
    let models = speech.models().unwrap();
    let find = |id: &str| {
        models
            .iter()
            .find(|m| m.id == id)
            .unwrap_or_else(|| panic!("{id} is not in the catalog"))
    };
    assert!(find("parakeet-tdt-v3").has_role(&Role::Final));
    assert!(find("silero-vad").has_role(&Role::Vad));
}

/// A real run: `SAYSO_SDK_CLIP=<16 kHz mono 16-bit WAV>`, `SAYSO_SDK_MODELS=<folder>`, and the models on disk there.
#[test]
#[ignore = "needs the engine, a clip, and the models on disk"]
fn real_parakeet_on_a_clip() {
    let clip = std::env::var("SAYSO_SDK_CLIP").expect("SAYSO_SDK_CLIP");
    let models = std::env::var("SAYSO_SDK_MODELS").expect("SAYSO_SDK_MODELS");
    let preview = std::env::var("SAYSO_SDK_PREVIEW").ok();
    let samples = read_wav(&clip);
    let temp = tempfile::tempdir().unwrap();
    let options = Options {
        mode: Mode::Local,
        app: App::new("Test"),
        engine: EngineSource::program(real_engine().expect("an engine"), models),
        temp_dir: temp.path().to_path_buf(),
        microphone: Some(Arc::new(NoMic)),
    };
    let speech = Speech::connect(options).unwrap();
    let events = speech.events();
    let started = std::time::Instant::now();
    let session = speech
        .start_session(SessionOptions {
            sources: vec![Source::Client],
            preview_model: preview.map(ModelId::new),
            vad: VadRun::Model(ModelId::new("silero-vad")),
            language: "en".into(),
            silence_limit: Some(Duration::from_secs(5)),
            ..SessionOptions::mic("parakeet-tdt-v3")
        })
        .unwrap();
    println!("started in {:?}", started.elapsed());
    for chunk in samples.chunks(RATE / 5) {
        speech.session_audio(session, chunk).unwrap();
        std::thread::sleep(Duration::from_millis(100));
    }
    let stopped = std::time::Instant::now();
    let text = speech.stop_session(session).unwrap();
    println!("stopped in {:?}", stopped.elapsed());
    for event in events.try_iter() {
        match event {
            Event::Partial(p) => println!("partial: {}{}", p.committed, p.tentative),
            Event::Segment(s) => println!(
                "segment {}-{}: {}",
                s.segment.start_ms, s.segment.end_ms, s.segment.text
            ),
            other => println!("{other:?}"),
        }
    }
    println!("text: {text}");
    assert!(!text.is_empty());
}

fn read_wav(path: &str) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap();
    let data = bytes
        .windows(4)
        .position(|w| w == b"data")
        .expect("a data chunk")
        + 8;
    bytes[data..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0)
        .collect()
}
