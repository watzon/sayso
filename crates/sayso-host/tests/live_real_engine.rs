//! A live session with the real Swift engine and models that are already on
//! disk. Ignored by default:
//!
//! ```text
//! (cd native/macos/SaysoEngine && swift build -c release)
//! SAYSO_CONF_MODELS=/tmp/conf/swift-models SAYSO_CONF_AUDIO=/tmp/conf/audio/short.wav \
//!   cargo test -p sayso-host --test live_real_engine -- --ignored --nocapture
//! ```
//!
//! The folder needs `parakeet-unified-en`, `silero-vad`, and
//! `speaker-diarization`. `SAYSO_ENGINE_PATH` can name another engine.
//!
//! The engine client of Sayso gets VAD sessions and diarization in 6c, so
//! this test speaks the engine protocol itself, with the few requests that a
//! live session needs.
#![cfg(target_os = "macos")]

use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::Mutex;
use sayso_core::models::{ModelId, Vad};
use sayso_host::live::{EngineError, EventSink, LiveDeps, LiveEngine, LiveEvent, LiveSessions, NoPriority, Owner, RawTurn, SessionSpec};
use sayso_protocol::{ErrorCode, Event, Segment, Source};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The engine process and the requests that wait for a reply.
struct Engine {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    waiting: Arc<Mutex<HashMap<String, Sender<Value>>>>,
    watchers: Arc<Mutex<Vec<Sender<LiveEvent>>>>,
    next: AtomicU64,
    files: tempfile::TempDir,
}

impl Engine {
    fn start(path: &Path, models: &Path) -> Engine {
        let mut child = Command::new(path).env("SAYSO_MODELS_DIR", models).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().expect("start the engine");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let waiting: Arc<Mutex<HashMap<String, Sender<Value>>>> = Arc::default();
        let watchers: Arc<Mutex<Vec<Sender<LiveEvent>>>> = Arc::default();
        {
            let (waiting, watchers) = (waiting.clone(), watchers.clone());
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { return };
                    let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
                    let kind = message["type"].as_str().unwrap_or_default();
                    let session = message["session"].as_u64().unwrap_or_default();
                    let ms = |field: &str| message[field].as_u64().unwrap_or_default();
                    let event = match kind {
                        "partial" => Some(LiveEvent::Partial { session, committed: message["committed"].as_str().unwrap_or_default().into(), tentative: message["tentative"].as_str().unwrap_or_default().into() }),
                        "speech_start" => Some(LiveEvent::SpeechStart { session, start_ms: ms("start_ms") }),
                        "speech_end" => Some(LiveEvent::SpeechEnd { session, start_ms: ms("start_ms"), end_ms: ms("end_ms") }),
                        _ => None,
                    };
                    if let Some(event) = event {
                        watchers.lock().retain(|w| w.send(event.clone()).is_ok());
                    } else if matches!(kind, "ok" | "error" | "final" | "hello")
                        && let Some(id) = message["id"].as_str()
                        && let Some(waiter) = waiting.lock().remove(id)
                    {
                        let _ = waiter.send(message);
                    } else if kind == "error" {
                        eprintln!("engine: {line}");
                    }
                }
            });
        }
        Engine { child: Mutex::new(child), stdin: Mutex::new(stdin), waiting, watchers, next: AtomicU64::new(1), files: tempfile::TempDir::new().unwrap() }
    }

    fn send(&self, message: Value) {
        let mut stdin = self.stdin.lock();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    fn call(&self, kind: &str, mut fields: Value) -> Result<Value, EngineError> {
        let id = format!("r{}", self.next.fetch_add(1, Ordering::SeqCst));
        let (tx, rx) = unbounded();
        self.waiting.lock().insert(id.clone(), tx);
        fields["v"] = json!(1);
        fields["type"] = json!(kind);
        fields["id"] = json!(id);
        self.send(fields);
        let reply = rx.recv_timeout(Duration::from_secs(600)).map_err(|_| EngineError::new(format!("{kind}: no reply")))?;
        if reply["type"] == "error" {
            let message = reply["message"].as_str().unwrap_or_default().to_string();
            return Err(match reply["code"].as_str() {
                Some(code) => EngineError::with_code(ErrorCode::parse(code), message),
                None => EngineError::new(message),
            });
        }
        Ok(reply)
    }

    fn wav(&self, samples: &[f32]) -> PathBuf {
        let path = self.files.path().join(format!("audio-{}.wav", self.next.fetch_add(1, Ordering::SeqCst)));
        let pcm = sayso_core::stt::to_pcm16(samples);
        std::fs::write(&path, sayso_core::stt::encode_wav(&pcm)).unwrap();
        path
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.call("shutdown", json!({}));
        let _ = self.child.lock().wait();
    }
}

