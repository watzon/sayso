//! Jobs from end to end, with a fake engine.
//!
//! The test audio has "speech" as runs of one constant sample value, and
//! silence as zeros. The fake engine finds the runs in the samples that it
//! gets, so its VAD, its segments, and its windows follow the audio like a
//! real engine: a run of the value 3 is the word "w3".

use super::*;
use crate::jobs::plan::Stopped;
use parking_lot::Mutex;
use sayso_core::models::ModelInfo;
use sayso_core::stt::{Diarization, EngineEvent, ModelStatus, SessionOptions, SpeechPart, TimedTranscript, Transcript, Turn};
use sayso_platform::{PlatformError, Result as PlatformResult, SttBackend};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// The fake engine
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Call {
    Load(ModelId),
    Vad { len_ms: u64 },
    Transcribe { len_ms: u64 },
    Diarize { speakers: Option<u32> },
}

type Hook = Box<dyn FnMut(usize) + Send>;

#[derive(Default)]
struct FakeEngine {
    loaded: Mutex<HashSet<ModelId>>,
    calls: Mutex<Vec<Call>>,
    turns: Vec<Turn>,
    transcriptions: AtomicUsize,
    /// Runs after each transcription, with how many there were.
    after_transcribe: Mutex<Option<Hook>>,
    fail_transcribe: bool,
}

