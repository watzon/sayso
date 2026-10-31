//! Live sessions with a fake engine, a fake microphone, and a fake dictation.

use super::*;
use crate::basic_vad::BasicVad;
use crate::{EngineEvent, SpeechPart, Turn};
use sayso_protocol::SourceEndReason;
use crossbeam_channel::Sender;
use parking_lot::Condvar;
use sayso_protocol::{PartialEvent, Segment, SegmentEvent};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const RATE: usize = 16_000;

/// A fake engine. The preview stream sends a partial for each push. The VAD
/// session finds speech with Basic VAD. The final pass gives one segment for
/// the loud samples of a part, with the text `part<n>`. Diarization gives two
/// speakers: 1 until 3.1 s, then 0.
#[derive(Default)]
struct FakeEngine {
    state: Mutex<EngineState>,
    /// While true, a final pass waits.
    hold: Mutex<bool>,
    released: Condvar,
    watchers: Mutex<Vec<Sender<EngineEvent>>>,
    /// The most streams and VAD sessions open at the same time.
    max_streams: Option<usize>,
    /// The VAD session does not end its open speech at `end_vad`.
    vad_keeps_open_speech: bool,
    /// The text of the final pass: a loud part gives the first, and another part the second.
    texts: Option<(&'static str, &'static str)>,
}

#[derive(Default)]
struct EngineState {
    loads: Vec<ModelId>,
    streams: Vec<u64>,
    vads: HashMap<u64, BasicVad>,
    /// Every stream and VAD session that opened, in order.
    opened: Vec<u64>,
    pushed: HashMap<u64, usize>,
    transcribed: usize,
    in_final_pass: bool,
    diarized: Vec<usize>,
}

impl FakeEngine {
    fn emit(&self, event: EngineEvent) {
        self.watchers.lock().retain(|w| w.send(event.clone()).is_ok());
    }

    fn open_now(&self) -> usize {
        let state = self.state.lock();
        state.streams.len() + state.vads.len()
    }

    fn hold_final_pass(&self, hold: bool) {
        *self.hold.lock() = hold;
        self.released.notify_all();
    }

    fn emit_parts(&self, session: u64, parts: Vec<crate::basic_vad::Part>) {
        for part in parts {
            self.emit(EngineEvent::SpeechStart { session, start_ms: part.start_ms });
            self.emit(EngineEvent::SpeechEnd { session, start_ms: part.start_ms, end_ms: part.end_ms });
        }
    }
}

impl SpeechEngine for FakeEngine {
    fn load(&self, model: &ModelId) -> Result<(), EngineError> {
        self.state.lock().loads.push(model.clone());
        Ok(())
    }

    fn transcribe(&self, _samples: &[f32], _model: &ModelId, _language: &str) -> Result<String, EngineError> {
        Err(EngineError::new("a live session transcribes with segments"))
    }

    fn detect_speech(&self, _model: &ModelId, _samples: &[f32]) -> Result<Vec<SpeechPart>, EngineError> {
        Err(EngineError::new("a live session finds speech with a VAD session"))
    }

    fn start_stream(&self, session: u64, _model: &ModelId, _language: &str) -> Result<(), EngineError> {
        if self.max_streams.is_some_and(|max| self.open_now() >= max) {
            return Err(EngineError::with_code(ErrorCode::TooManyStreams, "8 streams are open; end one first"));
        }
        let mut state = self.state.lock();
        state.streams.push(session);
        state.opened.push(session);
        Ok(())
    }

    fn start_vad(&self, session: u64, _model: &ModelId) -> Result<(), EngineError> {
        if self.max_streams.is_some_and(|max| self.open_now() >= max) {
            return Err(EngineError::with_code(ErrorCode::TooManyStreams, "8 streams are open; end one first"));
        }
        let mut state = self.state.lock();
        state.vads.insert(session, BasicVad::new());
        state.opened.push(session);
        Ok(())
    }

    fn push_audio(&self, session: u64, samples: &[f32]) -> Result<(), EngineError> {
        let (preview, parts, total) = {
            let mut state = self.state.lock();
            let total = {
                let pushed = state.pushed.entry(session).or_default();
                *pushed += samples.len();
                *pushed
            };
            let preview = state.streams.contains(&session);
            let parts = state.vads.get_mut(&session).map(|vad| vad.accept(samples));
            (preview, parts, total)
        };
        if preview {
            self.emit(EngineEvent::Partial { session, committed: String::new(), tentative: format!("{} ms", total * 1000 / RATE) });
        }
        if let Some(parts) = parts {
            self.emit_parts(session, parts);
        }
        if !preview && !self.state.lock().vads.contains_key(&session) {
            return Err(EngineError::new(format!("session {session} is not open")));
        }
        Ok(())
    }

    fn end_stream(&self, session: u64) {
        self.state.lock().streams.retain(|&s| s != session);
    }

    fn end_vad(&self, session: u64) -> Result<(), EngineError> {
        let vad = self.state.lock().vads.remove(&session);
        let Some(vad) = vad else { return Err(EngineError::new("not open")) };
        if !self.vad_keeps_open_speech {
            self.emit_parts(session, vad.finish());
        }
        Ok(())
    }

    fn transcribe_segments(&self, samples: &[f32], _model: &ModelId, _language: &str) -> Result<Vec<Segment>, EngineError> {
        self.state.lock().in_final_pass = true;
        {
            let mut hold = self.hold.lock();
            while *hold {
                self.released.wait(&mut hold);
            }
        }
        let mut state = self.state.lock();
        state.in_final_pass = false;
        state.transcribed += 1;
        let loud: Vec<usize> = samples.iter().enumerate().filter(|(_, s)| s.abs() > 0.02).map(|(i, _)| i).collect();
        let (Some(first), Some(last)) = (loud.first(), loud.last()) else { return Ok(Vec::new()) };
        let text = match self.texts {
            Some((loud_text, quiet_text)) => (if samples.iter().any(|s| s.abs() > 0.4) { loud_text } else { quiet_text }).to_string(),
            None => format!("part{}", state.transcribed),
        };
        Ok(vec![Segment { start_ms: (first * 1000 / RATE) as u64, end_ms: (last * 1000 / RATE) as u64, text, words: None }])
    }

