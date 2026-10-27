//! A live session over the socket, with the real engine client, the real
//! Swift engine, and models that are already on disk. Ignored by default:
//!
//! ```text
//! (cd native/macos/SaysoEngine && swift build -c release)
//! SAYSO_CONF_MODELS=/tmp/conf/swift-models SAYSO_CONF_AUDIO=/tmp/conf/audio/short.wav \
//!   cargo test -p sayso-host --test live_real_engine -- --ignored --nocapture
//! ```
//!
//! The folder needs `parakeet-unified-en`, `silero-vad`, and
//! `speaker-diarization`, as for `jobs_real_engine.rs`.
#![cfg(target_os = "macos")]

use sayso_core::config::{Config, RoleChoice};
use sayso_core::models::ModelId;
use sayso_engine_client::EngineClient;
use sayso_extension::protocol::{Event as HostEvent, SessionReply, TextReply};
use sayso_extension::{Client, Event};
use sayso_host::{DownloadQuestion, HostServices};
use sayso_platform::SttBackend;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod common;
use common::*;

struct Services {
    engine: Arc<dyn SttBackend>,
    config: Config,
}

impl HostServices for Services {
    fn engine(&self) -> Option<Arc<dyn SttBackend>> {
        Some(self.engine.clone())
    }
    fn config(&self) -> Config {
        self.config.clone()
    }
    fn ask_download(&self, _: &DownloadQuestion, _: &dyn Fn() -> bool) -> bool {
        false
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
    let models_dir = PathBuf::from(std::env::var_os("SAYSO_CONF_MODELS").expect("set SAYSO_CONF_MODELS"));
    let audio = PathBuf::from(std::env::var_os("SAYSO_CONF_AUDIO").expect("set SAYSO_CONF_AUDIO"));
    let engine_path = EngineClient::default_engine_path();
    assert!(engine_path.exists(), "build the sidecar first: {}", engine_path.display());
    let setup = setup();
    let cache = setup.dir.path().join("cache");
    let engine: Arc<dyn SttBackend> = Arc::new(EngineClient::spawn(engine_path, models_dir, cache).unwrap());
    println!("engine features: {:?}", engine.features());

    let mut config = Config::default();
    config.dictation.model = ModelId::new("parakeet-unified-en");
    config.models.vad = RoleChoice::Model(ModelId::new("silero-vad"));
    write_state(&setup.state, &here(), &["session.live"], None);
    let host = start_with(&setup, Arc::new(Services { engine, config }));
    let client = Client::connect_to(&setup.socket).unwrap();
    let events = client.events();

    let record_to = setup.dir.path().join("meeting");
    let started = Instant::now();
    let reply = client.request("session_start", fields(json!({"sources": ["client"], "diarize": true, "record_to": record_to}))).unwrap();
    let session = reply.parse::<SessionReply>().unwrap().session;
    println!("session_start took {} ms", started.elapsed().as_millis());
    println!("live session: {:?}", host.live_session());

    // The file, 3 s of silence, and the file again, at 4 times real time.
    let once = read_pcm(&audio);
    let all = [once.clone(), vec![0i16; 3 * 16_000], once.clone()].concat();
    let fed = Instant::now();
    for chunk in all.chunks(16_000 * 160 / 1000) {
        client.send("session_audio", fields(json!({"session": session, "pcm": sayso_extension::protocol::pcm::encode_i16(chunk)}))).unwrap();
        std::thread::sleep(Duration::from_millis(40));
    }
    println!("fed {} ms of audio in {} ms", all.len() / 16, fed.elapsed().as_millis());
    let stopping = Instant::now();
    let text = client.request("session_stop", fields(json!({"session": session}))).unwrap().parse::<TextReply>().unwrap().text;
    println!("session_stop took {} ms", stopping.elapsed().as_millis());
    println!("text: {text}");

    let found: Vec<(Option<String>, HostEvent)> = events.try_iter().filter_map(|e| if let Event::Host { id, event } = e { Some((id, event)) } else { None }).collect();
    assert!(found.iter().all(|(id, _)| id.is_none()), "live events have no id");
    let partials: Vec<_> = found.iter().filter_map(|(_, e)| if let HostEvent::Partial(p) = e { Some(p) } else { None }).collect();
    let segments: Vec<_> = found.iter().filter_map(|(_, e)| if let HostEvent::Segment(s) = e { Some(s) } else { None }).collect();
    let speakers: Vec<_> = found.iter().filter_map(|(_, e)| if let HostEvent::Speakers(s) = e { Some(s) } else { None }).collect();
    println!("{} partials, last: {:?}", partials.len(), partials.last().map(|p| format!("{} | {}", p.committed, p.tentative)));
    for s in &segments {
        println!("segment {}-{} ms: {}", s.segment.start_ms, s.segment.end_ms, s.segment.text);
    }
    for s in &speakers {
        println!("speakers: {:?}", s.turns.iter().map(|t| (t.start_ms, t.end_ms, t.speaker.as_str())).collect::<Vec<_>>());
    }

    assert!(!partials.is_empty(), "the preview stream gave partials");
    assert!(partials.iter().all(|p| p.session == session));
    assert!(segments.len() >= 2, "a part for each copy of the file");
    let end_ms = all.len() as u64 / 16;
    assert!(segments.iter().all(|s| s.session == Some(session) && s.segment.start_ms < s.segment.end_ms && s.segment.end_ms <= end_ms + 300));
    assert!(segments.windows(2).all(|w| w[0].segment.start_ms <= w[1].segment.start_ms), "in the order of time");
    // The second copy starts after the first copy and the silence: its segments have session times.
    let second = (once.len() as u64 / 16 + 3000).saturating_sub(500);
    assert!(segments.iter().any(|s| s.segment.start_ms >= second), "a segment of the second copy has a session time");
    assert_eq!(speakers.len(), 1);
    assert!(!speakers[0].turns.is_empty());
    assert!(!text.is_empty());
    assert_eq!(read_pcm(&record_to.join("client.wav")), all, "the recording has every sample");
    assert_eq!(host.live_session(), None);
}
