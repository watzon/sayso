//! `transcribe_file` and `cancel_job` over a real socket, with the real
//! identity check (see `common`) and a fake engine that finds "speech" in
//! the samples: runs of one constant value, where the value 3 is the word
//! "w3", and zeros for silence.

use crossbeam_channel::Receiver;
use parking_lot::Mutex;
use sayso_core::config::Config;
use sayso_core::models::{self, ModelId, ModelInfo, ModelTask};
use sayso_core::stt::{self, Diarization, EngineEvent, ModelStatus, SessionOptions, SpeechPart, TimedTranscript, Transcript, Turn};
use sayso_sdk::protocol::{ErrorCode, Event as HostEvent, JobReply};
use sayso_sdk::client::{Client, Event};
use sayso_host::{DownloadQuestion, Host, HostServices};
use sayso_platform::{PlatformError, Result, SttBackend};
use serde_json::json;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

mod common;
use common::*;

const ALL: &[&str] = &["transcribe.file"];

// ---------------------------------------------------------------------------
// The fake engine and the services
// ---------------------------------------------------------------------------

type Hook = Box<dyn FnMut(usize) + Send>;

struct JobEngine {
    on_disk: HashSet<ModelId>,
    features: Vec<String>,
    loaded: Mutex<HashSet<ModelId>>,
    turns: Vec<Turn>,
    transcriptions: AtomicUsize,
    after_transcribe: Mutex<Option<Hook>>,
}

impl JobEngine {
    fn new() -> JobEngine {
        JobEngine {
            on_disk: [speech(), first_of(ModelTask::Vad), first_of(ModelTask::Diarization)].into(),
            features: ["segments", "vad", "diarization"].map(String::from).to_vec(),
            loaded: Mutex::default(),
            turns: Vec::new(),
            transcriptions: AtomicUsize::new(0),
            after_transcribe: Mutex::default(),
        }
    }

    fn transcribed(&self) -> usize {
        self.transcriptions.load(Ordering::SeqCst)
    }
}

/// The runs of one non-zero value: (start_ms, end_ms, value).
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

impl SttBackend for JobEngine {
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
        self.loaded.lock().insert(model.clone());
        Ok(())
    }
    fn unload(&self, _: &ModelId, _: bool) -> Result<()> {
        Ok(())
    }
    fn preview_ready(&self, _: &ModelId) -> bool {
        false
    }
    fn start_stream(&self, _: u64, _: &SessionOptions) -> Result<()> {
        Ok(())
    }
    fn push_audio(&self, _: u64, _: &[f32]) -> Result<()> {
        Ok(())
    }
    fn end_stream(&self, _: u64) {}
    fn transcribe(&self, _: &[f32], _: &SessionOptions) -> Result<Transcript> {
        Err(PlatformError::Failed("this test uses segments".into()))
    }
    fn features(&self) -> Vec<String> {
        self.features.clone()
    }
    fn transcribe_segments(&self, samples: &[f32], options: &SessionOptions) -> Result<TimedTranscript> {
        let n = self.transcriptions.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(hook) = self.after_transcribe.lock().as_mut() {
            hook(n);
        }
        let segments = runs(samples)
            .into_iter()
            .map(|(start_ms, end_ms, v)| stt::Segment { start_ms, end_ms, text: format!("w{v}"), words: Vec::new() })
            .collect();
        Ok(TimedTranscript { text: String::new(), model: options.final_model.clone(), elapsed_ms: 1, segments })
    }
    fn detect_speech(&self, _: &ModelId, samples: &[f32]) -> Result<Vec<SpeechPart>> {
        Ok(runs(samples).into_iter().map(|(start_ms, end_ms, _)| SpeechPart { start_ms, end_ms }).collect())
    }
    fn diarize(&self, _: &ModelId, _: &[f32], _: Option<u32>) -> Result<Diarization> {
        Ok(Diarization { speakers: 2, turns: self.turns.clone() })
    }
    fn subscribe(&self) -> Receiver<EngineEvent> {
        crossbeam_channel::never()
    }
}

struct Services {
    engine: Option<Arc<JobEngine>>,
    config: Config,
}