    fn diarize(&self, _model: &ModelId, samples: &[f32], _speakers: Option<u32>) -> Result<Vec<Turn>, EngineError> {
        self.state.lock().diarized.push(samples.len());
        Ok(vec![Turn { start_ms: 900, end_ms: 3100, speaker: 1 }, Turn { start_ms: 3900, end_ms: 5600, speaker: 0 }])
    }

    fn events(&self) -> crossbeam_channel::Receiver<EngineEvent> {
        let (tx, rx) = unbounded();
        self.watchers.lock().push(tx);
        rx
    }
}

#[derive(Default)]
struct Events(Mutex<Vec<Event>>);

impl EventSink for Events {
    fn event(&self, event: Event) {
        self.0.lock().push(event);
    }
}

impl Events {
    fn all(&self) -> Vec<Event> {
        self.0.lock().clone()
    }

    fn segments(&self) -> Vec<SegmentEvent> {
        self.all().into_iter().filter_map(|e| if let Event::Segment(s) = e { Some(s) } else { None }).collect()
    }

    fn partials(&self) -> Vec<PartialEvent> {
        self.all().into_iter().filter_map(|e| if let Event::Partial(p) = e { Some(p) } else { None }).collect()
    }
}

/// A dictation that holds the final passes while it runs. It counts the
/// engine calls of the session, for which jobs wait.
#[derive(Default)]
struct FakeDictation {
    running: Mutex<bool>,
    ended: Condvar,
    engine_calls: AtomicU64,
}

impl FakeDictation {
    fn set(&self, running: bool) {
        *self.running.lock() = running;
        self.ended.notify_all();
    }
}

impl Gate for FakeDictation {
    fn wait_idle(&self, stop: &dyn Fn() -> bool) -> bool {
        let mut running = self.running.lock();
        while *running && !stop() {
            self.ended.wait_for(&mut running, Duration::from_millis(20));
        }
        !stop()
    }

    fn engine_call(&self) -> Box<dyn Send + '_> {
        self.engine_calls.fetch_add(1, Ordering::SeqCst);
        Box::new(())
    }
}

/// Models stay where they are.
struct NoHold;

impl ModelRetention for NoHold {
    fn hold(&self, _models: Vec<ModelId>) -> Box<dyn Send> {
        Box::new(())
    }
}

/// A microphone whose frames the test sends.
#[derive(Default)]
struct FakeMic {
    feed: Mutex<Option<Sender<FeedItem>>>,
    opened: AtomicU64,
}

impl FakeMic {
    fn send(&self, samples: Vec<f32>) {
        let feed = self.feed.lock();
        feed.as_ref().expect("the microphone is open").send(FeedItem::Audio(CapturedAudio::untimed(samples))).unwrap();
    }

    fn is_open(&self) -> bool {
        self.feed.lock().is_some()
    }
}

/// Dropping it closes the feed, as the shared microphone of the app does.
struct FakeLease(Arc<FakeMic>);

impl Drop for FakeLease {
    fn drop(&mut self) {
        *self.0.feed.lock() = None;
    }
}

struct MicHandle(Arc<FakeMic>);

impl Microphone for MicHandle {
    fn open(&self) -> Result<AudioFeed, OpenError> {
        let (tx, rx) = unbounded();
        *self.0.feed.lock() = Some(tx);
        self.0.opened.fetch_add(1, Ordering::SeqCst);
        Ok(AudioFeed { items: rx, lease: Box::new(FakeLease(self.0.clone())), excludes: Vec::new() })
    }
}

/// The system audio, which the test steers.
#[derive(Default)]
struct FakeSystem {
    feed: Mutex<Option<Sender<FeedItem>>>,
    refuse: Mutex<Option<OpenError>>,
    app_pid: Mutex<Option<u32>>,
    /// While true, `open` waits, as for a question of the system.
    hold: AtomicBool,
    /// `open` is waiting.
    waiting: AtomicBool,
    /// `open` panics.
    panics: AtomicBool,
    /// How many times `open` started.
    opens: AtomicU64,
}

impl FakeSystem {
    fn send(&self, item: FeedItem) {
        self.feed.lock().as_ref().expect("the capture runs").send(item).unwrap();
    }

    fn audio(&self, samples: &[f32]) {
        for frame in samples.chunks(320) {
            self.send(FeedItem::Audio(CapturedAudio::untimed(frame.to_vec())));
        }
    }
}

struct SystemHandle(Arc<FakeSystem>);

/// Dropping it closes the feed.
struct SystemLease(Arc<FakeSystem>);

impl Drop for SystemLease {
    fn drop(&mut self) {
        *self.0.feed.lock() = None;
    }
}

