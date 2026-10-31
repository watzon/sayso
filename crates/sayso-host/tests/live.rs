//! `session_start`, `session_audio`, and `session_stop` over a real socket,
//! with the real identity check (see `common`) and a fake engine. As in
//! `jobs.rs`, "speech" is a run of one constant value, where the value 3 is
//! the word "w3", and zeros are silence. The VAD sessions and the preview
//! streams of the fake engine find the runs in the audio that they get.

use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::Mutex;
use sayso_core::config::Config;
use sayso_core::models::{self, ModelId, ModelInfo, ModelTask};
use sayso_core::stt::{self, Diarization, EngineEvent, ModelStatus, SessionOptions, TimedTranscript, Transcript, Turn};
use sayso_sdk::protocol::{Capability, ErrorCode, Event as HostEvent, SessionReply, Source, SourceEndReason, TextReply};
use sayso_sdk::client::{Client, Event};
use sayso_host::live::{AudioFeed, CapturedAudio, FeedItem, LiveInfo, LiveObserver, Microphone, OpenError, SourceEnd, SystemAudio};
use sayso_host::{DownloadQuestion, Host, HostServices};
use sayso_platform::{PlatformError, Result, SttBackend};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

mod common;
use common::*;

const LIVE: &[&str] = &["session.live"];

// ---------------------------------------------------------------------------
// The fake engine, the microphone, and the services
// ---------------------------------------------------------------------------

/// The state of a VAD session: the samples so far, and the start of the open speech.
#[derive(Default)]
struct VadState {
    samples: u64,
    open: Option<u64>,
}

struct LiveEngine {
    on_disk: HashSet<ModelId>,
    features: Vec<String>,
    loaded: Mutex<HashSet<ModelId>>,
    subscribers: Mutex<Vec<Sender<EngineEvent>>>,
    /// The open preview streams, with the samples so far.
    streams: Mutex<HashMap<u64, u64>>,
    vads: Mutex<HashMap<u64, VadState>>,
    /// The engine refuses a stream or a VAD session when this many are open.
    max_streams: usize,
    transcriptions: AtomicUsize,
    /// A load waits for one message here, when it is set. It says first that it runs.
    load_gate: Mutex<Option<(Sender<()>, Receiver<()>)>>,
}

impl LiveEngine {
    fn new() -> LiveEngine {
        LiveEngine {
            on_disk: [streaming(), first_of(ModelTask::Vad), first_of(ModelTask::Diarization)].into(),
            features: ["segments", "vad", "diarization", "concurrent_streams"].map(String::from).to_vec(),
            loaded: Mutex::default(),
            subscribers: Mutex::default(),
            streams: Mutex::default(),
            vads: Mutex::default(),
            max_streams: 8,
            transcriptions: AtomicUsize::new(0),
            load_gate: Mutex::default(),
        }
    }

    fn emit(&self, event: EngineEvent) {
        self.subscribers.lock().retain(|tx| tx.send(event.clone()).is_ok());
    }

    /// The preview streams and VAD sessions that are open.
    fn open_streams(&self) -> usize {
        self.streams.lock().len() + self.vads.lock().len()
    }

    fn full(&self) -> Result<()> {
        if self.open_streams() >= self.max_streams {
            return Err(PlatformError::Engine { code: "too_many_streams".into(), message: "the streams are full; end one first".into() });
        }
        Ok(())
    }
}

/// The value of a sample: 3 for the word "w3", 0 for silence.
fn value(sample: f32) -> i32 {
    (sample * 32768.0).round() as i32 / 1000
}

/// The runs of one non-zero value: (start_ms, end_ms, value).
fn runs(samples: &[f32]) -> Vec<(u64, u64, i32)> {
    let mut found = Vec::new();
    let mut open: Option<(usize, i32)> = None;
    for (i, &s) in samples.iter().chain(std::iter::once(&0.0)).enumerate() {
        let value = value(s);
        match open {
            Some((_, v)) if v == value => {}
            Some((start, v)) => {
                found.push((start as u64 / 16, i as u64 / 16, v));
                open = (value != 0).then_some((i, value));
            }
            None if value != 0 => open = Some((i, value)),
            None => {}
        }
    }
    found
}