impl HostServices for Services {
    fn engine(&self) -> Option<Arc<dyn SttBackend>> {
        self.engine.clone().map(|engine| engine as Arc<dyn SttBackend>)
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
}

fn first_of(task: ModelTask) -> ModelId {
    models::models_for(task).next().unwrap().id.clone()
}

fn speech() -> ModelId {
    models::catalog().iter().find(|m| m.final_pass).unwrap().id.clone()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct World {
    host: Host,
    setup: Setup,
    engine: Arc<JobEngine>,
}

fn world(granted: &[&str], engine: JobEngine) -> World {
    let setup = setup();
    write_apps(&setup.apps, granted);
    let engine = Arc::new(engine);
    let mut config = Config::default();
    config.dictation.model = speech();
    let host = start_with(&setup, Arc::new(Services { engine: Some(engine.clone()), config }));
    World { host, setup, engine }
}

impl World {
    fn client(&self) -> (Client, Receiver<Event>) {
        let client = Client::connect_to(&self.setup.socket).unwrap();
        let events = client.events();
        (client, events)
    }

    /// A WAV file of `len_ms` with the words `(start_ms, end_ms, value)`.
    fn wav(&self, name: &str, len_ms: u64, words: &[(u64, u64, i16)]) -> PathBuf {
        let mut samples = vec![0i16; (len_ms * 16) as usize];
        for &(start, end, value) in words {
            samples[(start * 16) as usize..(end * 16) as usize].fill(value * 1000);
        }
        let path = self.setup.dir.path().join(name);
        std::fs::write(&path, stt::encode_wav(&samples)).unwrap();
        path
    }
}

const THREE: [(u64, u64, i16); 3] = [(1_000, 3_000, 1), (8_000, 10_000, 2), (15_000, 17_000, 3)];

fn start_job(client: &Client, path: &Path, extra: serde_json::Value) -> u64 {
    let mut body = json!({"path": path});
    body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    client.request("transcribe_file", fields(body)).unwrap().parse::<JobReply>().unwrap().job
}

/// The events of `job` until its `job_done` or `job_failed`. Every job event
/// has no `id`.
fn until_end(events: &Receiver<Event>, job: u64) -> Vec<HostEvent> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut found = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(Event::Host { id, event }) = events.recv_timeout(left) else { panic!("job {job} did not end: {found:?}") };
        if job_of(&event) != Some(job) {
            continue;
        }
        assert_eq!(id, None, "a job event has no id: {event:?}");
        let end = matches!(event, HostEvent::JobDone { .. } | HostEvent::JobFailed { .. });
        found.push(event);
        if end {
            return found;
        }
    }
}

fn job_of(event: &HostEvent) -> Option<u64> {
    match event {
        HostEvent::JobProgress { job, .. } | HostEvent::JobDone { job, .. } | HostEvent::JobFailed { job, .. } => Some(*job),
        HostEvent::Segment(s) => s.job,
        HostEvent::Speakers(s) => s.job,
        _ => None,
    }
}