impl SystemAudio for SystemHandle {
    fn open(&self, app_pid: Option<u32>) -> Result<AudioFeed, OpenError> {
        self.0.opens.fetch_add(1, Ordering::SeqCst);
        while self.0.hold.load(Ordering::SeqCst) {
            self.0.waiting.store(true, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!self.0.panics.load(Ordering::SeqCst), "a panic in the capture");
        if let Some(error) = self.0.refuse.lock().clone() {
            return Err(error);
        }
        *self.0.app_pid.lock() = app_pid;
        let (tx, rx) = unbounded();
        *self.0.feed.lock() = Some(tx);
        Ok(AudioFeed { items: rx, lease: Box::new(SystemLease(self.0.clone())), excludes: vec!["sayso".into(), "app".into()] })
    }
}

#[derive(Default)]
struct Observed(Mutex<Vec<Option<LiveInfo>>>);

impl LiveObserver for Observed {
    fn changed(&self, live: Option<&LiveInfo>) {
        self.0.lock().push(live.cloned());
    }
}

struct Fixture {
    engine: Arc<FakeEngine>,
    mic: Arc<FakeMic>,
    system: Arc<FakeSystem>,
    dictation: Arc<FakeDictation>,
    sessions: LiveSessions,
    temp: tempfile::TempDir,
}

fn fixture_with(engine: FakeEngine) -> Fixture {
    let engine = Arc::new(engine);
    let mic = Arc::new(FakeMic::default());
    let system = Arc::new(FakeSystem::default());
    let dictation = Arc::new(FakeDictation::default());
    let temp = tempfile::TempDir::new().unwrap();
    let sessions = LiveSessions::new(LiveDeps {
        engine: engine.clone(),
        microphone: Some(Arc::new(MicHandle(mic.clone()))),
        system_audio: Some(Arc::new(SystemHandle(system.clone()))),
        gate: dictation.clone(),
        models: Arc::new(NoHold),
        recordings: Arc::new(RecordingFolder::new(temp.path().join("live"))),
    });
    Fixture { engine, mic, system, dictation, sessions, temp }
}

fn fixture() -> Fixture {
    fixture_with(FakeEngine::default())
}

fn owner(connection: u64) -> Owner {
    Owner { connection, app_id: "dev.example.notetaker".into(), pid: None }
}

fn spec(sources: &[Source]) -> SessionSpec {
    SessionSpec {
        sources: sources.to_vec(),
        final_model: ModelId::new("final"),
        preview_model: Some(ModelId::new("streaming")),
        vad: VadRun::Basic,
        diarization: None,
        language: "en".into(),
        record_to: None,
    }
}

fn silence(ms: usize) -> Vec<f32> {
    vec![0.0; ms * RATE / 1000]
}

/// Something like speech, about -20 dB: a voice of 140 Hz with harmonics in
/// syllables of 200 ms.
fn speech(ms: usize) -> Vec<f32> {
    (0..ms * RATE / 1000)
        .map(|n| {
            let t = n as f32 / RATE as f32;
            let voice: f32 = (1..=5).map(|h| (2.0 * std::f32::consts::PI * 140.0 * h as f32 * t).sin() / h as f32).sum();
            let syllable = (t * 5.0).fract();
            let envelope = if syllable < 0.85 { (std::f32::consts::PI * syllable / 0.85).sin().max(0.3) } else { 0.05 };
            0.1 * voice * envelope
        })
        .collect()
}

/// The audio as the 16-bit PCM of `session_audio`.
fn pcm(samples: &[f32]) -> Vec<i16> {
    samples.iter().map(|&s| wav::to_i16(s)).collect()
}

/// Send the audio in parts of 160 ms, as an extension does.
fn send_client(sessions: &LiveSessions, connection: u64, session: u64, samples: &[i16]) {
    for chunk in samples.chunks(RATE * 160 / 1000) {
        sessions.audio(connection, session, &sayso_protocol::pcm::encode_i16(chunk)).unwrap();
    }
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// True when `ms` is within `slack` of `want`.
fn near(ms: u64, want: u64, slack: u64) -> bool {
    ms.abs_diff(want) <= slack
}

#[test]
fn a_client_session_gives_partials_segments_a_recording_and_speakers() {
    let f = fixture();
    let record_to = f.temp.path().join("meeting");
    let sink = Arc::new(Events::default());
    let observed = Arc::new(Observed::default());
    f.sessions.set_observer(observed.clone());
    let spec = SessionSpec { diarization: Some(ModelId::new("speaker-diarization")), record_to: Some(record_to.clone()), ..spec(&[Source::Client]) };
    let session = f.sessions.start(owner(1), spec, sink.clone()).unwrap().session;
    assert_eq!(f.sessions.current().map(|l| (l.session, l.app_id, l.captures_mic)), Some((session, "dev.example.notetaker".to_string(), false)));

    let audio = pcm(&[silence(1000), speech(2000), silence(1000), speech(1500), silence(1000)].concat());
    send_client(&f.sessions, 1, session, &audio);
    // The first part ends at a pause, and its segment comes while the session runs.
    eventually("the first segment comes before the stop", || !sink.segments().is_empty());
    let text = f.sessions.stop(1, session).unwrap();
    assert_eq!(text, "part1 part2");

    let partials = sink.partials();
    assert!(!partials.is_empty());
    assert!(partials.iter().all(|p| p.session == session && p.source == Some(Source::Client)));
    assert_eq!(partials.last().unwrap().tentative, "6500 ms", "the preview stream got all the audio");

    let segments = sink.segments();
    assert_eq!(segments.len(), 2, "{segments:?}");
    assert!(segments.iter().all(|s| s.session == Some(session) && s.source == Some(Source::Client) && s.job.is_none()));
    // The times are from the start of the session, not from the start of the part.
    assert!(near(segments[0].segment.start_ms, 1000, 60) && near(segments[0].segment.end_ms, 3000, 60), "{segments:?}");
    assert!(near(segments[1].segment.start_ms, 4000, 60) && near(segments[1].segment.end_ms, 5500, 60), "{segments:?}");

    // The recording has every sample that the extension sent.
    let path = record_to.join("client.wav");
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.len(), 44 + audio.len() * 2);
    assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize, audio.len() * 2);
    assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 16_000);
    assert_eq!(pcm(&wav::read(&path).unwrap()), audio);
    assert_eq!(f.engine.state.lock().diarized, vec![audio.len()], "diarization ran on the whole recording");

    // Speakers come after the last segment. Each segment has one speaker, and
    // the turns follow the segments, with labels in the order of first speech.
    let events = sink.all();
    let speakers = events.iter().position(|e| matches!(e, Event::Speakers(_))).unwrap();
    assert!(events[speakers + 1..].iter().all(|e| !matches!(e, Event::Segment(_) | Event::Partial(_))));
    let Event::Speakers(speakers) = &events[speakers] else { unreachable!() };
    assert_eq!((speakers.session, speakers.source.clone(), speakers.job), (Some(session), Some(Source::Client), None));
    let labels: Vec<(u64, &str)> = speakers.turns.iter().map(|t| (t.start_ms, t.speaker.as_str())).collect();
    assert_eq!(labels, vec![(segments[0].segment.start_ms, "1"), (segments[1].segment.start_ms, "2")]);

    // The engine sessions are closed, the slot is free, and the app heard both changes.
    assert_eq!(f.engine.open_now(), 0);
    assert_eq!(f.sessions.current(), None);
    let observed = observed.0.lock().clone();
    assert_eq!(observed.first().cloned().flatten().map(|l| l.session), Some(session));
    assert_eq!(observed.last(), Some(&None));
    let loads = f.engine.state.lock().loads.clone();
    assert_eq!(loads, vec![ModelId::new("final"), ModelId::new("streaming"), ModelId::new("speaker-diarization")]);
    // Jobs waited during the two final passes and the diarization.
    assert_eq!(f.dictation.engine_calls.load(Ordering::SeqCst), 3);
}