impl SttBackend for LiveEngine {
    fn catalog(&self) -> Vec<ModelInfo> {
        Vec::new()
    }
    fn status(&self, model: &ModelId) -> ModelStatus {
        if self.loaded.lock().contains(model) {
            ModelStatus::Ready
        } else if self.on_disk.contains(model) {
            ModelStatus::Downloaded
        } else {
            ModelStatus::NotDownloaded
        }
    }
    fn download(&self, _: &ModelId) -> Result<()> {
        Ok(())
    }
    fn delete(&self, _: &ModelId) -> Result<()> {
        Ok(())
    }
    fn load(&self, model: &ModelId) -> Result<()> {
        if let Some((started, release)) = self.load_gate.lock().take() {
            let _ = started.send(());
            let _ = release.recv();
        }
        self.loaded.lock().insert(model.clone());
        Ok(())
    }
    fn unload(&self, _: &ModelId, _: bool) -> Result<()> {
        Ok(())
    }
    fn preview_ready(&self, _: &ModelId) -> bool {
        true
    }
    fn start_stream(&self, session: u64, _: &SessionOptions) -> Result<()> {
        self.full()?;
        self.streams.lock().insert(session, 0);
        Ok(())
    }
    fn start_vad(&self, session: u64, _: &ModelId) -> Result<()> {
        self.full()?;
        self.vads.lock().insert(session, VadState::default());
        Ok(())
    }
    /// A preview stream says how much audio it heard. A VAD session finds the runs.
    fn push_audio(&self, session: u64, samples: &[f32]) -> Result<()> {
        let mut events = Vec::new();
        if let Some(heard) = self.streams.lock().get_mut(&session) {
            *heard += samples.len() as u64;
            events.push(EngineEvent::Partial { session, committed: String::new(), tentative: format!("{} ms", *heard / 16) });
        }
        if let Some(vad) = self.vads.lock().get_mut(&session) {
            for &sample in samples {
                let ms = vad.samples / 16;
                match (vad.open, value(sample) != 0) {
                    (None, true) => {
                        vad.open = Some(ms);
                        events.push(EngineEvent::SpeechStart { session, start_ms: ms });
                    }
                    (Some(start_ms), false) => {
                        vad.open = None;
                        events.push(EngineEvent::SpeechEnd { session, start_ms, end_ms: ms });
                    }
                    _ => {}
                }
                vad.samples += 1;
            }
        }
        for event in events {
            self.emit(event);
        }
        Ok(())
    }
    fn end_stream(&self, session: u64) {
        self.streams.lock().remove(&session);
    }
    /// As the engine: the open speech ends at the end of the audio, before the reply.
    fn end_vad(&self, session: u64) -> Result<()> {
        let Some(vad) = self.vads.lock().remove(&session) else { return Err(PlatformError::Failed(format!("no VAD session {session}"))) };
        if let Some(start_ms) = vad.open {
            self.emit(EngineEvent::SpeechEnd { session, start_ms, end_ms: vad.samples / 16 });
        }
        Ok(())
    }
    fn transcribe(&self, _: &[f32], _: &SessionOptions) -> Result<Transcript> {
        Err(PlatformError::Failed("this test uses segments".into()))
    }
    fn features(&self) -> Vec<String> {
        self.features.clone()
    }
    fn transcribe_segments(&self, samples: &[f32], options: &SessionOptions) -> Result<TimedTranscript> {
        self.transcriptions.fetch_add(1, Ordering::SeqCst);
        let segments = runs(samples).into_iter().map(|(start_ms, end_ms, v)| stt::Segment { start_ms, end_ms, text: format!("w{v}"), words: Vec::new() }).collect();
        Ok(TimedTranscript { text: String::new(), model: options.final_model.clone(), elapsed_ms: 1, segments })
    }
    /// Odd words are speaker 0, even words speaker 1.
    fn diarize(&self, _: &ModelId, samples: &[f32], _: Option<u32>) -> Result<Diarization> {
        let turns = runs(samples).into_iter().map(|(start_ms, end_ms, v)| Turn { start_ms, end_ms, speaker: (v as u32 + 1) % 2 }).collect();
        Ok(Diarization { speakers: 2, turns })
    }
    fn subscribe(&self) -> Receiver<EngineEvent> {
        let (tx, rx) = unbounded();
        self.subscribers.lock().push(tx);
        rx
    }
}