fn cancelled(job: u64) -> HostEvent {
    HostEvent::JobFailed { job, message: sayso_host::jobs::CANCELLED.into(), code: None }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn a_job_gives_progress_segments_speakers_and_its_text() {
    let mut engine = JobEngine::new();
    engine.turns = vec![
        Turn { start_ms: 900, end_ms: 3_100, speaker: 0 },
        Turn { start_ms: 7_900, end_ms: 10_100, speaker: 1 },
        Turn { start_ms: 14_900, end_ms: 17_100, speaker: 0 },
    ];
    let world = world(ALL, engine);
    let (client, events) = world.client();
    let path = world.wav("meeting.wav", 20_000, &THREE);
    let job = start_job(&client, &path, json!({"diarize": true}));
    assert_eq!(job, 1);
    let found = until_end(&events, job);

    assert_eq!(found[0], HostEvent::JobProgress { job, fraction: 0.0 });
    let segments: Vec<(u64, u64, String)> = found
        .iter()
        .filter_map(|e| if let HostEvent::Segment(s) = e { Some((s.segment.start_ms, s.segment.end_ms, s.segment.text.clone())) } else { None })
        .collect();
    assert_eq!(segments, [(1_000, 3_000, "w1".into()), (8_000, 10_000, "w2".into()), (15_000, 17_000, "w3".into())]);
    let n = found.len();
    let HostEvent::Speakers(speakers) = &found[n - 2] else { panic!("speakers before the end: {found:?}") };
    let turns: Vec<(u64, u64, &str)> = speakers.turns.iter().map(|t| (t.start_ms, t.end_ms, t.speaker.as_str())).collect();
    assert_eq!(turns, [(1_000, 3_000, "1"), (8_000, 10_000, "2"), (15_000, 17_000, "1")]);
    assert_eq!(found[n - 1], HostEvent::JobDone { job, text: "w1 w2 w3".into() });
    // While the job ran, its models were held; after it, they wait for the idle time.
    assert!(world.host.model_use().wanted(Some(Duration::from_secs(60))).contains(&speech()));
}

#[test]
fn a_second_job_waits_with_fraction_0_then_runs() {
    let world = world(ALL, JobEngine::new());
    let (client, events) = world.client();
    let path = world.wav("a.wav", 20_000, &THREE);
    // A dictation holds the first job before its first engine call.
    world.host.dictation_gate().begin();
    let first = start_job(&client, &path, json!({}));
    let second = start_job(&client, &path, json!({}));
    std::thread::sleep(Duration::from_millis(100));
    // Both jobs say only that they wait. (The second can hear it twice, when
    // the first job starts after the second came.)
    let early: Vec<HostEvent> = events.try_iter().filter_map(|e| if let Event::Host { event, .. } = e { Some(event) } else { None }).collect();
    assert!(early.iter().all(|e| matches!(e, HostEvent::JobProgress { fraction, .. } if *fraction == 0.0)), "{early:?}");
    for job in [first, second] {
        assert!(early.contains(&HostEvent::JobProgress { job, fraction: 0.0 }), "{early:?}");
    }
    assert_eq!(world.engine.transcribed(), 0);

    world.host.dictation_gate().end();
    let found = until_end(&events, first);
    assert!(matches!(found.last(), Some(HostEvent::JobDone { .. })));
    // The second job runs after the first one.
    let found = until_end(&events, second);
    assert!(matches!(found.last(), Some(HostEvent::JobDone { .. })));
    assert_eq!(world.engine.transcribed(), 6);
}

#[test]
fn cancel_ends_a_waiting_and_a_running_job() {
    let world = world(ALL, JobEngine::new());
    let (client, events) = world.client();
    let path = world.wav("a.wav", 20_000, &THREE);
    world.host.dictation_gate().begin();
    let running = start_job(&client, &path, json!({}));
    let waiting = start_job(&client, &path, json!({}));

    client.request("cancel_job", fields(json!({"job": waiting}))).unwrap();
    assert_eq!(until_end(&events, waiting).last(), Some(&cancelled(waiting)));
    client.request("cancel_job", fields(json!({"job": running}))).unwrap();
    assert_eq!(until_end(&events, running).last(), Some(&cancelled(running)));
    assert_eq!(world.engine.transcribed(), 0);
    // A second cancel of an ended job of this connection is fine.
    client.request("cancel_job", fields(json!({"job": running}))).unwrap();

    // A job that this connection did not start.
    assert_eq!(refused(client.request("cancel_job", fields(json!({"job": 99})))).code, Some(ErrorCode::BadRequest));
    let (other, _) = world.client();
    world.host.dictation_gate().end();
    let job = start_job(&client, &path, json!({}));
    assert_eq!(refused(other.request("cancel_job", fields(json!({"job": job})))).code, Some(ErrorCode::BadRequest));
    assert!(matches!(until_end(&events, job).last(), Some(HostEvent::JobDone { .. })));
}

#[test]
fn a_dictation_holds_a_job_between_its_parts() {
    let world = world(ALL, JobEngine::new());
    let gate = world.host.dictation_gate();
    {
        let gate = gate.clone();
        // A dictation starts while the first part runs.
        *world.engine.after_transcribe.lock() = Some(Box::new(move |n| {
            if n == 1 {
                gate.begin();
            }
        }));
    }
    let (client, events) = world.client();
    let path = world.wav("a.wav", 20_000, &THREE);
    let job = start_job(&client, &path, json!({}));
    eventually("the first part", || world.engine.transcribed() == 1);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(world.engine.transcribed(), 1, "the job waits for the dictation");
    gate.end();
    assert!(matches!(until_end(&events, job).last(), Some(HostEvent::JobDone { .. })));
    assert_eq!(world.engine.transcribed(), 3);
}

#[test]
fn a_closed_connection_cancels_its_jobs() {
    let world = world(ALL, JobEngine::new());
    let path = world.wav("a.wav", 20_000, &THREE);
    world.host.dictation_gate().begin();
    let (first, _) = world.client();
    start_job(&first, &path, json!({}));
    start_job(&first, &path, json!({}));
    drop(first);
    eventually("the host sees the close", || world.host.connected_apps().is_empty());

    let (second, events) = world.client();
    let job = start_job(&second, &path, json!({}));
    assert_eq!(job, 3);
    world.host.dictation_gate().end();
    assert!(matches!(until_end(&events, job).last(), Some(HostEvent::JobDone { .. })));
    // Only the job of the open connection ran: 3 parts.
    assert_eq!(world.engine.transcribed(), 3);
}

#[test]
fn a_job_that_cannot_run_is_refused_with_a_code() {
    let mut engine = JobEngine::new();
    engine.on_disk.remove(&first_of(ModelTask::Diarization));
    let world = world(ALL, engine);
    let (client, _events) = world.client();
    let path = world.wav("a.wav", 1_000, &[]);
    let code = |extra: serde_json::Value| {
        let mut body = json!({"path": path});
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        refused(client.request("transcribe_file", fields(body))).code
    };
    assert_eq!(code(json!({"model": "no-such-model"})), Some(ErrorCode::UnknownModel));
    assert_eq!(code(json!({"vad_model": speech().as_str()})), Some(ErrorCode::WrongRole));
    assert_eq!(code(json!({"diarize": true})), Some(ErrorCode::ModelNotDownloaded));
    assert_eq!(code(json!({"path": "relative.wav"})), Some(ErrorCode::BadRequest));

    let mut old = JobEngine::new();
    old.features = vec!["segments".into()];
    let world = self::world(ALL, old);
    let (client, _events) = world.client();
    let body = json!({"path": path, "vad_model": first_of(ModelTask::Vad).as_str()});
    assert_eq!(refused(client.request("transcribe_file", fields(body))).code, Some(ErrorCode::Unsupported));
}

#[test]
fn jobs_need_their_capability_and_an_engine() {
    let world = world(&[], JobEngine::new());
    let (client, _events) = world.client();
    let path = world.wav("a.wav", 1_000, &[]);
    let error = refused(client.request("transcribe_file", fields(json!({"path": path}))));
    assert_eq!(error.code, Some(ErrorCode::Capability));
    assert_eq!(error.capability, Some(sayso_sdk::protocol::Capability::TranscribeFile));
    // An app can always cancel its own job, so this needs no grant.
    assert_eq!(refused(client.request("cancel_job", fields(json!({"job": 1})))).code, Some(ErrorCode::BadRequest));

    let setup = setup();
    write_apps(&setup.apps, ALL);
    let _host = start_with(&setup, Arc::new(Services { engine: None, config: Config::default() }));
    let client = Client::connect_to(&setup.socket).unwrap();
    let error = refused(client.request("transcribe_file", fields(json!({"path": path}))));
    assert_eq!(error.code, Some(ErrorCode::Unsupported));
}

#[test]
fn closing_the_host_fails_the_jobs_before_host_closing() {
    let world = world(ALL, JobEngine::new());
    let (client, events) = world.client();
    let path = world.wav("a.wav", 20_000, &THREE);
    world.host.dictation_gate().begin();
    let job = start_job(&client, &path, json!({}));
    world.host.close();
    assert_eq!(until_end(&events, job).last(), Some(&cancelled(job)));
    // `host_closing` comes after the end of the job.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Event::HostGone) => break,
            Ok(Event::Host { event, .. }) => assert!(job_of(&event) != Some(job), "an event of the job after its end: {event:?}"),
            Ok(Event::Error(error)) => panic!("an error without an id: {error:?}"),
            Err(_) => panic!("no host_closing"),
        }
    }
}