#[test]
fn a_second_session_is_busy_until_the_first_ends() {
    let f = fixture();
    let first = f.sessions.start(owner(1), spec(&[Source::Client]), Arc::new(Events::default())).unwrap().session;
    let busy = f.sessions.start(owner(2), spec(&[Source::Client]), Arc::new(Events::default())).unwrap_err();
    assert_eq!(busy.code, Some(ErrorCode::Busy));
    // Another connection cannot stop it or send it audio.
    assert_eq!(f.sessions.stop(2, first).unwrap_err().code, Some(ErrorCode::BadRequest));
    assert_eq!(f.sessions.audio(2, first, "").unwrap_err().code, Some(ErrorCode::BadRequest));
    assert_eq!(f.sessions.stop(1, first).unwrap(), "");
    let second = f.sessions.start(owner(2), spec(&[Source::Client]), Arc::new(Events::default())).unwrap().session;
    assert_ne!(first, second);
    assert_eq!(f.sessions.stop(1, second).unwrap_err().code, Some(ErrorCode::BadRequest), "the first connection does not own it");
    f.sessions.stop(2, second).unwrap();
}

#[test]
fn engine_session_ids_are_above_the_ids_of_dictation_and_unique() {
    let f = fixture();
    let spec = SessionSpec { vad: VadRun::Model(ModelId::new("silero-vad")), ..spec(&[Source::Mic, Source::Client]) };
    for _ in 0..2 {
        let session = f.sessions.start(owner(1), spec.clone(), Arc::new(Events::default())).unwrap().session;
        assert_eq!(f.engine.open_now(), 4, "a preview stream and a VAD session for each source");
        f.sessions.stop(1, session).unwrap();
    }
    let opened = f.engine.state.lock().opened.clone();
    assert_eq!(opened.len(), 8);
    assert!(opened.iter().all(|&id| id >= ENGINE_SESSION_BASE), "{opened:?}");
    let mut unique = opened.clone();
    unique.dedup();
    assert_eq!(unique, opened);
    assert_eq!(f.engine.open_now(), 0);
}

#[test]
fn a_full_engine_gives_too_many_streams_and_closes_what_opened() {
    let f = fixture_with(FakeEngine { max_streams: Some(3), ..FakeEngine::default() });
    let spec = SessionSpec { vad: VadRun::Model(ModelId::new("silero-vad")), ..spec(&[Source::Mic, Source::Client]) };
    let error = f.sessions.start(owner(1), spec, Arc::new(Events::default())).unwrap_err();
    assert_eq!(error.code, Some(ErrorCode::TooManyStreams), "{error:?}");
    assert_eq!(f.engine.open_now(), 0, "the streams of the failed start are closed");
    assert!(!f.mic.is_open(), "the microphone is given back");
    assert_eq!(f.sessions.current(), None);
    assert!(std::fs::read_dir(f.temp.path().join("live")).unwrap().next().is_none(), "no recording is left");
}

#[test]
fn sources_are_checked() {
    let f = fixture();
    let start = |sources: &[Source]| f.sessions.start(owner(1), spec(sources), Arc::new(Events::default())).unwrap_err().code;
    assert_eq!(start(&[]), Some(ErrorCode::BadRequest));
    assert_eq!(start(&[Source::Client, Source::Client]), Some(ErrorCode::BadRequest));
    assert_eq!(start(&[Source::System, Source::System]), Some(ErrorCode::BadRequest));
    assert_eq!(start(&[Source::Other("camera".into())]), Some(ErrorCode::BadRequest));
    let relative = SessionSpec { record_to: Some(PathBuf::from("notes")), ..spec(&[Source::Client]) };
    assert_eq!(f.sessions.start(owner(1), relative, Arc::new(Events::default())).unwrap_err().code, Some(ErrorCode::BadRequest));
    assert_eq!(f.sessions.current(), None, "a refused start leaves the slot free");
}