impl LiveEngine for Engine {
    fn load(&self, model: &ModelId) -> Result<(), EngineError> {
        self.call("load", json!({"model": model.as_str()})).map(drop)
    }

    fn start_stream(&self, session: u64, model: &ModelId, language: &str) -> Result<(), EngineError> {
        self.call("stream_start", json!({"session": session, "model": model.as_str(), "language": language})).map(drop)
    }

    fn start_vad(&self, session: u64, model: &ModelId) -> Result<(), EngineError> {
        self.call("vad_start", json!({"session": session, "model": model.as_str()})).map(drop)
    }

    fn push_audio(&self, session: u64, samples: &[f32]) -> Result<(), EngineError> {
        self.send(json!({"v": 1, "type": "stream_audio", "session": session, "pcm": sayso_protocol::pcm::encode_f32(samples)}));
        Ok(())
    }

    fn end_stream(&self, session: u64) {
        let _ = self.call("stream_end", json!({"session": session}));
    }

    fn end_vad(&self, session: u64) -> Result<(), EngineError> {
        self.call("stream_end", json!({"session": session})).map(drop)
    }

    fn transcribe_segments(&self, samples: &[f32], model: &ModelId, language: &str) -> Result<Vec<Segment>, EngineError> {
        let path = self.wav(samples);
        let reply = self.call("transcribe", json!({"model": model.as_str(), "path": path, "language": language, "vocabulary": [], "segments": true}))?;
        let _ = std::fs::remove_file(&path);
        serde_json::from_value(reply["segments"].clone()).map_err(|e| EngineError::new(format!("bad segments: {e}")))
    }

    fn diarize(&self, model: &ModelId, samples: &[f32]) -> Result<Vec<RawTurn>, EngineError> {
        let path = self.wav(samples);
        let reply = self.call("diarize", json!({"model": model.as_str(), "path": path}))?;
        let _ = std::fs::remove_file(&path);
        let turns = reply["turns"].as_array().cloned().unwrap_or_default();
        Ok(turns
            .iter()
            .map(|t| RawTurn { start_ms: t["start_ms"].as_u64().unwrap_or_default(), end_ms: t["end_ms"].as_u64().unwrap_or_default(), speaker: t["speaker"].as_u64().unwrap_or_default() as u32 })
            .collect())
    }

    fn events(&self) -> Receiver<LiveEvent> {
        let (tx, rx) = unbounded();
        self.watchers.lock().push(tx);
        rx
    }
}

#[derive(Default)]
struct Events(Mutex<Vec<(Instant, Event)>>);

impl EventSink for Events {
    fn event(&self, event: Event) {
        self.0.lock().push((Instant::now(), event));
    }
}

fn read_pcm(path: &Path) -> Vec<i16> {
    let bytes = std::fs::read(path).unwrap();
    let data = bytes.windows(4).position(|w| w == b"data").expect("a data chunk") + 8;
    bytes[data..].as_chunks::<2>().0.iter().map(|&pair| i16::from_le_bytes(pair)).collect()
}

