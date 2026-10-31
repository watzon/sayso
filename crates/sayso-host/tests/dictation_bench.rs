//! The dictation benchmark of step 6 (`docs/benchmark.md`, "Dictation while
//! an extension works"): the time of the final pass of a dictation, with
//! nothing else on the engine, while an extension runs a job, and while an
//! extension runs a live session. Ignored by default:
//!
//! ```text
//! (cd native/macos/SaysoEngine && swift build -c release)
//! SAYSO_CONF_MODELS=/tmp/conf/swift-models \
//!   cargo test --release -p sayso-host --test dictation_bench -- --ignored --nocapture
//! ```
//!
//! The folder needs `parakeet-unified-en`, `silero-vad`, and
//! `speaker-diarization`, as for `jobs_real_engine.rs`. The clips come from
//! the benchmark set in `~/.cache/sayso-bench` (`manifest.jsonl`): the first
//! `SAYSO_BENCH_CLIPS` (default 15) clips of LibriSpeech clean of 5 to 15 s
//! for the dictations, and AMI meeting clips for the long file of the job and
//! the audio of the live session. Without the set, `SAYSO_CONF_AUDIO` (a
//! 16 kHz mono WAV file) is each clip and, many times, the long file.
//!
//! Each dictation does what the app does: `DictationGate::begin` when the
//! recording starts, the recording time, `SttBackend::transcribe` with the
//! final model (the time that this test measures), then `DictationGate::end`.
//! A second pass has no recording time: the final pass starts at the same
//! moment as `begin`, so a part of a job or of a live session that is in the
//! engine then is not done yet. That is the worst case.
#![cfg(target_os = "macos")]

use sayso_core::config::{Config, RoleChoice};
use sayso_core::models::ModelId;
use sayso_core::stt::SessionOptions;
use sayso_engine_client::EngineClient;
use sayso_sdk::protocol::{Event as HostEvent, JobReply, SessionReply};
use sayso_sdk::client::{Client, Event};
use sayso_host::{DownloadQuestion, HostServices};
use sayso_platform::SttBackend;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

mod common;
use common::*;

const FINAL_MODEL: &str = "parakeet-unified-en";
/// The length of the long file of the job, and of the audio of the live session.
const LONG_SECONDS: f64 = 300.0;

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
    fn ask_approval(&self, _: &sayso_host::ApprovalQuestion, _: &dyn Fn() -> bool) -> Option<bool> {
        None
    }
}

fn read_pcm(path: &Path) -> Vec<i16> {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[22..24], &[1, 0], "{}: mono", path.display());
    assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 16_000, "{}: 16 kHz", path.display());
    let data = bytes.windows(4).position(|w| w == b"data").expect("a data chunk") + 8;
    bytes[data..].as_chunks::<2>().0.iter().map(|&pair| i16::from_le_bytes(pair)).collect()
}

/// The clips of the dictations, and the clips of the long audio.
fn clips() -> (Vec<Vec<i16>>, Vec<Vec<i16>>) {
    let count: usize = std::env::var("SAYSO_BENCH_CLIPS").ok().and_then(|n| n.parse().ok()).unwrap_or(15);
    let manifest = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".cache/sayso-bench/manifest.jsonl");
    if let Ok(text) = std::fs::read_to_string(&manifest) {
        let rows: Vec<serde_json::Value> = text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect();
        let pick = |set: &str, short: bool| {
            rows.iter()
                .filter(|row| row["set"] == set)
                .filter(|row| !short || (5.0..=15.0).contains(&row["seconds"].as_f64().unwrap_or(0.0)))
                .map(|row| PathBuf::from(row["path"].as_str().unwrap()))
                .collect::<Vec<_>>()
        };
        let short: Vec<Vec<i16>> = pick("librispeech_clean", true).iter().take(count).map(|p| read_pcm(p)).collect();
        let long: Vec<Vec<i16>> = pick("ami", false).iter().map(|p| read_pcm(p)).collect();
        if short.len() == count && !long.is_empty() {
            println!("clips: {} from {}", count, manifest.display());
            return (short, long);
        }
    }
    let audio = PathBuf::from(std::env::var_os("SAYSO_CONF_AUDIO").expect("no benchmark set: set SAYSO_CONF_AUDIO"));
    println!("clips: {} times {}", count, audio.display());
    let once = read_pcm(&audio);
    (vec![once.clone(); count], vec![once])
}