#[test]
fn a_mic_session_records_to_a_temporary_file_that_the_stop_deletes() {
    let f = fixture();
    let sink = Arc::new(Events::default());
    let session = f.sessions.start(owner(1), spec(&[Source::Mic]), sink.clone()).unwrap().session;
    assert!(f.sessions.current().unwrap().captures_mic);
    let audio = [silence(500), speech(1500), silence(800)].concat();
    for frame in audio.chunks(320) {
        f.mic.send(frame.to_vec());
    }
    let recording = std::fs::read_dir(f.temp.path().join("live")).unwrap().map(|e| e.unwrap().path()).collect::<Vec<_>>();
    assert_eq!(recording.len(), 1);
    assert!(recording[0].to_string_lossy().ends_with("mic.wav"), "{recording:?}");
    assert_eq!(f.sessions.stop(1, session).unwrap(), "part1");
    assert!(!f.mic.is_open(), "the stop gives the microphone back");
    assert!(!recording[0].exists(), "the temporary recording is deleted");
    let segments = sink.segments();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].source, Some(Source::Mic));
    assert!(near(segments[0].segment.start_ms, 500, 60), "{segments:?}");
}

#[test]
fn basic_vad_cuts_parts_at_pauses_and_the_stop_ends_the_open_part() {
    let f = fixture();
    let sink = Arc::new(Events::default());
    let session = f.sessions.start(owner(1), SessionSpec { preview_model: None, ..spec(&[Source::Client]) }, sink.clone()).unwrap().session;
    // Speech, a pause of 300 ms that does not end it, a pause of 1 s that does,
    // and speech that still runs at the stop.
    let audio = pcm(&[silence(500), speech(1000), silence(300), speech(700), silence(1000), speech(1200)].concat());
    send_client(&f.sessions, 1, session, &audio);
    assert_eq!(f.sessions.stop(1, session).unwrap(), "part1 part2");
    let segments: Vec<(u64, u64)> = sink.segments().iter().map(|s| (s.segment.start_ms, s.segment.end_ms)).collect();
    assert_eq!(segments.len(), 2, "{segments:?}");
    assert!(near(segments[0].0, 500, 60) && near(segments[0].1, 2500, 60), "{segments:?}");
    assert!(near(segments[1].0, 3500, 60) && near(segments[1].1, 4700, 60), "{segments:?}");
    assert!(sink.partials().is_empty(), "no preview model, no partials");
}

#[test]
fn an_engine_vad_gives_the_parts_and_the_stop_ends_speech_that_is_open() {
    let f = fixture_with(FakeEngine { vad_keeps_open_speech: true, ..FakeEngine::default() });
    let sink = Arc::new(Events::default());
    let spec = SessionSpec { vad: VadRun::Model(ModelId::new("silero-vad")), ..spec(&[Source::Client]) };
    let session = f.sessions.start(owner(1), spec, sink.clone()).unwrap().session;
    let audio = pcm(&[silence(500), speech(1000), silence(1000), speech(800)].concat());
    send_client(&f.sessions, 1, session, &audio);
    // The engine reports the start of the speech that is open, and never its end.
    let vad = *f.engine.state.lock().vads.keys().next().unwrap();
    eventually("the first part", || !sink.segments().is_empty());
    f.engine.emit(EngineEvent::SpeechStart { session: vad, start_ms: 2500 });
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(f.sessions.stop(1, session).unwrap(), "part1 part2");
    let segments: Vec<(u64, u64)> = sink.segments().iter().map(|s| (s.segment.start_ms, s.segment.end_ms)).collect();
    assert!(near(segments[0].0, 500, 60) && near(segments[0].1, 1500, 60), "{segments:?}");
    assert!(near(segments[1].0, 2500, 60) && near(segments[1].1, 3300, 60), "the open speech goes to the end of the audio: {segments:?}");
    assert!(f.engine.state.lock().loads.contains(&ModelId::new("silero-vad")));
}

#[test]
fn a_stop_while_a_part_is_transcribed_waits_for_it_and_for_the_last_part() {
    let f = fixture();
    let sink = Arc::new(Events::default());
    let session = f.sessions.start(owner(1), spec(&[Source::Client]), sink.clone()).unwrap().session;
    f.engine.hold_final_pass(true);
    send_client(&f.sessions, 1, session, &pcm(&[silence(300), speech(1000), silence(1000)].concat()));
    eventually("the final pass of the first part runs", || f.engine.state.lock().in_final_pass);
    send_client(&f.sessions, 1, session, &pcm(&speech(800)));
    let stopping = {
        let sessions = f.sessions.clone();
        std::thread::spawn(move || sessions.stop(1, session))
    };
    std::thread::sleep(Duration::from_millis(200));
    assert!(!stopping.is_finished(), "the stop waits for the final pass");
    assert!(f.sessions.current().unwrap().stopping);
    assert_eq!(f.sessions.audio(1, session, "").unwrap_err().code, Some(ErrorCode::BadRequest), "no audio after the stop");
    assert!(f.sessions.start(owner(2), spec(&[Source::Client]), Arc::new(Events::default())).unwrap_err().code == Some(ErrorCode::Busy));
    f.engine.hold_final_pass(false);
    assert_eq!(stopping.join().unwrap().unwrap(), "part1 part2");
    assert_eq!(sink.segments().len(), 2);
    assert_eq!(f.sessions.current(), None);
}

#[test]
fn final_passes_wait_while_a_dictation_runs_but_the_preview_does_not() {
    let f = fixture();
    let sink = Arc::new(Events::default());
    let session = f.sessions.start(owner(1), spec(&[Source::Client]), sink.clone()).unwrap().session;
    f.dictation.set(true);
    send_client(&f.sessions, 1, session, &pcm(&[speech(1000), silence(1000)].concat()));
    eventually("the preview goes on", || sink.partials().len() >= 12);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(f.engine.state.lock().transcribed, 0, "the part waits for the dictation");
    f.dictation.set(false);
    eventually("the part runs after the dictation", || sink.segments().len() == 1);
    f.sessions.stop(1, session).unwrap();
}