/// The runs of one non-zero value in `samples`: (start_ms, end_ms, value).
fn runs(samples: &[f32]) -> Vec<(u64, u64, i32)> {
    let mut found = Vec::new();
    let mut open: Option<(usize, i32)> = None;
    for (i, &s) in samples.iter().chain(std::iter::once(&0.0)).enumerate() {
        let value = (s * 32768.0).round() as i32 / 1000;
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

impl FakeEngine {
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().clone()
    }

    fn transcribed(&self) -> usize {
        self.transcriptions.load(Ordering::SeqCst)
    }

    fn after_transcription(&self) -> PlatformResult<()> {
        let n = self.transcriptions.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(hook) = self.after_transcribe.lock().as_mut() {
            hook(n);
        }
        if self.fail_transcribe { Err(PlatformError::Failed("the model crashed".into())) } else { Ok(()) }
    }
}

impl SttBackend for FakeEngine {
    fn catalog(&self) -> Vec<ModelInfo> {
        Vec::new()
    }
    fn status(&self, model: &ModelId) -> ModelStatus {
        if self.loaded.lock().contains(model) { ModelStatus::Ready } else { ModelStatus::Downloaded }
    }
    fn download(&self, _model: &ModelId) -> PlatformResult<()> {
        Ok(())
    }
    fn delete(&self, _model: &ModelId) -> PlatformResult<()> {
        Ok(())
    }
    fn load(&self, model: &ModelId) -> PlatformResult<()> {
        self.calls.lock().push(Call::Load(model.clone()));
        self.loaded.lock().insert(model.clone());
        Ok(())
    }
    fn unload(&self, _model: &ModelId, _keep_preview: bool) -> PlatformResult<()> {
        Ok(())
    }
    fn preview_ready(&self, _model: &ModelId) -> bool {
        false
    }
    fn start_stream(&self, _session: u64, _options: &SessionOptions) -> PlatformResult<()> {
        Ok(())
    }
    fn push_audio(&self, _session: u64, _samples: &[f32]) -> PlatformResult<()> {
        Ok(())
    }
    fn end_stream(&self, _session: u64) {}
    fn transcribe(&self, samples: &[f32], options: &SessionOptions) -> PlatformResult<Transcript> {
        self.calls.lock().push(Call::Transcribe { len_ms: samples.len() as u64 / 16 });
        self.after_transcription()?;
        let words: Vec<String> = runs(samples).iter().map(|(_, _, v)| format!("w{v}")).collect();
        Ok(Transcript { text: format!(" {} ", words.join(" ")), model: options.final_model.clone(), elapsed_ms: 1 })
    }
    fn transcribe_segments(&self, samples: &[f32], options: &SessionOptions) -> PlatformResult<TimedTranscript> {
        self.calls.lock().push(Call::Transcribe { len_ms: samples.len() as u64 / 16 });
        self.after_transcription()?;
        let segments = runs(samples)
            .into_iter()
            .map(|(start_ms, end_ms, v)| sayso_core::stt::Segment {
                start_ms,
                end_ms,
                text: format!(" w{v}"),
                words: vec![sayso_core::stt::Word { start_ms, end_ms, text: format!("w{v}") }],
            })
            .collect();
        Ok(TimedTranscript { text: String::new(), model: options.final_model.clone(), elapsed_ms: 1, segments })
    }
    fn detect_speech(&self, _model: &ModelId, samples: &[f32]) -> PlatformResult<Vec<SpeechPart>> {
        self.calls.lock().push(Call::Vad { len_ms: samples.len() as u64 / 16 });
        Ok(runs(samples).into_iter().map(|(start_ms, end_ms, _)| SpeechPart { start_ms, end_ms }).collect())
    }
    fn diarize(&self, _model: &ModelId, _samples: &[f32], speakers: Option<u32>) -> PlatformResult<Diarization> {
        self.calls.lock().push(Call::Diarize { speakers });
        let count = self.turns.iter().map(|t| t.speaker).collect::<HashSet<_>>().len() as u32;
        Ok(Diarization { speakers: count, turns: self.turns.clone() })
    }
    fn subscribe(&self) -> crossbeam_channel::Receiver<EngineEvent> {
        crossbeam_channel::never()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The events of a job, in order.
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

    fn segments(&self) -> Vec<(u64, u64, String)> {
        self.all()
            .into_iter()
            .filter_map(|e| match e {
                Event::Segment(s) => Some((s.segment.start_ms, s.segment.end_ms, s.segment.text)),
                _ => None,
            })
            .collect()
    }

    fn last(&self) -> Option<Event> {
        self.0.lock().last().cloned()
    }

    /// Wait for `job_done` or `job_failed`.
    fn wait_end(&self) -> Event {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(end @ (Event::JobDone { .. } | Event::JobFailed { .. })) = self.last() {
                return end;
            }
            assert!(Instant::now() < deadline, "the job did not end: {:?}", self.all());
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Wait until `done` is true.
fn wait_for(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// 16 kHz audio of `len_ms` with runs of `(start_ms, end_ms, value)`.
fn audio(len_ms: u64, runs: &[(u64, u64, i16)]) -> Vec<i16> {
    let mut samples = vec![0i16; (len_ms * 16) as usize];
    for &(start, end, value) in runs {
        samples[(start * 16) as usize..(end * 16) as usize].fill(value * 1000);
    }
    samples
}

/// A WAV file of `samples` in a new temporary folder.
fn wav(samples: &[i16]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("job.wav");
    std::fs::write(&path, sayso_core::stt::encode_wav(samples)).unwrap();
    (dir, path)
}

fn spec(path: std::path::PathBuf, timestamps: Timestamps, vad: VadRun) -> JobSpec {
    JobSpec {
        path,
        model: ModelId::new("speech"),
        timestamps,
        vad,
        diarization: None,
        language: "en".into(),
        speakers: None,
    }
}

fn vad_model() -> VadRun {
    VadRun::Model(ModelId::new("vad"))
}

/// Three words, with pauses longer than a chunk may have.
const THREE: [(u64, u64, i16); 3] = [(1_000, 3_000, 1), (8_000, 10_000, 2), (15_000, 17_000, 3)];

struct Harness {
    engine: Arc<FakeEngine>,
    gate: Arc<DictationGate>,
    models: Arc<ModelUse>,
    queue: Arc<JobQueue>,
}

impl Harness {
    fn new(engine: FakeEngine) -> Harness {
        Self::with_window(engine, plan::VAD_WINDOW_MS)
    }

    fn with_window(engine: FakeEngine, window_ms: u64) -> Harness {
        let engine = Arc::new(engine);
        let gate = Arc::new(DictationGate::new());
        let models = ModelUse::new();
        let runner = JobRunner::new(engine.clone(), models.clone()).with_vad_window(window_ms);
        let queue = Arc::new(JobQueue::start(Arc::new(runner), gate.clone()).unwrap());
        Harness { engine, gate, models, queue }
    }

    /// Run `job` to its end, and give its events.
    fn run(&self, job: JobSpec) -> Arc<Events> {
        let events = Arc::new(Events::default());
        self.queue.submit(1, job, events.clone(), |_| {}).unwrap();
        events.wait_end();
        events
    }
}

// ---------------------------------------------------------------------------
// The runner
// ---------------------------------------------------------------------------

#[test]
fn a_file_with_three_parts_gives_shifted_segments_in_order() {
    let (_dir, path) = wav(&audio(20_000, &THREE));
    let harness = Harness::new(FakeEngine::default());
    let events = harness.run(spec(path, Timestamps::Word, vad_model()));

    assert_eq!(
        events.segments(),
        [(1_000, 3_000, "w1".to_string()), (8_000, 10_000, "w2".to_string()), (15_000, 17_000, "w3".to_string())]
    );
    assert_eq!(events.last(), Some(Event::JobDone { job: 1, text: "w1 w2 w3".into() }));
    // Words have times from the start of the file too.
    let words: Vec<_> = events
        .all()
        .into_iter()
        .filter_map(|e| if let Event::Segment(s) = e { s.segment.words } else { None })
        .flatten()
        .map(|w| (w.start_ms, w.end_ms))
        .collect();
    assert_eq!(words, [(1_000, 3_000), (8_000, 10_000), (15_000, 17_000)]);
    // Progress: 0 in the queue, then the end of each chunk, then all.
    let progress: Vec<f64> = events
        .all()
        .into_iter()
        .filter_map(|e| if let Event::JobProgress { fraction, .. } = e { Some(fraction) } else { None })
        .collect();
    assert_eq!(progress, [0.0, 3_200.0 / 20_000.0, 10_200.0 / 20_000.0, 1.0]);
    // The models load once, the VAD model first. Each chunk is one call of
    // its padded length.
    assert_eq!(
        harness.engine.calls(),
        [
            Call::Load(ModelId::new("vad")),
            Call::Load(ModelId::new("speech")),
            Call::Vad { len_ms: 20_000 },
            Call::Transcribe { len_ms: 2_400 },
            Call::Transcribe { len_ms: 2_400 },
            Call::Transcribe { len_ms: 2_400 },
        ]
    );
    // After the job, its models stay wanted for the idle time only.
    assert!(!harness.models.held(&ModelId::new("speech")));
    assert_eq!(harness.models.wanted(Some(Duration::from_secs(60))), [ModelId::new("speech"), ModelId::new("vad")]);
}

#[test]
fn basic_vad_finds_the_same_parts_in_the_host() {
    let (_dir, path) = wav(&audio(20_000, &THREE));
    let harness = Harness::new(FakeEngine::default());
    let events = harness.run(spec(path, Timestamps::Word, VadRun::Basic));
    assert_eq!(events.segments().iter().map(|s| (s.0, s.1)).collect::<Vec<_>>(), [(1_000, 3_000), (8_000, 10_000), (15_000, 17_000)]);
    // No VAD model: the engine loads the speech model only, and gets no `vad`.
    let calls = harness.engine.calls();
    assert_eq!(calls[0], Call::Load(ModelId::new("speech")));
    assert!(calls[1..].iter().all(|c| matches!(c, Call::Transcribe { .. })), "{calls:?}");
    assert_eq!(harness.engine.transcribed(), 3);
}

#[test]
fn a_model_without_times_gives_one_segment_for_each_chunk() {
    // Two words close together join into one chunk; the third is alone.
    let runs = [(1_000, 2_000, 1), (2_500, 3_500, 2), (10_000, 11_000, 3)];
    let (_dir, path) = wav(&audio(12_000, &runs));
    let harness = Harness::new(FakeEngine::default());
    let events = harness.run(spec(path, Timestamps::None, vad_model()));
    assert_eq!(events.segments(), [(800, 3_700, "w1 w2".to_string()), (9_800, 11_200, "w3".to_string())]);
}

#[test]
fn a_part_across_two_vad_windows_stays_whole() {
    // Windows of 5 s: the word from 4 s to 6 s crosses the first edge.
    let runs = [(1_000, 2_000, 1), (4_000, 6_000, 2), (12_000, 13_000, 3)];
    let (_dir, path) = wav(&audio(14_000, &runs));
    let harness = Harness::with_window(FakeEngine::default(), 5_000);
    let events = harness.run(spec(path, Timestamps::Word, vad_model()));
    assert_eq!(events.segments().iter().map(|s| (s.0, s.1)).collect::<Vec<_>>(), [(1_000, 2_000), (4_000, 6_000), (12_000, 13_000)]);
    let windows: Vec<Call> = harness.engine.calls().into_iter().filter(|c| matches!(c, Call::Vad { .. })).collect();
    // 0-5 s (the part at 4 s waits), 4-9 s, 9-14 s.
    assert_eq!(windows, [Call::Vad { len_ms: 5_000 }, Call::Vad { len_ms: 5_000 }, Call::Vad { len_ms: 5_000 }]);
}

#[test]
fn a_file_without_speech_is_done_without_text() {
    let (_dir, path) = wav(&audio(5_000, &[]));
    let harness = Harness::new(FakeEngine::default());
    let events = harness.run(spec(path, Timestamps::Word, vad_model()));
    assert_eq!(events.last(), Some(Event::JobDone { job: 1, text: String::new() }));
    assert_eq!(harness.engine.transcribed(), 0);
}

#[test]
fn an_unreadable_file_or_a_failed_engine_fails_the_job() {
    let dir = tempfile::TempDir::new().unwrap();
    let text = dir.path().join("notes.mp3");
    std::fs::write(&text, "this is not audio, only text that is long enough to probe").unwrap();
    let harness = Harness::new(FakeEngine::default());
    let Event::JobFailed { message, code, .. } = harness.run(spec(text, Timestamps::Word, VadRun::Basic)).wait_end() else { panic!() };
    assert!(message.contains("notes.mp3") && message.contains("WAV, FLAC, MP3, AAC, and M4A"), "{message}");
    assert_eq!(code, None);
    let Event::JobFailed { message, .. } = harness.run(spec(dir.path().join("gone.wav"), Timestamps::Word, VadRun::Basic)).wait_end() else {
        panic!()
    };
    assert!(message.starts_with("Sayso cannot open"), "{message}");

    let (_dir, path) = wav(&audio(5_000, &[(1_000, 2_000, 1)]));
    let harness = Harness::new(FakeEngine { fail_transcribe: true, ..Default::default() });
    let Event::JobFailed { message, .. } = harness.run(spec(path, Timestamps::Word, VadRun::Basic)).wait_end() else { panic!() };
    assert_eq!(message, "Transcription failed: the model crashed");
}

#[test]
fn diarization_gives_labels_in_order_of_first_speech_after_the_last_segment() {
    let runs = [(1_000, 3_000, 1), (8_000, 10_000, 2), (15_000, 17_000, 3), (21_000, 23_000, 4)];
    let (_dir, path) = wav(&audio(25_000, &runs));
    // The engine says speaker 1 speaks first, then 0. Speaker 2 says 300 ms
    // in the middle of the second word (fewer than 2 % of the speech and 10 s).
    let turns = vec![
        Turn { start_ms: 900, end_ms: 3_100, speaker: 1 },
        Turn { start_ms: 7_900, end_ms: 10_100, speaker: 0 },
        Turn { start_ms: 9_000, end_ms: 9_300, speaker: 2 },
        Turn { start_ms: 14_900, end_ms: 17_100, speaker: 1 },
        Turn { start_ms: 20_900, end_ms: 23_100, speaker: 1 },
        Turn { start_ms: 24_000, end_ms: 24_100, speaker: 3 },
    ]
    .into_iter()
    .chain((0..60).map(|i| Turn { start_ms: 30_000 + i * 1_000, end_ms: 30_900 + i * 1_000, speaker: 0 }))
    .collect();
    let harness = Harness::new(FakeEngine { turns, ..Default::default() });
    let job = JobSpec { diarization: Some(ModelId::new("diarizer")), ..spec(path, Timestamps::Word, vad_model()) };
    let events = harness.run(job.clone()).all();

    let speakers_at = events.iter().position(|e| matches!(e, Event::Speakers(_))).expect("a speakers event");
    let last_segment = events.iter().rposition(|e| matches!(e, Event::Segment(_))).unwrap();
    assert!(speakers_at > last_segment && speakers_at + 1 == events.len() - 1, "{events:?}");
    let Event::Speakers(found) = &events[speakers_at] else { unreachable!() };
    assert_eq!(found.job, Some(1));
    let turns: Vec<(u64, u64, &str)> = found.turns.iter().map(|t| (t.start_ms, t.end_ms, t.speaker.as_str())).collect();
    assert_eq!(turns, [(1_000, 3_000, "1"), (8_000, 10_000, "2"), (15_000, 23_000, "1")]);
    assert!(harness.engine.calls().contains(&Call::Diarize { speakers: None }));
    // Progress leaves room for diarization.
    assert!(events.iter().any(|e| matches!(e, Event::JobProgress { fraction, .. } if (*fraction - TRANSCRIPTION_SHARE).abs() < 1e-9)));

    // With the number of speakers, the engine gets the number.
    let events = harness.run(JobSpec { speakers: Some(4), ..job }).all();
    assert!(harness.engine.calls().contains(&Call::Diarize { speakers: Some(4) }));
    assert!(events.iter().any(|e| matches!(e, Event::Speakers(_))));
}

// ---------------------------------------------------------------------------
// The queue, the gate, and cancel
// ---------------------------------------------------------------------------

#[test]
fn a_cancel_stops_a_running_job_after_its_part() {
    let (_dir, path) = wav(&audio(20_000, &THREE));
    let queue_slot: Arc<Mutex<Option<Arc<JobQueue>>>> = Arc::default();
    let engine = FakeEngine::default();
    {
        let queue_slot = queue_slot.clone();
        *engine.after_transcribe.lock() = Some(Box::new(move |n| {
            if n == 1 {
                assert!(queue_slot.lock().as_ref().unwrap().cancel(7, 1));
            }
        }));
    }
    let harness = Harness::new(engine);
    *queue_slot.lock() = Some(harness.queue.clone());
    let events = Arc::new(Events::default());
    harness.queue.submit(7, spec(path, Timestamps::Word, vad_model()), events.clone(), |_| {}).unwrap();
    assert_eq!(events.wait_end(), Event::JobFailed { job: 1, message: CANCELLED.into(), code: None });
    assert_eq!(events.segments().len(), 1, "the part that ran gives its segment");
    assert_eq!(harness.engine.transcribed(), 1);
    assert!(!harness.queue.cancel(7, 1), "the job ended");
}

#[test]
fn the_gate_holds_a_job_between_parts_while_a_dictation_runs() {
    let (_dir, path) = wav(&audio(20_000, &THREE));
    let gate_slot: Arc<Mutex<Option<Arc<DictationGate>>>> = Arc::default();
    let engine = FakeEngine::default();
    {
        let gate_slot = gate_slot.clone();
        // A dictation starts while the first part runs.
        *engine.after_transcribe.lock() = Some(Box::new(move |n| {
            if n == 1 {
                gate_slot.lock().as_ref().unwrap().begin();
            }
        }));
    }
    let harness = Harness::new(engine);
    *gate_slot.lock() = Some(harness.gate.clone());
    let events = Arc::new(Events::default());
    harness.queue.submit(1, spec(path, Timestamps::Word, vad_model()), events.clone(), |_| {}).unwrap();

    wait_for("the first part", || harness.engine.transcribed() == 1);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(harness.engine.transcribed(), 1, "the job waits for the dictation");
    assert_eq!(events.segments().len(), 1, "the part that ran is out");
    harness.gate.end();
    assert!(matches!(events.wait_end(), Event::JobDone { .. }));
    assert_eq!(harness.engine.transcribed(), 3);
}

#[test]
fn a_job_cancelled_while_it_waits_for_a_dictation_stops_at_once() {
    let (_dir, path) = wav(&audio(20_000, &THREE));
    let harness = Harness::new(FakeEngine::default());
    harness.gate.begin();
    let events = Arc::new(Events::default());
    harness.queue.submit(1, spec(path, Timestamps::Word, vad_model()), events.clone(), |_| {}).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(harness.engine.calls().is_empty(), "nothing runs while the dictation runs");
    assert!(harness.queue.has_jobs(1));
    harness.queue.cancel_connection(1);
    assert_eq!(events.wait_end(), Event::JobFailed { job: 1, message: CANCELLED.into(), code: None });
    wait_for("the job leaves the queue", || !harness.queue.has_jobs(1));
}

/// A job that runs until the test lets it go.
struct Blocking {
    started: Mutex<Vec<u64>>,
    release: crossbeam_channel::Receiver<()>,
}

impl RunJob for Blocking {
    fn run(&self, _job: &JobSpec, run: &JobRun<'_>) {
        self.started.lock().push(run.id);
        loop {
            if run.cancelled() {
                return run.stopped(Stopped::Cancelled);
            }
            if self.release.recv_timeout(Duration::from_millis(5)).is_ok() {
                return run.event(Event::JobDone { job: run.id, text: String::new() });
            }
        }
    }
}

#[test]
fn the_queue_runs_one_job_at_a_time_in_order() {
    let (release, released) = crossbeam_channel::unbounded();
    let runner = Arc::new(Blocking { started: Mutex::new(Vec::new()), release: released });
    let queue = JobQueue::start(runner.clone(), Arc::new(DictationGate::new())).unwrap();
    let job = || spec("/tmp/x.wav".into(), Timestamps::Word, VadRun::Basic);
    let sinks: Vec<Arc<Events>> = (0..4).map(|_| Arc::new(Events::default())).collect();
    for (i, sink) in sinks.iter().enumerate() {
        let accepted = sink.clone();
        let connection = if i == 3 { 2 } else { 1 };
        let id = queue
            .submit(connection, job(), sink.clone(), move |id| accepted.event(Event::JobDone { job: id * 100, text: "accepted".into() }))
            .unwrap();
        assert_eq!(id, i as u64 + 1);
        // The reply comes before the first event of the job.
        assert_eq!(sink.all()[0], Event::JobDone { job: id * 100, text: "accepted".into() });
        assert_eq!(sink.all()[1], Event::JobProgress { job: id, fraction: 0.0 });
        if i == 0 {
            wait_for("job 1 starts", || *runner.started.lock() == [1]);
        }
    }

    // Cancel a waiting job: it ends at once, and never runs.
    assert!(queue.cancel(1, 3));
    assert_eq!(sinks[2].last(), Some(Event::JobFailed { job: 3, message: CANCELLED.into(), code: None }));
    // Another connection cannot cancel the job.
    assert!(!queue.cancel(2, 2));

    release.send(()).unwrap();
    wait_for("job 2 starts", || *runner.started.lock() == [1, 2]);
    assert!(matches!(sinks[0].last(), Some(Event::JobDone { job: 1, .. })));
    // The queue moved: job 4 hears that it still waits.
    let waits = sinks[3].all().iter().filter(|e| matches!(e, Event::JobProgress { fraction, .. } if *fraction == 0.0)).count();
    assert_eq!(waits, 2, "{:?}", sinks[3].all());

    // The connection of jobs 1 to 3 closes: job 2 stops, job 4 runs.
    queue.cancel_connection(1);
    assert_eq!(sinks[1].wait_end(), Event::JobFailed { job: 2, message: CANCELLED.into(), code: None });
    wait_for("job 4 starts", || *runner.started.lock() == [1, 2, 4]);
    queue.close();
    assert_eq!(sinks[3].wait_end(), Event::JobFailed { job: 4, message: CANCELLED.into(), code: None });
    assert!(queue.submit(1, job(), sinks[0].clone(), |_| panic!("no reply after close")).is_none());
}