/// About `LONG_SECONDS` of the long clips, with 1 s of silence between them.
fn long_audio(clips: &[Vec<i16>]) -> Vec<i16> {
    let mut all = Vec::new();
    for clip in clips.iter().cycle() {
        if all.len() as f64 >= LONG_SECONDS * 16_000.0 {
            break;
        }
        all.extend_from_slice(clip);
        all.extend(std::iter::repeat_n(0i16, 16_000));
    }
    all
}

#[derive(Clone, Copy, PartialEq)]
enum Recording {
    /// The recording lasts as long as the clip.
    AsLongAsTheClip,
    /// No recording time: the worst case.
    None,
}

/// The time of the final pass of each clip, in ms.
fn dictate(engine: &Arc<dyn SttBackend>, gate: &sayso_host::jobs::DictationGate, clips: &[Vec<f32>], recording: Recording) -> Vec<f64> {
    let options = SessionOptions { final_model: ModelId::new(FINAL_MODEL), preview_model: None, language: Config::default().dictation.language, vocabulary: Vec::new() };
    let mut times = Vec::new();
    for clip in clips {
        gate.begin();
        if recording == Recording::AsLongAsTheClip {
            std::thread::sleep(Duration::from_secs_f64(clip.len() as f64 / 16_000.0));
        }
        let started = Instant::now();
        let transcript = engine.transcribe(clip, &options).expect("the final pass");
        times.push(started.elapsed().as_secs_f64() * 1000.0);
        gate.end();
        assert!(!transcript.text.is_empty());
        // The pause between two dictations.
        std::thread::sleep(Duration::from_millis(500));
    }
    times
}

fn median_p95(times: &[f64]) -> (f64, f64) {
    let mut sorted = times.to_vec();
    sorted.sort_by(f64::total_cmp);
    let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    (at(0.5), at(0.95))
}

/// Each case, and its times for both kinds of recording.
struct Results(Vec<(&'static str, Recording, Vec<f64>)>);

impl Results {
    fn print(&self) {
        println!();
        println!("| Case | Recording | Median (ms) | p95 (ms) | Slowest (ms) |");
        println!("|---|---|---|---|---|");
        for (case, recording, times) in &self.0 {
            let (median, p95) = median_p95(times);
            let slowest = times.iter().copied().fold(0.0, f64::max);
            let recording = match recording {
                Recording::AsLongAsTheClip => "as long as the clip",
                Recording::None => "none (worst case)",
            };
            println!("| {case} | {recording} | {median:.0} | {p95:.0} | {slowest:.0} |");
        }
    }
}

/// An extension that transcribes the long file again and again, with VAD and
/// diarization, until `stop`. Gives the number of jobs that ended.
fn run_jobs(client: Client, path: PathBuf, stop: Arc<AtomicBool>, ended: Arc<AtomicUsize>, started: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let events = client.events();
        while !stop.load(Ordering::Relaxed) {
            let reply = client.request("transcribe_file", fields(json!({"path": path, "diarize": true, "vad_model": "silero-vad"}))).unwrap();
            let job = reply.parse::<JobReply>().unwrap().job;
            loop {
                match events.recv_timeout(Duration::from_secs(600)).expect("the job goes on") {
                    Event::Host { event: HostEvent::Segment(_), .. } => started.store(true, Ordering::Relaxed),
                    Event::Host { event: HostEvent::JobProgress { fraction, .. }, .. } if fraction > 0.0 => started.store(true, Ordering::Relaxed),
                    Event::Host { event: HostEvent::JobDone { job: done, .. }, .. } if done == job => break,
                    Event::Host { event: HostEvent::JobFailed { job: failed, message, .. }, .. } if failed == job => panic!("the job failed: {message}"),
                    _ => {}
                }
            }
            ended.fetch_add(1, Ordering::Relaxed);
        }
    })
}

/// An extension that feeds the long audio to a live session in real time,
/// with diarization, until `stop`.
fn run_live(client: Client, audio: Vec<i16>, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<usize> {
    std::thread::spawn(move || {
        let reply = client.request("session_start", fields(json!({"sources": ["client"], "diarize": true}))).unwrap();
        let session = reply.parse::<SessionReply>().unwrap().session;
        let chunk = 16_000 * 160 / 1000;
        let started = Instant::now();
        let mut sent = 0usize;
        'feed: loop {
            for part in audio.chunks(chunk) {
                if stop.load(Ordering::Relaxed) {
                    break 'feed;
                }
                client.send("session_audio", fields(json!({"session": session, "pcm": sayso_sdk::protocol::pcm::encode_i16(part)}))).unwrap();
                sent += part.len();
                // Real time: wait until the audio that went out is due.
                let due = Duration::from_secs_f64(sent as f64 / 16_000.0);
                if let Some(wait) = due.checked_sub(started.elapsed()) {
                    std::thread::sleep(wait);
                }
            }
        }
        let events = client.events();
        let segments = events.try_iter().filter(|e| matches!(e, Event::Host { event: HostEvent::Segment(_), .. })).count();
        client.request("session_stop", fields(json!({"session": session}))).unwrap();
        segments
    })
}