#[test]
fn a_closed_connection_stops_its_session_without_diarization() {
    let f = fixture();
    let sink = Arc::new(Events::default());
    let spec = SessionSpec { diarization: Some(ModelId::new("speaker-diarization")), ..spec(&[Source::Mic]) };
    let session = f.sessions.start(owner(7), spec, sink.clone()).unwrap().session;
    f.mic.send(speech(500));
    f.sessions.connection_closed(3);
    assert!(f.sessions.current().is_some(), "another connection does not stop it");
    f.sessions.connection_closed(7);
    eventually("the session ends", || f.sessions.current().is_none());
    assert!(!f.mic.is_open());
    assert_eq!(f.engine.open_now(), 0);
    assert!(f.engine.state.lock().diarized.is_empty());
    assert!(!sink.all().iter().any(|e| matches!(e, Event::Speakers(_))));
    assert!(std::fs::read_dir(f.temp.path().join("live")).unwrap().next().is_none(), "the temporary recording is deleted");
    assert_eq!(f.sessions.stop(7, session).unwrap_err().code, Some(ErrorCode::BadRequest));
}

#[test]
fn a_host_without_system_audio_gets_unsupported() {
    let engine = Arc::new(FakeEngine::default());
    let temp = tempfile::TempDir::new().unwrap();
    let sessions = LiveSessions::new(LiveDeps {
        engine,
        microphone: None,
        system_audio: None,
        gate: Arc::new(FakeDictation::default()),
        models: Arc::new(NoHold),
        recordings: Arc::new(RecordingFolder::new(temp.path().join("live"))),
    });
    for source in [Source::System, Source::Mic] {
        let error = sessions.start(owner(1), spec(&[source]), Arc::new(Events::default())).unwrap_err();
        assert_eq!(error.code, Some(ErrorCode::Unsupported));
    }
}

const THE_CALL: &str = "we should ship the release on friday if the tests pass";
const THE_USER: &str = "i can take that one after lunch";

/// Louder than the 0.4 of the fake engine: the words of the call.
fn loud(ms: usize) -> Vec<f32> {
    speech(ms).into_iter().map(|s| s * 3.0).collect()
}

fn echo_fixture() -> Fixture {
    fixture_with(FakeEngine { texts: Some((THE_CALL, THE_USER)), ..FakeEngine::default() })
}

fn no_preview(sources: &[Source]) -> SessionSpec {
    SessionSpec { preview_model: None, ..spec(sources) }
}

fn mic_audio(f: &Fixture, samples: &[f32]) {
    for frame in samples.chunks(320) {
        f.mic.send(frame.to_vec());
    }
}

#[test]
fn a_microphone_segment_that_the_system_covers_is_dropped() {
    let f = echo_fixture();
    let sink = Arc::new(Events::default());
    let session = f.sessions.start(owner(1), no_preview(&[Source::Mic, Source::System]), sink.clone()).unwrap();
    assert_eq!(session.excludes, ["sayso", "app"]);
    // The call plays, and the microphone hears it.
    let audio = [silence(500), loud(2000), silence(1000)].concat();
    f.system.audio(&audio);
    mic_audio(&f, &audio);
    assert_eq!(f.sessions.stop(1, session.session).unwrap(), THE_CALL, "one copy of the sentence");
    let segments = sink.segments();
    assert_eq!(segments.len(), 1, "{segments:?}");
    assert_eq!(segments[0].source, Some(Source::System));
}

#[test]
fn the_own_words_of_the_user_stay_while_the_call_speaks() {
    let f = echo_fixture();
    let sink = Arc::new(Events::default());
    let session = f.sessions.start(owner(1), no_preview(&[Source::Mic, Source::System]), sink.clone()).unwrap();
    f.system.audio(&[silence(500), loud(2000), silence(1000)].concat());
    mic_audio(&f, &[silence(500), speech(2000), silence(1000)].concat());
    let text = f.sessions.stop(1, session.session).unwrap();
    assert!(text.contains(THE_CALL) && text.contains(THE_USER), "{text}");
    assert_eq!(sink.segments().len(), 2);
}

#[test]
fn a_held_segment_comes_while_the_session_runs_when_the_system_catches_up() {
    let f = echo_fixture();
    let sink = Arc::new(Events::default());
    let session = f.sessions.start(owner(1), no_preview(&[Source::Mic, Source::System]), sink.clone()).unwrap();
    mic_audio(&f, &[silence(200), speech(1000), silence(1000)].concat());
    eventually("the final pass of the microphone ran", || f.engine.state.lock().transcribed == 1);
    // The system source hears silence, and its time goes past the segment.
    f.system.audio(&silence(4000));
    eventually("the segment comes", || {
        f.system.audio(&silence(20));
        !sink.segments().is_empty()
    });
    assert_eq!(sink.segments()[0].source, Some(Source::Mic));
    assert_eq!(f.sessions.stop(1, session.session).unwrap(), THE_USER);
}

#[test]
fn a_session_with_one_captured_source_has_no_echo_rule() {
    let f = echo_fixture();
    let sink = Arc::new(Events::default());
    let session = f.sessions.start(owner(1), no_preview(&[Source::Mic]), sink.clone()).unwrap();
    mic_audio(&f, &[silence(200), speech(1000), silence(1000)].concat());
    eventually("the segment comes at once", || !sink.segments().is_empty());
    f.sessions.stop(1, session.session).unwrap();
}