/// A microphone that the test feeds. Like the shared microphone of the app,
/// it closes the channel of a feed when its lease drops.
#[derive(Default)]
struct FakeMic {
    feeds: Arc<Mutex<HashMap<u64, Sender<FeedItem>>>>,
    next: AtomicUsize,
}

struct Lease(u64, Arc<Mutex<HashMap<u64, Sender<FeedItem>>>>);

impl Drop for Lease {
    fn drop(&mut self) {
        self.1.lock().remove(&self.0);
    }
}

impl FakeMic {
    fn feed(&self, samples: &[i16]) {
        let frame: Vec<f32> = samples.iter().map(|&s| f32::from(s) / 32768.0).collect();
        for tx in self.feeds.lock().values() {
            let _ = tx.send(FeedItem::Audio(CapturedAudio::untimed(frame.clone())));
        }
    }

    fn open_feeds(&self) -> usize {
        self.feeds.lock().len()
    }
}

impl Microphone for FakeMic {
    fn open(&self) -> std::result::Result<AudioFeed, OpenError> {
        let (tx, items) = unbounded();
        let id = self.next.fetch_add(1, Ordering::SeqCst) as u64;
        self.feeds.lock().insert(id, tx);
        Ok(AudioFeed { items, lease: Box::new(Lease(id, self.feeds.clone())), excludes: Vec::new() })
    }
}

/// The system audio, which the test steers.
#[derive(Default)]
struct FakeSystem {
    refused: AtomicBool,
    feed: Mutex<Option<Sender<FeedItem>>>,
    /// The process that the session asked to leave out.
    app_pid: Mutex<Option<u32>>,
}

impl FakeSystem {
    fn send(&self, item: FeedItem) {
        self.feed.lock().as_ref().expect("the capture runs").send(item).unwrap();
    }
}

impl SystemAudio for FakeSystem {
    fn open(&self, app_pid: Option<u32>) -> std::result::Result<AudioFeed, OpenError> {
        if self.refused.load(Ordering::SeqCst) {
            return Err(OpenError::Permission("the system audio permission is denied".into()));
        }
        *self.app_pid.lock() = app_pid;
        let (tx, items) = unbounded();
        *self.feed.lock() = Some(tx);
        Ok(AudioFeed { items, lease: Box::new(()), excludes: vec!["sayso".into(), "app".into()] })
    }
}

struct Services {
    engine: Arc<LiveEngine>,
    mic: Option<Arc<FakeMic>>,
    system: Option<Arc<FakeSystem>>,
    config: Config,
    temp: PathBuf,
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
    fn ask_approval(&self, _: &sayso_host::ApprovalQuestion, _: &dyn Fn() -> bool) -> Option<bool> {
        None
    }
    fn microphone(&self) -> Option<Arc<dyn Microphone>> {
        self.mic.clone().map(|mic| mic as Arc<dyn Microphone>)
    }
    fn system_audio(&self) -> Option<Arc<dyn SystemAudio>> {
        self.system.clone().map(|system| system as Arc<dyn SystemAudio>)
    }
    fn live_temp_dir(&self) -> PathBuf {
        self.temp.clone()
    }
}

#[derive(Default)]
struct Observer(Mutex<Vec<Option<LiveInfo>>>);

impl LiveObserver for Observer {
    fn changed(&self, live: Option<&LiveInfo>) {
        self.0.lock().push(live.cloned());
    }
}

fn first_of(task: ModelTask) -> ModelId {
    models::models_for(task).next().unwrap().id.clone()
}