#[test]
#[ignore = "needs the real sidecar, models on disk, and minutes of time"]
fn dictation_while_an_extension_works() {
    let models_dir = PathBuf::from(std::env::var_os("SAYSO_CONF_MODELS").expect("set SAYSO_CONF_MODELS"));
    let engine_path = EngineClient::default_engine_path();
    assert!(engine_path.exists(), "build the sidecar first: {}", engine_path.display());
    let setup = setup();
    let engine: Arc<dyn SttBackend> = Arc::new(EngineClient::spawn(engine_path, models_dir, setup.dir.path().join("cache")).unwrap());

    let (short, long) = clips();
    let short: Vec<Vec<f32>> = short.iter().map(|clip| clip.iter().map(|&s| s as f32 / 32768.0).collect()).collect();
    let seconds: Vec<f64> = short.iter().map(|clip| clip.len() as f64 / 16_000.0).collect();
    println!("dictation clips: {} s to {} s", seconds.iter().copied().fold(f64::MAX, f64::min), seconds.iter().copied().fold(0.0, f64::max));
    let long = long_audio(&long);
    let long_path = setup.dir.path().join("long.wav");
    std::fs::write(&long_path, sayso_core::stt::encode_wav(&long)).unwrap();
    println!("long audio: {:.0} s", long.len() as f64 / 16_000.0);

    let mut config = Config::default();
    config.dictation.model = ModelId::new(FINAL_MODEL);
    config.models.vad = RoleChoice::Model(ModelId::new("silero-vad"));
    write_apps(&setup.apps, &["transcribe.file", "session.live"]);
    let host = start_with(&setup, Arc::new(Services { engine: engine.clone(), config }));
    let gate = host.dictation_gate();

    // The final model is in memory, as in the app after the first dictation.
    let final_model = ModelId::new(FINAL_MODEL);
    engine.load(&final_model).expect("load the final model");
    let deadline = Instant::now() + Duration::from_secs(300);
    while engine.status(&final_model) != sayso_core::stt::ModelStatus::Ready {
        assert!(Instant::now() < deadline, "the final model did not load: {:?}", engine.status(&final_model));
        std::thread::sleep(Duration::from_millis(100));
    }
    dictate(&engine, &gate, &short[..1], Recording::None);
    let mut results = Results(Vec::new());
    let measure = |case: &'static str, results: &mut Results| {
        for recording in [Recording::AsLongAsTheClip, Recording::None] {
            results.0.push((case, recording, dictate(&engine, &gate, &short, recording)));
        }
    };

    measure("Nothing else", &mut results);

    // A job: the first segment shows that it works, then the dictations start.
    let (stop, ended, working) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
    let jobs = run_jobs(Client::connect_to(&setup.socket).unwrap(), long_path.clone(), stop.clone(), ended.clone(), working.clone());
    let deadline = Instant::now() + Duration::from_secs(120);
    while !working.load(Ordering::Relaxed) {
        assert!(Instant::now() < deadline, "the job did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    let job_started = Instant::now();
    measure("A job (VAD and diarization)", &mut results);
    let job_time = job_started.elapsed();
    stop.store(true, Ordering::Relaxed);
    jobs.join().unwrap();
    println!("jobs that ended: {} (the dictations took {:.0} s)", ended.load(Ordering::Relaxed), job_time.as_secs_f64());

    // A live session: 10 s of audio first, so that its parts are in progress.
    let stop = Arc::new(AtomicBool::new(false));
    let live = run_live(Client::connect_to(&setup.socket).unwrap(), long.clone(), stop.clone());
    std::thread::sleep(Duration::from_secs(10));
    measure("A live session (diarization)", &mut results);
    stop.store(true, Ordering::Relaxed);
    println!("live segments before the stop: {}", live.join().unwrap());

    results.print();
}
