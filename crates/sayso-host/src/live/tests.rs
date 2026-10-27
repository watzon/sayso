//! Live sessions with a fake engine, a fake microphone, and a fake dictation.

use super::*;
use crossbeam_channel::Sender;
use parking_lot::Condvar;
use sayso_core::basic_vad::BasicVad;
use sayso_core::stt::{EngineEvent, Turn};
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

    fn emit_parts(&self, session: u64, parts: Vec<sayso_core::basic_vad::Part>) {
        for part in parts {
            self.emit(EngineEvent::SpeechStart { session, start_ms: part.start_ms });
            self.emit(EngineEvent::SpeechEnd { session, start_ms: part.start_ms, end_ms: part.end_ms });
        }
    }
}

impl LiveEngine for FakeEngine {
    fn load(&self, model: &ModelId) -> Result<(), EngineError> {
        self.state.lock().loads.push(model.clone());
        Ok(())
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
        Ok(vec![Segment { start_ms: (first * 1000 / RATE) as u64, end_ms: (last * 1000 / RATE) as u64, text: format!("part{}", state.transcribed), words: None }])
    }

    fn diarize(&self, _model: &ModelId, samples: &[f32]) -> Result<Vec<Turn>, EngineError> {
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

impl Priority for FakeDictation {
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

/// A microphone whose frames the test sends.
#[derive(Default)]
struct FakeMic {
    feed: Mutex<Option<Sender<Vec<f32>>>>,
    opened: AtomicU64,
}

impl FakeMic {
    fn send(&self, samples: Vec<f32>) {
        let feed = self.feed.lock();
        feed.as_ref().expect("the microphone is open").send(samples).unwrap();
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
    fn open(&self) -> Result<MicFeed, String> {
        let (tx, rx) = unbounded();
        *self.0.feed.lock() = Some(tx);
        self.0.opened.fetch_add(1, Ordering::SeqCst);
        Ok(MicFeed { frames: rx, lease: Box::new(FakeLease(self.0.clone())) })
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
    dictation: Arc<FakeDictation>,
    sessions: LiveSessions,
    temp: tempfile::TempDir,
}

fn fixture_with(engine: FakeEngine) -> Fixture {
    let engine = Arc::new(engine);
    let mic = Arc::new(FakeMic::default());
    let dictation = Arc::new(FakeDictation::default());
    let temp = tempfile::TempDir::new().unwrap();
    let sessions = LiveSessions::new(LiveDeps {
        engine: engine.clone(),
        microphone: Some(Arc::new(MicHandle(mic.clone()))),
        priority: dictation.clone(),
        models: crate::jobs::ModelUse::new(),
        temp_dir: temp.path().join("live"),
    });
    Fixture { engine, mic, dictation, sessions, temp }
}

fn fixture() -> Fixture {
    fixture_with(FakeEngine::default())
}

fn owner(connection: u64) -> Owner {
    Owner { connection, extension_id: "dev.example.notetaker".into() }
}

fn spec(sources: &[Source]) -> SessionSpec {
    SessionSpec {
        sources: sources.to_vec(),
        final_model: ModelId::new("final"),
        preview_model: Some(ModelId::new("streaming")),
        vad: Vad::Basic,
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
    let session = f.sessions.start(owner(1), spec, sink.clone()).unwrap();
    assert_eq!(f.sessions.current().map(|l| (l.session, l.extension_id, l.captures_mic)), Some((session, "dev.example.notetaker".to_string(), false)));

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
    let first = f.sessions.start(owner(1), spec(&[Source::Client]), Arc::new(Events::default())).unwrap();
    let busy = f.sessions.start(owner(2), spec(&[Source::Client]), Arc::new(Events::default())).unwrap_err();
    assert_eq!(busy.code, Some(ErrorCode::Busy));
    // Another connection cannot stop it or send it audio.
    assert_eq!(f.sessions.stop(2, first).unwrap_err().code, Some(ErrorCode::BadRequest));
    assert_eq!(f.sessions.audio(2, first, "").unwrap_err().code, Some(ErrorCode::BadRequest));
    assert_eq!(f.sessions.stop(1, first).unwrap(), "");
    let second = f.sessions.start(owner(2), spec(&[Source::Client]), Arc::new(Events::default())).unwrap();
    assert_ne!(first, second);
    assert_eq!(f.sessions.stop(1, second).unwrap_err().code, Some(ErrorCode::BadRequest), "the first connection does not own it");
    f.sessions.stop(2, second).unwrap();
}

#[test]
fn engine_session_ids_are_above_the_ids_of_dictation_and_unique() {
    let f = fixture();
    let spec = SessionSpec { vad: Vad::Model(ModelId::new("silero-vad")), ..spec(&[Source::Mic, Source::Client]) };
    for _ in 0..2 {
        let session = f.sessions.start(owner(1), spec.clone(), Arc::new(Events::default())).unwrap();
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
    let spec = SessionSpec { vad: Vad::Model(ModelId::new("silero-vad")), ..spec(&[Source::Mic, Source::Client]) };
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
    assert_eq!(start(&[Source::System]), Some(ErrorCode::Unsupported));
    assert_eq!(start(&[Source::Other("camera".into())]), Some(ErrorCode::BadRequest));
    let relative = SessionSpec { record_to: Some(PathBuf::from("notes")), ..spec(&[Source::Client]) };
    assert_eq!(f.sessions.start(owner(1), relative, Arc::new(Events::default())).unwrap_err().code, Some(ErrorCode::BadRequest));
    assert_eq!(f.sessions.current(), None, "a refused start leaves the slot free");
}

#[test]
fn a_mic_session_records_to_a_temporary_file_that_the_stop_deletes() {
    let f = fixture();
    let sink = Arc::new(Events::default());
    let session = f.sessions.start(owner(1), spec(&[Source::Mic]), sink.clone()).unwrap();
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
    let session = f.sessions.start(owner(1), SessionSpec { preview_model: None, ..spec(&[Source::Client]) }, sink.clone()).unwrap();
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
    let spec = SessionSpec { vad: Vad::Model(ModelId::new("silero-vad")), ..spec(&[Source::Client]) };
    let session = f.sessions.start(owner(1), spec, sink.clone()).unwrap();
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
    let session = f.sessions.start(owner(1), spec(&[Source::Client]), sink.clone()).unwrap();
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
    let session = f.sessions.start(owner(1), spec(&[Source::Client]), sink.clone()).unwrap();
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
    let session = f.sessions.start(owner(7), spec, sink.clone()).unwrap();
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