#[test]
#[ignore = "needs the real sidecar and models on disk"]
fn a_client_session_on_the_real_engine_gives_partials_segments_and_speakers() {
    let models = PathBuf::from(std::env::var_os("SAYSO_CONF_MODELS").expect("set SAYSO_CONF_MODELS"));
    let audio = PathBuf::from(std::env::var_os("SAYSO_CONF_AUDIO").expect("set SAYSO_CONF_AUDIO"));
    let engine_path = std::env::var_os("SAYSO_ENGINE_PATH").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../native/macos/SaysoEngine/.build/release/SaysoEngine"));
    assert!(engine_path.exists(), "build the engine first: {}", engine_path.display());
    let engine = Arc::new(Engine::start(&engine_path, &models));
    let hello = engine.call("hello", json!({})).unwrap();
    println!("engine features: {}", hello["features"]);

    let temp = tempfile::TempDir::new().unwrap();
    let sessions = LiveSessions::new(LiveDeps { engine: engine.clone(), microphone: None, priority: Arc::new(NoPriority), temp_dir: temp.path().join("live") });
    let record_to = temp.path().join("meeting");
    let spec = SessionSpec {
        sources: vec![Source::Client],
        final_model: ModelId::new("parakeet-unified-en"),
        preview_model: Some(ModelId::new("parakeet-unified-en")),
        vad: Vad::Model(ModelId::new("silero-vad")),
        diarization: Some(ModelId::new("speaker-diarization")),
        language: "en".into(),
        record_to: Some(record_to.clone()),
    };
    let sink = Arc::new(Events::default());
    let started = Instant::now();
    let session = sessions.start(Owner { connection: 1, extension_id: "dev.example.notetaker".into() }, spec, sink.clone()).unwrap();
    println!("session_start took {} ms", started.elapsed().as_millis());

    // The file, 3 s of silence, and the file again, at 4 times real time.
    let once = read_pcm(&audio);
    let all = [once.clone(), vec![0i16; 3 * 16_000], once].concat();
    let fed = Instant::now();
    for chunk in all.chunks(16_000 * 160 / 1000) {
        sessions.audio(1, session, &sayso_protocol::pcm::encode_i16(chunk)).unwrap();
        std::thread::sleep(Duration::from_millis(40));
    }
    println!("fed {} ms of audio in {} ms", all.len() / 16, fed.elapsed().as_millis());
    let stopping = Instant::now();
    let text = sessions.stop(1, session).unwrap();
    println!("session_stop took {} ms", stopping.elapsed().as_millis());
    println!("text: {text}");

    let events = sink.0.lock().clone();
    let partials: Vec<_> = events.iter().filter_map(|(_, e)| if let Event::Partial(p) = e { Some(p) } else { None }).collect();
    let segments: Vec<_> = events.iter().filter_map(|(at, e)| if let Event::Segment(s) = e { Some((at.duration_since(started), s)) } else { None }).collect();
    let speakers: Vec<_> = events.iter().filter_map(|(_, e)| if let Event::Speakers(s) = e { Some(s) } else { None }).collect();
    println!("{} partials, last: {:?}", partials.len(), partials.last().map(|p| format!("{} | {}", p.committed, p.tentative)));
    for (at, s) in &segments {
        println!("segment at {} ms: {}-{} ms: {}", at.as_millis(), s.segment.start_ms, s.segment.end_ms, s.segment.text);
    }
    for s in &speakers {
        println!("speakers: {:?}", s.turns);
    }

    assert!(!partials.is_empty(), "the preview stream gave partials");
    assert!(segments.len() >= 2, "a part for each copy of the file");
    let end_ms = all.len() as u64 / 16;
    assert!(segments.iter().all(|(_, s)| s.segment.start_ms < s.segment.end_ms && s.segment.end_ms <= end_ms + 300));
    assert!(segments.windows(2).all(|w| w[0].1.segment.start_ms <= w[1].1.segment.start_ms), "in the order of time");
    // The second copy starts after the first copy and the silence: its segments have session times.
    let second = (once_len_ms(&audio) + 3000).saturating_sub(500);
    assert!(segments.iter().any(|(_, s)| s.segment.start_ms >= second), "a segment of the second copy has a session time");
    assert_eq!(speakers.len(), 1);
    assert!(!speakers[0].turns.is_empty());
    assert!(!text.is_empty());
    let recording = read_pcm(&record_to.join("client.wav"));
    assert_eq!(recording, all, "the recording has every sample");
}

fn once_len_ms(audio: &Path) -> u64 {
    read_pcm(audio).len() as u64 / 16
}