#[test]
fn a_part_of_a_live_session_holds_a_job_between_its_parts() {
    let world = world(ALL, JobEngine::new());
    let gate = world.host.dictation_gate();
    let (release, released) = crossbeam_channel::bounded::<()>(0);
    let (holding, held) = crossbeam_channel::bounded::<()>(0);
    {
        let gate = gate.clone();
        // A live session gives the engine a part while the first part of the job runs.
        *world.engine.after_transcribe.lock() = Some(Box::new(move |n| {
            if n == 1 {
                let (gate, released, holding) = (gate.clone(), released.clone(), holding.clone());
                std::thread::spawn(move || {
                    let _part = gate.live_part();
                    holding.send(()).unwrap();
                    let _ = released.recv();
                });
                let _ = held.recv();
            }
        }));
    }
    let (client, events) = world.client();
    let path = world.wav("a.wav", 20_000, &THREE);
    let job = start_job(&client, &path, json!({}));
    eventually("the first part", || world.engine.transcribed() == 1);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(world.engine.transcribed(), 1, "the job waits for the part of the live session");
    release.send(()).unwrap();
    assert!(matches!(until_end(&events, job).last(), Some(HostEvent::JobDone { .. })));
    assert_eq!(world.engine.transcribed(), 3);
}

#[test]
fn jobs_hold_sayso_open_until_they_end() {
    let world = world(ALL, JobEngine::new());
    // The name of the record is what Sayso shows.
    let apps = std::fs::read_to_string(&world.setup.apps).unwrap();
    std::fs::write(&world.setup.apps, apps.replace("name = \"Test\"", "name = \"Media\"")).unwrap();
    let changes = Arc::new(AtomicUsize::new(0));
    {
        let changes = changes.clone();
        world.host.set_holds_observer(Arc::new(move || {
            changes.fetch_add(1, Ordering::SeqCst);
        }));
    }
    assert!(world.host.holds().is_empty());
    let (client, events) = world.client();
    let path = world.wav("a.wav", 20_000, &THREE);
    world.host.dictation_gate().begin();
    let first = start_job(&client, &path, json!({}));
    let second = start_job(&client, &path, json!({}));
    let hold = sayso_host::Hold { app_id: id(), name: "Media".into(), work: sayso_host::Work::Job };
    assert_eq!(world.host.holds(), vec![hold], "one entry for the jobs of one extension");
    assert!(changes.load(Ordering::SeqCst) >= 2, "the observer hears each new job");

    client.request("cancel_job", fields(json!({"job": second}))).unwrap();
    let before_end = changes.load(Ordering::SeqCst);
    world.host.dictation_gate().end();
    assert!(matches!(until_end(&events, first).last(), Some(HostEvent::JobDone { .. })));
    eventually("the jobs no longer hold Sayso", || world.host.holds().is_empty());
    eventually("the observer hears the end", || changes.load(Ordering::SeqCst) > before_end);
}

#[test]
fn a_grant_that_goes_ends_the_jobs_of_the_app_with_capability() {
    let world = world(ALL, JobEngine::new());
    let (client, events) = world.client();
    let path = world.wav("a.wav", 20_000, &THREE);
    world.host.dictation_gate().begin();
    let running = start_job(&client, &path, json!({}));
    let waiting = start_job(&client, &path, json!({}));
    world.host.stop_work_without_grant();
    assert!(!world.host.holds().is_empty(), "the grant is still there");

    world.host.apps().set_grant(&id(), &sayso_sdk::protocol::Capability::TranscribeFile, false).unwrap();
    world.host.stop_work_without_grant();
    let gone = |job| HostEvent::JobFailed { job, message: sayso_host::jobs::GRANT_GONE.into(), code: Some(ErrorCode::Capability) };
    assert_eq!(until_end(&events, running).last(), Some(&gone(running)));
    assert_eq!(until_end(&events, waiting).last(), Some(&gone(waiting)));
    world.host.dictation_gate().end();
    assert_eq!(world.engine.transcribed(), 0, "no text after the grant went");

    // An app can always cancel its own job.
    client.request("cancel_job", fields(json!({"job": running}))).unwrap();
}