#[test]
fn a_gap_gives_an_event_and_silence_so_the_times_go_on() {
    let f = fixture();
    let sink = Arc::new(Events::default());
    let record_to = f.temp.path().join("meeting");
    let session = f.sessions.start(owner(1), SessionSpec { record_to: Some(record_to.clone()), ..no_preview(&[Source::System]) }, sink.clone()).unwrap().session;
    f.system.audio(&silence(500));
    f.system.send(FeedItem::Gap { ms: 1000 });
    f.system.audio(&[speech(1000), silence(1000)].concat());
    f.sessions.stop(1, session).unwrap();
    assert!(sink.all().contains(&Event::SourceGap { session, source: Source::System, ms: 1000 }));
    let segments = sink.segments();
    assert!(near(segments[0].segment.start_ms, 1500, 60), "the speech comes after the gap: {segments:?}");
    assert_eq!(wav::read(&record_to.join("system.wav")).unwrap().len(), 3500 * RATE / 1000, "the recording has the silence");
}

#[test]
fn a_very_long_gap_does_not_write_all_of_its_silence() {
    let f = fixture();
    let sink = Arc::new(Events::default());
    let record_to = f.temp.path().join("meeting");
    let session = f.sessions.start(owner(1), SessionSpec { record_to: Some(record_to.clone()), ..no_preview(&[Source::System]) }, sink.clone()).unwrap().session;
    f.system.send(FeedItem::Gap { ms: 8 * 3_600_000 });
    f.sessions.stop(1, session).unwrap();
    assert_eq!(wav::read(&record_to.join("system.wav")).unwrap().len(), 60 * RATE, "at most a minute");
}

#[test]
fn a_source_that_ends_tells_the_app_after_its_last_segment_and_the_others_go_on() {
    let f = echo_fixture();
    let sink = Arc::new(Events::default());
    let session = f.sessions.start(owner(1), no_preview(&[Source::Mic, Source::System]), sink.clone()).unwrap().session;
    f.system.audio(&[silence(200), loud(1000)].concat());
    f.system.send(FeedItem::Ended(SourceEnd::Permission));
    let ended = Event::SourceEnded { session, source: Source::System, reason: SourceEndReason::Permission };
    eventually("the source ends", || sink.all().contains(&ended));
    let events = sink.all();
    let at = events.iter().position(|e| *e == ended).unwrap();
    assert!(events[..at].iter().any(|e| matches!(e, Event::Segment(s) if s.source == Some(Source::System))), "the last segment comes first: {events:?}");

    // The microphone goes on, and its segments no longer wait.
    assert!(f.sessions.current().is_some());
    mic_audio(&f, &[silence(10_000), speech(1000), silence(1000)].concat());
    eventually("the microphone segment comes", || sink.segments().iter().any(|s| s.source == Some(Source::Mic)));
    let text = f.sessions.stop(1, session).unwrap();
    assert!(text.contains(THE_CALL) && text.contains(THE_USER), "{text}");
}

#[test]
fn a_capture_that_fails_for_good_ends_with_the_reason_failed() {
    let f = fixture();
    let sink = Arc::new(Events::default());
    let session = f.sessions.start(owner(1), no_preview(&[Source::System]), sink.clone()).unwrap().session;
    f.system.send(FeedItem::Ended(SourceEnd::Failed("the sound server is gone".into())));
    let ended = Event::SourceEnded { session, source: Source::System, reason: SourceEndReason::Failed };
    eventually("the source ends", || sink.all().contains(&ended));
    f.sessions.stop(1, session).unwrap();
}

#[test]
fn a_refused_permission_and_an_old_system_stop_the_start_and_close_what_opened() {
    let f = fixture();
    for (error, code) in [(OpenError::Permission("refused".into()), Some(ErrorCode::Permission)), (OpenError::Unsupported("macOS 14.1".into()), Some(ErrorCode::Unsupported)), (OpenError::Failed("no device".into()), None)] {
        *f.system.refuse.lock() = Some(error);
        let refused = f.sessions.start(owner(1), spec(&[Source::Mic, Source::System]), Arc::new(Events::default())).unwrap_err();
        assert_eq!(refused.code, code, "{refused:?}");
        assert_eq!(f.engine.open_now(), 0);
        assert!(!f.mic.is_open(), "the microphone is given back");
        assert_eq!(f.sessions.current(), None);
    }
}

#[test]
fn the_pid_of_the_app_goes_to_the_capture() {
    let f = fixture();
    let session = f.sessions.start(Owner { connection: 1, app_id: "app".into(), pid: Some(4242) }, no_preview(&[Source::System]), Arc::new(Events::default())).unwrap().session;
    assert_eq!(*f.system.app_pid.lock(), Some(4242));
    f.sessions.stop(1, session).unwrap();
}

#[test]
fn a_source_that_starts_late_gets_silence_so_the_sources_share_their_times() {
    let f = fixture();
    let sink = Arc::new(Events::default());
    let record_to = f.temp.path().join("meeting");
    let session = f.sessions.start(owner(1), SessionSpec { record_to: Some(record_to.clone()), ..no_preview(&[Source::Mic, Source::System]) }, sink.clone()).unwrap().session;
    let at = |ms: u64, samples: Vec<f32>| FeedItem::Audio(CapturedAudio { samples, time: Some(Duration::from_millis(5_000 + ms)) });
    f.mic.feed.lock().as_ref().unwrap().send(at(0, silence(1000))).unwrap();
    // The source handles its items in order, so this event says that it has the first audio.
    f.mic.feed.lock().as_ref().unwrap().send(FeedItem::Gap { ms: 0 }).unwrap();
    eventually("the first audio of the microphone arrives", || sink.all().iter().any(|e| matches!(e, Event::SourceGap { .. })));
    // The first audio of the system comes 300 ms after the first of the microphone.
    f.system.send(at(300, silence(700)));
    f.sessions.stop(1, session).unwrap();
    assert_eq!(wav::read(&record_to.join("mic.wav")).unwrap().len(), RATE);
    assert_eq!(wav::read(&record_to.join("system.wav")).unwrap().len(), RATE, "300 ms of silence and 700 ms of audio");
}