/// A model that does the final pass and streams.
fn streaming() -> ModelId {
    models::catalog().iter().find(|m| m.final_pass && m.live_preview && m.understands("en")).unwrap().id.clone()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct World {
    host: Host,
    setup: Setup,
    engine: Arc<LiveEngine>,
    mic: Arc<FakeMic>,
    system: Arc<FakeSystem>,
}

fn world(granted: &[&str], engine: LiveEngine) -> World {
    world_with(granted, engine, false)
}

/// `system`: this host can capture the system audio.
fn world_with(granted: &[&str], engine: LiveEngine, system: bool) -> World {
    let setup = setup();
    write_apps(&setup.apps, granted);
    let engine = Arc::new(engine);
    let mic = Arc::new(FakeMic::default());
    let mut config = Config::default();
    config.dictation.model = streaming();
    let temp = setup.dir.path().join("live-temp");
    let fake_system = Arc::new(FakeSystem::default());
    let services = Services { engine: engine.clone(), mic: Some(mic.clone()), system: system.then(|| fake_system.clone()), config, temp };
    let host = start_with(&setup, Arc::new(services));
    World { host, setup, engine, mic, system: fake_system }
}

impl World {
    fn client(&self) -> (Client, Receiver<Event>) {
        let client = Client::connect_to(&self.setup.socket).unwrap();
        let events = client.events();
        (client, events)
    }
}

/// `len_ms` of audio with the words `(start_ms, end_ms, value)`.
fn audio(len_ms: u64, words: &[(u64, u64, i16)]) -> Vec<i16> {
    let mut samples = vec![0i16; (len_ms * 16) as usize];
    for &(start, end, value) in words {
        samples[(start * 16) as usize..(end * 16) as usize].fill(value * 1000);
    }
    samples
}

const TWO: [(u64, u64, i16); 2] = [(1_000, 3_000, 1), (8_000, 10_000, 2)];

fn start(client: &Client, body: serde_json::Value) -> Answer {
    client.request("session_start", fields(body))
}

fn session_of(answer: Answer) -> u64 {
    answer.unwrap().parse::<SessionReply>().unwrap().session
}

fn stop(client: &Client, session: u64) -> String {
    client.request("session_stop", fields(json!({"session": session}))).unwrap().parse::<TextReply>().unwrap().text
}

/// Send the audio in frames of 160 ms, without an `id`.
fn send_audio(client: &Client, session: u64, samples: &[i16]) {
    for chunk in samples.chunks(16 * 160) {
        client.send("session_audio", fields(json!({"session": session, "pcm": sayso_sdk::protocol::pcm::encode_i16(chunk)}))).unwrap();
    }
}

/// The events that came so far. Every live event has no `id`.
fn drain(events: &Receiver<Event>) -> Vec<HostEvent> {
    events
        .try_iter()
        .filter_map(|e| match e {
            Event::Host { id, event } => {
                assert_eq!(id, None, "a live event has no id: {event:?}");
                Some(event)
            }
            Event::Error(error) => panic!("an error without an id: {error:?}"),
            Event::HostGone => None,
        })
        .collect()
}

fn read_wav(path: &Path) -> Vec<i16> {
    let bytes = std::fs::read(path).unwrap();
    bytes[44..].as_chunks::<2>().0.iter().map(|&pair| i16::from_le_bytes(pair)).collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn a_client_session_gives_partials_segments_speakers_and_its_text() {
    let world = world(LIVE, LiveEngine::new());
    let observer = Arc::new(Observer::default());
    world.host.set_live_observer(observer.clone());
    let (client, events) = world.client();
    let record_to = world.setup.dir.path().join("meeting");
    let session = session_of(start(&client, json!({"sources": ["client"], "diarize": true, "record_to": record_to})));
    let running = LiveInfo { session, app_id: id(), sources: vec![Source::Client], captures_mic: false, stopping: false };
    assert_eq!(world.host.live_session(), Some(running.clone()));
    assert_eq!(world.engine.open_streams(), 2, "a preview stream and a VAD session");

    let samples = audio(12_000, &TWO);
    send_audio(&client, session, &samples);
    // With an `id`, `session_audio` gets `ok`.
    let pcm = sayso_sdk::protocol::pcm::encode_i16(&[0; 160]);
    client.request("session_audio", fields(json!({"session": session, "pcm": pcm}))).unwrap();
    let text = stop(&client, session);
    assert_eq!(text, "w1 w2");
    assert_eq!(world.host.live_session(), None);
    assert_eq!(world.engine.open_streams(), 0);

    // The reply of `session_stop` comes after the last events of the session.
    let found = drain(&events);
    let partials = found.iter().filter(|e| matches!(e, HostEvent::Partial(p) if p.session == session && p.source == Some(Source::Client))).count();
    assert!(partials > 0, "{found:?}");
    let segments: Vec<(u64, u64, String)> = found
        .iter()
        .filter_map(|e| match e {
            HostEvent::Segment(s) => {
                assert_eq!((s.job, s.session, s.source.clone()), (None, Some(session), Some(Source::Client)));
                Some((s.segment.start_ms, s.segment.end_ms, s.segment.text.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(segments, [(1_000, 3_000, "w1".into()), (8_000, 10_000, "w2".into())]);
    let HostEvent::Speakers(speakers) = found.last().unwrap() else { panic!("speakers last: {found:?}") };
    assert_eq!((speakers.session, speakers.source.clone()), (Some(session), Some(Source::Client)));
    let turns: Vec<(u64, u64, &str)> = speakers.turns.iter().map(|t| (t.start_ms, t.end_ms, t.speaker.as_str())).collect();
    assert_eq!(turns, [(1_000, 3_000, "1"), (8_000, 10_000, "2")]);

    // The recording has every sample, also the last request with an `id`.
    let recorded = read_wav(&record_to.join("client.wav"));
    assert_eq!(recorded.len(), samples.len() + 160);
    assert_eq!(&recorded[..samples.len()], &samples[..]);
    // The models stayed held while the session ran; now they wait for the idle time.
    assert!(world.host.model_use().wanted(Some(Duration::from_secs(60))).contains(&streaming()));
    let seen = observer.0.lock().clone();
    assert_eq!(seen.first(), Some(&Some(running)));
    assert_eq!(seen.last(), Some(&None));
}

#[test]
fn a_session_without_audio_stops_with_no_text() {
    let world = world(LIVE, LiveEngine::new());
    let (client, events) = world.client();
    let session = session_of(start(&client, json!({"sources": ["client"], "vad_model": "basic"})));
    assert_eq!(world.engine.open_streams(), 1, "only the preview stream");
    assert_eq!(stop(&client, session), "");
    assert!(drain(&events).iter().all(|e| !matches!(e, HostEvent::Segment(_))));
    // A second stop, and audio after the stop, name no session that runs.
    let error = refused(client.request("session_stop", fields(json!({"session": session}))));
    assert_eq!(error.code, Some(ErrorCode::BadRequest));
    let pcm = sayso_sdk::protocol::pcm::encode_i16(&[0; 160]);
    assert_eq!(refused(client.request("session_audio", fields(json!({"session": session, "pcm": pcm.clone()})))).code, Some(ErrorCode::BadRequest));
    // Without an `id`, the error has none either, and comes with the events.
    client.send("session_audio", fields(json!({"session": session, "pcm": pcm}))).unwrap();
    match events.recv_timeout(Duration::from_secs(5)) {
        Ok(Event::Error(error)) => assert_eq!(error.code, Some(ErrorCode::BadRequest)),
        other => panic!("expected an error without an id, got {other:?}"),
    }
}

#[test]
fn one_session_runs_at_a_time_across_connections() {
    let world = world(LIVE, LiveEngine::new());
    let (first, _) = world.client();
    let session = session_of(start(&first, json!({"sources": ["client"]})));

    let (second, _) = world.client();
    let busy = refused(start(&second, json!({"sources": ["client"]})));
    assert_eq!(busy.code, Some(ErrorCode::Busy));
    // The other connection cannot stop or feed a session that it did not start.
    assert_eq!(refused(second.request("session_stop", fields(json!({"session": session})))).code, Some(ErrorCode::BadRequest));
    assert_eq!(world.host.live_session().map(|l| l.app_id), Some(id()));

    stop(&first, session);
    let session = session_of(start(&second, json!({"sources": ["client"]})));
    assert_eq!(world.host.live_session().map(|l| l.app_id), Some(id()));
    stop(&second, session);
}

#[test]
fn a_session_needs_its_capabilities() {
    let world = world(&[], LiveEngine::new());
    let (client, _) = world.client();
    let error = refused(start(&client, json!({"sources": ["client"]})));
    assert_eq!((error.code, error.capability), (Some(ErrorCode::Capability), Some(Capability::SessionLive)));
    let error = refused(client.request("session_audio", fields(json!({"session": 1, "pcm": ""}))));
    assert_eq!(error.capability, Some(Capability::SessionLive));
    // An app can always stop its own session, so this needs no grant.
    assert_eq!(refused(client.request("session_stop", fields(json!({"session": 1})))).code, Some(ErrorCode::BadRequest));

    let world = self::world(LIVE, LiveEngine::new());
    let (client, _) = world.client();
    let error = refused(start(&client, json!({"sources": ["client", "mic"]})));
    assert_eq!((error.code, error.capability), (Some(ErrorCode::Capability), Some(Capability::AudioMic)));
    let error = refused(start(&client, json!({"sources": ["system"]})));
    assert_eq!(error.capability, Some(Capability::AudioSystem));
    assert_eq!(world.host.live_session(), None);
}

#[test]
fn the_source_system_is_unsupported() {
    let world = world(&["session.live", "audio.system"], LiveEngine::new());
    let (client, _) = world.client();
    let error = refused(start(&client, json!({"sources": ["system"]})));
    assert_eq!(error.code, Some(ErrorCode::Unsupported));
    assert_eq!(world.host.live_session(), None);
    assert_eq!(world.engine.open_streams(), 0);
}

#[test]
fn the_source_system_replies_with_what_it_leaves_out_and_gives_its_events() {
    let world = world_with(&["session.live", "audio.system", "audio.mic"], LiveEngine::new(), true);
    let (client, events) = world.client();
    let reply = start(&client, json!({"sources": ["mic", "system"]})).unwrap().parse::<SessionReply>().unwrap();
    assert_eq!(reply.excludes, ["sayso", "app"]);
    assert_eq!(*world.system.app_pid.lock(), Some(std::process::id()), "the capture leaves out the process of the app");
    world.system.send(FeedItem::Gap { ms: 240 });
    world.system.send(FeedItem::Ended(SourceEnd::Permission));
    let seen = Mutex::new(Vec::new());
    eventually("the events arrive", || {
        let mut all = seen.lock();
        all.extend(drain(&events));
        let gap = all.iter().any(|e| matches!(e, HostEvent::SourceGap { session, source: Source::System, ms: 240 } if *session == reply.session));
        let ended = all.iter().any(|e| matches!(e, HostEvent::SourceEnded { source: Source::System, reason: SourceEndReason::Permission, .. }));
        gap && ended
    });
    assert!(world.host.live_session().is_some(), "the session goes on with the microphone");
    assert_eq!(world.mic.open_feeds(), 1);
    stop(&client, reply.session);
}

#[test]
fn a_refused_system_audio_permission_gets_the_code_permission() {
    let world = world_with(&["session.live", "audio.system", "audio.mic"], LiveEngine::new(), true);
    world.system.refused.store(true, Ordering::SeqCst);
    let (client, _) = world.client();
    let error = refused(start(&client, json!({"sources": ["mic", "system"]})));
    assert_eq!(error.code, Some(ErrorCode::Permission));
    assert_eq!(world.host.live_session(), None);
    assert_eq!((world.engine.open_streams(), world.mic.open_feeds()), (0, 0), "the failed start closes what it opened");
}

#[test]
fn a_mic_session_takes_the_shared_microphone() {
    let world = world(&["session.live", "audio.mic"], LiveEngine::new());
    let (client, events) = world.client();
    let session = session_of(start(&client, json!({"sources": ["mic"]})));
    let live = world.host.live_session().unwrap();
    assert_eq!((live.sources, live.captures_mic), (vec![Source::Mic], true));
    assert_eq!(world.mic.open_feeds(), 1);
    // `session_audio` is only for the source `client`.
    let pcm = sayso_sdk::protocol::pcm::encode_i16(&[0; 160]);
    assert_eq!(refused(client.request("session_audio", fields(json!({"session": session, "pcm": pcm})))).code, Some(ErrorCode::BadRequest));

    for frame in audio(12_000, &TWO).chunks(16 * 20) {
        world.mic.feed(frame);
    }
    assert_eq!(stop(&client, session), "w1 w2");
    assert_eq!(world.mic.open_feeds(), 0, "the session gave the microphone back");
    let segments = drain(&events).into_iter().filter(|e| matches!(e, HostEvent::Segment(s) if s.source == Some(Source::Mic))).count();
    assert_eq!(segments, 2);
}

#[test]
fn a_closed_connection_stops_its_session() {
    let world = world(&["session.live", "audio.mic"], LiveEngine::new());
    let (first, _) = world.client();
    session_of(start(&first, json!({"sources": ["client", "mic"]})));
    assert_eq!(world.engine.open_streams(), 4);
    drop(first);
    eventually("the session stops", || world.host.live_session().is_none());
    eventually("the streams close", || world.engine.open_streams() == 0);
    assert_eq!(world.mic.open_feeds(), 0);

    // The next session can start.
    let (second, _) = world.client();
    let session = session_of(start(&second, json!({"sources": ["client"]})));
    stop(&second, session);
}

#[test]
fn a_closing_host_stops_the_session() {
    let world = world(LIVE, LiveEngine::new());
    let (client, events) = world.client();
    session_of(start(&client, json!({"sources": ["client"]})));
    world.host.close();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(Event::HostGone) => break,
            Ok(_) => {}
            Err(_) => panic!("the client did not hear that the host closed"),
        }
    }
    eventually("the session stops", || world.host.live_session().is_none());
    eventually("the streams close", || world.engine.open_streams() == 0);
}

#[test]
fn a_full_engine_gives_too_many_streams() {
    let engine = LiveEngine { max_streams: 1, ..LiveEngine::new() };
    let world = world(LIVE, engine);
    let (client, _) = world.client();
    let error = refused(start(&client, json!({"sources": ["client"]})));
    assert_eq!(error.code, Some(ErrorCode::TooManyStreams), "{error:?}");
    assert_eq!(world.engine.open_streams(), 0, "the host closed the stream that it opened");
    assert_eq!(world.host.live_session(), None);
    // One stream is enough without a preview.
    let session = session_of(start(&client, json!({"sources": ["client"], "vad_model": "basic"})));
    stop(&client, session);
}

#[test]
fn a_session_that_cannot_run_is_refused_with_a_code() {
    let mut engine = LiveEngine::new();
    engine.on_disk.remove(&first_of(ModelTask::Diarization));
    let world = world(LIVE, engine);
    let (client, _) = world.client();
    let code = |body: serde_json::Value| refused(start(&client, body)).code;
    assert_eq!(code(json!({"sources": ["client"], "model": "no-such-model"})), Some(ErrorCode::UnknownModel));
    assert_eq!(code(json!({"sources": ["client"], "vad_model": streaming().as_str()})), Some(ErrorCode::WrongRole));
    assert_eq!(code(json!({"sources": ["client"], "diarize": true})), Some(ErrorCode::ModelNotDownloaded));
    assert_eq!(code(json!({"sources": ["client"], "record_to": "relative"})), Some(ErrorCode::BadRequest));
    assert_eq!(code(json!({"sources": []})), Some(ErrorCode::BadRequest));
    assert_eq!(world.host.live_session(), None);
}

#[test]
fn a_live_session_holds_sayso_open_until_it_ends() {
    let world = world(&["session.live", "audio.mic"], LiveEngine::new());
    // A record without a name: Sayso shows the id.
    let changes = Arc::new(AtomicUsize::new(0));
    {
        let changes = changes.clone();
        world.host.set_holds_observer(Arc::new(move || {
            changes.fetch_add(1, Ordering::SeqCst);
        }));
    }
    // The observer of the menu bar still hears the session too.
    let observer = Arc::new(Observer::default());
    world.host.set_live_observer(observer.clone());
    let (client, _events) = world.client();
    let session = session_of(start(&client, json!({"sources": ["mic"]})));
    let hold = sayso_host::Hold { app_id: id(), name: "Test".into(), work: sayso_host::Work::LiveSession };
    assert_eq!(world.host.holds(), vec![hold]);
    assert_eq!(changes.load(Ordering::SeqCst), 1);
    stop(&client, session);
    assert!(world.host.holds().is_empty());
    assert!(changes.load(Ordering::SeqCst) >= 2, "the stop and the end");
    assert_eq!(observer.0.lock().last(), Some(&None));

    // A connection that closes ends the hold too.
    session_of(start(&client, json!({"sources": ["client"]})));
    assert_eq!(world.host.holds().len(), 1);
    drop(client);
    eventually("the session of a closed connection ends", || world.host.holds().is_empty());
}

fn wait_for_end(events: &Receiver<Event>, session: u64) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match events.recv_timeout(left) {
            Ok(Event::Host { event: HostEvent::SessionEnded { session: ended, text, .. }, .. }) if ended == session => return text,
            Ok(_) => {}
            Err(_) => panic!("no session_ended for session {session}"),
        }
    }
}

#[test]
fn stop_in_sayso_ends_the_session_and_tells_the_app() {
    let world = world(LIVE, LiveEngine::new());
    let (client, events) = world.client();
    let session = session_of(start(&client, json!({"sources": ["client"]})));
    assert!(world.host.stop_live_session(session));
    assert!(!world.host.stop_live_session(session), "it stops once");
    wait_for_end(&events, session);
    eventually("the slot is free", || world.host.live_session().is_none());
    assert_eq!(refused(client.request("session_stop", fields(json!({"session": session})))).code, Some(ErrorCode::BadRequest), "the session is gone");
    let next = session_of(start(&client, json!({"sources": ["client"]})));
    assert_ne!(next, session);
    stop(&client, next);
}

#[test]
fn a_session_ends_when_the_user_takes_its_grant_back() {
    let world = world(LIVE, LiveEngine::new());
    let (client, events) = world.client();
    let session = session_of(start(&client, json!({"sources": ["client"]})));
    world.host.stop_work_without_grant();
    assert!(world.host.live_session().is_some_and(|live| !live.stopping), "the grant is still there");
    world.host.apps().set_grant(&id(), &Capability::SessionLive, false).unwrap();
    world.host.stop_work_without_grant();
    wait_for_end(&events, session);
    eventually("the slot is free", || world.host.live_session().is_none());
}

#[test]
fn a_grant_that_goes_during_the_load_stops_the_session_when_it_runs() {
    let engine = LiveEngine::new();
    let (started, load_started) = crossbeam_channel::bounded(1);
    let (release, released) = crossbeam_channel::bounded(1);
    *engine.load_gate.lock() = Some((started, released));
    let world = world(&["session.live", "audio.mic"], engine);
    let (client, events) = world.client();
    let starting = std::thread::spawn(move || {
        let answer = start(&client, json!({"sources": ["mic"]}));
        (client, answer)
    });
    load_started.recv_timeout(Duration::from_secs(5)).unwrap();

    world.host.apps().set_grant(&id(), &Capability::AudioMic, false).unwrap();
    world.host.stop_work_without_grant();
    assert_eq!(world.mic.open_feeds(), 0, "the session does not run yet");
    release.send(()).unwrap();

    let (_client, answer) = starting.join().unwrap();
    wait_for_end(&events, session_of(answer));
    eventually("the slot is free", || world.host.live_session().is_none());
    eventually("the microphone is off", || world.mic.open_feeds() == 0);
}

#[test]
fn a_mic_session_ends_when_session_live_goes() {
    let world = world(&["session.live", "audio.mic"], LiveEngine::new());
    let (client, events) = world.client();
    let session = session_of(start(&client, json!({"sources": ["mic"]})));
    assert_eq!(world.mic.open_feeds(), 1);
    world.host.apps().set_grant(&id(), &Capability::SessionLive, false).unwrap();
    world.host.stop_work_without_grant();
    wait_for_end(&events, session);
    eventually("the microphone is off", || world.mic.open_feeds() == 0);
}

#[test]
fn an_app_can_stop_its_own_session_after_a_grant_goes() {
    let world = world(LIVE, LiveEngine::new());
    let (client, _events) = world.client();
    let session = session_of(start(&client, json!({"sources": ["client"]})));
    world.host.apps().set_grant(&id(), &Capability::SessionLive, false).unwrap();
    let audio = fields(json!({"session": session, "pcm": ""}));
    assert_eq!(refused(client.request("session_audio", audio)).code, Some(ErrorCode::Capability), "no new audio");
    stop(&client, session);
    eventually("the slot is free", || world.host.live_session().is_none());
}
