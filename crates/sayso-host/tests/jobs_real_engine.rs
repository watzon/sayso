//! A job with the real Swift engine and models that are already on disk. Ignored by default:
//!
//! ```text
//! (cd native/macos/SaysoEngine && swift build -c release)
//! SAYSO_CONF_MODELS=/tmp/conf/swift-models SAYSO_CONF_AUDIO=/tmp/conf/audio/short.wav \
//!   cargo test -p sayso-host --test jobs_real_engine -- --ignored --nocapture
//! ```
//!
//! The folder needs the default speech model (`parakeet-unified-en`), `silero-vad`, and
//! `speaker-diarization`, as for `crates/sayso-engine-client/tests/real_timing.rs`.
#![cfg(target_os = "macos")]

use parking_lot::Mutex;
use sayso_core::config::RoleChoice;
use sayso_core::models::{self, ModelId};
use sayso_engine_client::EngineClient;
use sayso_host::jobs::{DictationGate, EngineView, EventSink, JobQueue, JobRunner, ModelUse, UserChoices, VadRun, resolve};
use sayso_platform::SttBackend;
use sayso_protocol::{Event, TranscribeFile};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Default)]
struct Events(Mutex<Vec<Event>>);

impl EventSink for Events {
    fn event(&self, event: Event) {
        self.0.lock().push(event);
    }
}

impl Events {
    fn wait_end(&self, limit: Duration) -> Vec<Event> {
        let deadline = Instant::now() + limit;
        loop {
            let events = self.0.lock().clone();
            if matches!(events.last(), Some(Event::JobDone { .. } | Event::JobFailed { .. })) {
                return events;
            }
            assert!(Instant::now() < deadline, "the job did not end: {events:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
#[ignore = "needs the real sidecar and models on disk"]
fn a_job_on_the_real_engine_gives_segments_and_speakers() {
    let models_dir = PathBuf::from(std::env::var_os("SAYSO_CONF_MODELS").expect("set SAYSO_CONF_MODELS"));
    let audio = PathBuf::from(std::env::var_os("SAYSO_CONF_AUDIO").expect("set SAYSO_CONF_AUDIO"));
    let engine = EngineClient::default_engine_path();
    assert!(engine.exists(), "build the sidecar first: {}", engine.display());
    let cache = tempfile::TempDir::new().unwrap();
    let client: Arc<dyn SttBackend> = Arc::new(EngineClient::spawn(engine, models_dir, cache.path().to_path_buf()).unwrap());
    let features = client.features();
    println!("features: {features:?}");

    let gate = Arc::new(DictationGate::new());
    let queue = JobQueue::start(Arc::new(JobRunner::new(client.clone(), ModelUse::new())), gate).unwrap();
    let final_model = models::default_model();
    let (vad, diarization) = (RoleChoice::Model(ModelId::new("silero-vad")), RoleChoice::Auto);
    let user = UserChoices { final_model: &final_model, language: "en", vad: &vad, diarization: &diarization };
    let on_disk = |id: &ModelId| client.status(id).is_on_disk();
    let view = EngineView { on_disk: &on_disk, features: &features };

    // The file three times, with 4 s of silence between: three chunks.
    let once = sayso_host::jobs::audio::read(&audio).unwrap();
    let silence = vec![0i16; 4 * 16_000];
    let three = [once.samples(), &silence, once.samples(), &silence, once.samples()].concat();
    let three_path = cache.path().join("three.wav");
    std::fs::write(&three_path, sayso_core::stt::encode_wav(&three)).unwrap();
    let shift = once.len_ms() + 4_000;

    for (path, vad_model) in [(&audio, None), (&audio, Some("basic")), (&three_path, None)] {
        let request = TranscribeFile { path: path.clone(), diarize: Some(true), vad_model: vad_model.map(String::from), ..Default::default() };
        let job = resolve(&request, &user, &view).unwrap_or_else(|e| panic!("refused: {e:?}"));
        println!("job: {job:?}");
        assert_eq!(job.vad == VadRun::Basic, vad_model.is_some());
        let events = Arc::new(Events::default());
        let started = Instant::now();
        queue.submit(1, job, events.clone(), |_| {}).unwrap();
        let events = events.wait_end(Duration::from_secs(300));
        println!("{} events in {} ms", events.len(), started.elapsed().as_millis());
        let mut segments = Vec::new();
        let mut last_end = 0;
        for event in &events {
            match event {
                Event::Segment(s) => {
                    println!("  segment {}..{} {:?}", s.segment.start_ms, s.segment.end_ms, s.segment.text);
                    assert!(s.segment.start_ms >= last_end, "segments in order");
                    last_end = s.segment.end_ms;
                    segments.push(s.segment.start_ms);
                }
                Event::Speakers(s) => println!("  speakers {:?}", s.turns.iter().map(|t| (t.start_ms, t.end_ms, t.speaker.as_str())).collect::<Vec<_>>()),
                Event::JobProgress { fraction, .. } => println!("  progress {fraction:.3}"),
                other => println!("  {other:?}"),
            }
        }
        assert!(!segments.is_empty());
        if path == &three_path {
            // Each copy has a segment that starts one copy later than the first segment.
            // The engine can cut a copy into sentences, so there can be more segments.
            println!("one copy and its silence: {shift} ms");
            for copy in 1..3 {
                let want = segments[0] + copy * shift;
                assert!(segments.iter().any(|&start| start.abs_diff(want) <= 300), "no segment near {want} ms: {segments:?}");
            }
        }
        assert!(events.iter().any(|e| matches!(e, Event::Speakers(s) if !s.turns.is_empty())));
        let Some(Event::JobDone { text, .. }) = events.last() else { panic!("the job failed: {:?}", events.last()) };
        assert!(!text.is_empty());
    }
}