/// A start on a thread of its own that waits in the capture of the system audio.
fn start_that_waits(f: &Fixture, connection: u64, spec: SessionSpec) -> std::thread::JoinHandle<Result<Started, LiveError>> {
    f.system.hold.store(true, Ordering::SeqCst);
    let sessions = f.sessions.clone();
    let start = std::thread::spawn(move || sessions.start(owner(connection), spec, Arc::new(Events::default())));
    eventually("the capture waits", || f.system.waiting.load(Ordering::SeqCst));
    start
}

#[test]
fn a_start_whose_connection_closes_ends_when_the_question_ends_and_frees_the_slot() {
    let f = fixture();
    let observed = Arc::new(Observed::default());
    f.sessions.set_observer(observed.clone());
    let start = start_that_waits(&f, 1, spec(&[Source::Mic, Source::System]));

    f.sessions.connection_closed(2);
    f.sessions.connection_closed(1);
    let other = f.sessions.start(owner(3), spec(&[Source::Client]), Arc::new(Events::default())).unwrap_err();
    assert_eq!(other.code, Some(ErrorCode::Busy), "the slot stays taken while the question is open");

    f.system.hold.store(false, Ordering::SeqCst);
    let error = start.join().unwrap().unwrap_err();
    assert!(error.message.contains("closed"), "{error:?}");
    assert_eq!(f.system.feed.lock().is_some(), false, "the capture closed");
    assert_eq!(f.mic.opened.load(Ordering::SeqCst), 0, "the microphone did not open");
    assert_eq!(f.engine.open_now(), 0);
    assert_eq!(f.sessions.current(), None);
    assert!(observed.0.lock().is_empty(), "no recording state for a session that did not start");
    let next = f.sessions.start(owner(3), spec(&[Source::Client]), Arc::new(Events::default())).unwrap();
    f.sessions.stop(3, next.session).unwrap();
}

#[test]
fn a_cancel_of_another_connection_does_not_end_a_start() {
    let f = fixture();
    let start = start_that_waits(&f, 1, spec(&[Source::System]));
    f.sessions.connection_closed(2);
    f.system.hold.store(false, Ordering::SeqCst);
    let started = start.join().unwrap().unwrap();
    f.sessions.stop(1, started.session).unwrap();
}

#[test]
fn a_close_during_a_start_ends_it_and_no_session_starts_after_it() {
    let f = fixture();
    let start = start_that_waits(&f, 1, spec(&[Source::System]));
    f.sessions.close();
    f.system.hold.store(false, Ordering::SeqCst);
    assert!(start.join().unwrap().is_err());
    assert!(f.system.feed.lock().is_none());
    assert_eq!(f.sessions.current(), None);
    assert!(f.sessions.start(owner(1), spec(&[Source::Client]), Arc::new(Events::default())).is_err(), "Sayso closes");
}

#[test]
fn the_microphone_waits_for_the_question_of_the_system_also_when_it_is_first_in_the_list() {
    let f = fixture();
    let start = start_that_waits(&f, 1, spec(&[Source::Mic, Source::System]));
    assert_eq!(f.mic.opened.load(Ordering::SeqCst), 0, "no microphone while the user answers");
    f.system.hold.store(false, Ordering::SeqCst);
    let started = start.join().unwrap().unwrap();
    assert_eq!(f.mic.opened.load(Ordering::SeqCst), 1);
    assert_eq!(f.sessions.current().map(|live| live.captures_mic), Some(true));
    f.mic.send(silence(100));
    f.sessions.stop(1, started.session).unwrap();
}

#[test]
fn a_refused_system_audio_opens_no_microphone_and_leaves_no_recording() {
    let f = fixture();
    let folder = f.temp.path().join("recordings");
    *f.system.refuse.lock() = Some(OpenError::Permission("refused".into()));
    let spec = SessionSpec { record_to: Some(folder.clone()), ..spec(&[Source::Mic, Source::System]) };
    let error = f.sessions.start(owner(1), spec, Arc::new(Events::default())).unwrap_err();
    assert_eq!(error.code, Some(ErrorCode::Permission));
    assert_eq!(f.mic.opened.load(Ordering::SeqCst), 0);
    assert_eq!(f.engine.open_now(), 0);
    assert!(std::fs::read_dir(&folder).unwrap().next().is_none(), "no WAV stays");
}

#[test]
fn a_start_that_fails_late_deletes_the_recordings_that_it_made() {
    let f = fixture_with(FakeEngine { max_streams: Some(1), ..FakeEngine::default() });
    let folder = f.temp.path().join("recordings");
    let spec = SessionSpec { record_to: Some(folder.clone()), ..spec(&[Source::Client, Source::Mic]) };
    let error = f.sessions.start(owner(1), spec, Arc::new(Events::default())).unwrap_err();
    assert_eq!(error.code, Some(ErrorCode::TooManyStreams), "{error:?}");
    assert!(std::fs::read_dir(&folder).unwrap().next().is_none(), "no WAV stays");
}

#[test]
fn a_panic_in_a_start_frees_the_slot() {
    let f = fixture();
    f.system.panics.store(true, Ordering::SeqCst);
    let sessions = f.sessions.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sessions.start(owner(1), spec(&[Source::System]), Arc::new(Events::default()))));
    assert!(result.is_err());
    f.system.panics.store(false, Ordering::SeqCst);
    let started = f.sessions.start(owner(1), spec(&[Source::Client]), Arc::new(Events::default())).expect("the slot is free");
    f.sessions.stop(1, started.session).unwrap();
}

#[cfg(unix)]
#[test]
fn a_recording_is_for_this_user_only() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("mic.wav");
    wav::WavWriter::create(&path).unwrap().finish().unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
}
