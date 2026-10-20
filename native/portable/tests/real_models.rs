//! Real models, real audio. Ignored by default, because they download
//! Parakeet TDT v3 (671 MB) and Whisper tiny (78 MB) from Hugging Face:
//!
//! ```text
//! cd native/portable
//! cargo test --release --test real_models -- --ignored --nocapture --test-threads=1
//! ```
//!
//! The models go to `native/portable/target/test-models` and stay there, so a
//! second run downloads nothing.

use crossbeam_channel::Receiver;
use sayso_core::models::{self, ModelId, ModelInfo};
use sayso_core::stt::{EngineEvent, ModelStatus, SessionOptions};
use sayso_engine::audio::{read_wav, rms};
use sayso_engine::preview::{Config, Decoder, Preview, drive};
use sayso_engine::recognizer::{Parakeet, Recognizer};
use sayso_engine_client::EngineClient;
use sayso_platform::SttBackend;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn models_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-models")
}

fn clip(name: &str) -> Vec<f32> {
    read_wav(&root().join("spikes/engine/audio").join(name)).unwrap()
}

/// The English test clips, each with a word that must be heard.
const CLIPS: [(&str, &str); 2] = [("short.wav", "dictation"), ("long30.wav", "deployment")];

fn heard(text: &str, word: &str) -> bool {
    text.to_lowercase().contains(word)
}

fn client() -> (EngineClient, Receiver<EngineEvent>) {
    let client = EngineClient::spawn(
        PathBuf::from(env!("CARGO_BIN_EXE_SaysoEngine")),
        models_dir(),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-cache"),
    )
    .unwrap();
    let events = client.subscribe();
    (client, events)
}

fn info(id: &str) -> ModelInfo {
    models::find(&ModelId::new(id)).unwrap_or_else(|| panic!("{id} is in the catalog"))
}

fn ensure_downloaded(client: &EngineClient, events: &Receiver<EngineEvent>, info: &ModelInfo) {
    if client.status(&info.id).is_on_disk() {
        return;
    }
    let started = Instant::now();
    client.download(&info.id).unwrap();
    let mut last_print = Instant::now();
    loop {
        match events
            .recv_timeout(Duration::from_secs(900))
            .expect("download progress")
        {
            EngineEvent::ModelStatus { model, status } if model == info.id => match status {
                ModelStatus::Downloading {
                    fraction,
                    bytes_done,
                    bytes_total,
                } => {
                    if last_print.elapsed() > Duration::from_secs(5) {
                        last_print = Instant::now();
                        println!(
                            "  downloading {:.0}% ({bytes_done} of {bytes_total} bytes)",
                            fraction * 100.0
                        );
                    }
                }
                ModelStatus::Downloaded => break,
                ModelStatus::Failed { message, .. } => {
                    panic!("{}: download failed: {message}", info.id)
                }
                _ => {}
            },
            _ => {}
        }
    }
    println!("  download: {} s", started.elapsed().as_secs());
}

fn options(final_model: &ModelInfo, preview: Option<&ModelInfo>) -> SessionOptions {
    SessionOptions {
        final_model: final_model.id.clone(),
        preview_model: preview.map(|p| p.id.clone()),
        language: "en".into(),
        vocabulary: vec![],
    }
}

fn final_passes(client: &EngineClient, info: &ModelInfo) -> Vec<String> {
    let started = Instant::now();
    client.load(&info.id).unwrap();
    println!("  load: {} ms", started.elapsed().as_millis());
    assert_eq!(client.status(&info.id), ModelStatus::Ready);
    let mut texts = Vec::new();
    for (name, word) in CLIPS {
        let samples = clip(name);
        let seconds = samples.len() as f64 / 16_000.0;
        let result = client.transcribe(&samples, &options(info, None)).unwrap();
        println!(
            "  {name}: {seconds:.1} s of audio in {} ms ({:.0}x real time)\n    {:?}",
            result.elapsed_ms,
            seconds * 1000.0 / result.elapsed_ms.max(1) as f64,
            result.text
        );
        assert!(
            heard(&result.text, word),
            "{}: {name}: {:?}",
            info.id,
            result.text
        );
        texts.push(result.text);
    }
    texts
}

/// Words of `a` that also appear in `b`, as a fraction of the words of `a`.
fn overlap(a: &str, b: &str) -> f64 {
    let clean = |s: &str| -> Vec<String> {
        s.split_whitespace()
            .map(|w| {
                w.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase()
            })
            .filter(|w| !w.is_empty())
            .collect()
    };
    let (a, b) = (clean(a), clean(b));
    if a.is_empty() {
        return 0.0;
    }
    a.iter().filter(|w| b.contains(w)).count() as f64 / a.len() as f64
}

#[test]
#[ignore = "downloads Parakeet TDT v3 (671 MB)"]
fn parakeet_final_pass_and_live_preview() {
    let (client, events) = client();
    let info = info("parakeet-tdt-v3");
    println!("== {}", info.id);
    ensure_downloaded(&client, &events, &info);
    let texts = final_passes(&client, &info);

    // Stream short.wav in 160 ms chunks at the speed of speech.
    let samples = clip("short.wav");
    let session = 7;
    client
        .start_stream(session, &options(&info, Some(&info)))
        .unwrap();
    let started = Instant::now();
    for (i, chunk) in samples.chunks(2560).enumerate() {
        client.push_audio(session, chunk).unwrap();
        let due = Duration::from_millis(160 * (i as u64 + 1));
        if let Some(wait) = due.checked_sub(started.elapsed()) {
            std::thread::sleep(wait);
        }
    }
    client.end_stream(session);
    // The final pass comes right after the end of the stream, like in the app.
    let final_started = Instant::now();
    let result = client
        .transcribe(&samples, &options(&info, Some(&info)))
        .unwrap();
    let final_wall = final_started.elapsed().as_millis();

    let mut partials = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while let Ok(event) = events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        if let EngineEvent::Partial {
            session: s,
            committed,
            tentative,
        } = event
        {
            assert_eq!(s, session);
            partials.push(format!("{committed} | {tentative}"));
        }
    }
    println!("  stream: {} partials", partials.len());
    for p in &partials {
        println!("    {p}");
    }
    println!(
        "  final pass after the stream: {} ms in the engine, {final_wall} ms wall",
        result.elapsed_ms
    );
    assert!(partials.len() >= 3, "partials arrive while audio streams");
    let last = partials.last().unwrap().replace(" | ", " ");
    let similarity = overlap(&texts[0], &last);
    println!(
        "  preview end vs final pass: {:.0}% of the final words",
        similarity * 100.0
    );
    assert!(
        similarity >= 0.8,
        "preview end {last:?} vs final {:?}",
        texts[0]
    );
    assert!(heard(&last, "dictation"));
}

#[test]
#[ignore = "downloads Whisper tiny (78 MB)"]
fn whisper_tiny_final_pass() {
    let (client, events) = client();
    let info = info("whisper-tiny");
    println!("== {}", info.id);
    ensure_downloaded(&client, &events, &info);
    final_passes(&client, &info);
    // Dictionary words go in as the prompt; the call must still work.
    let mut with_words = options(&info, None);
    with_words.vocabulary = vec!["Sayso".into(), "Kubernetes".into()];
    let result = client.transcribe(&clip("short.wav"), &with_words).unwrap();
    println!("  short.wav with vocabulary: {:?}", result.text);
    assert!(heard(&result.text, "dictation"));
    // Silence gives no text.
    let silence = client
        .transcribe(&vec![0.0; 32_000], &options(&info, None))
        .unwrap();
    println!("  2 s of silence: {:?}", silence.text);
    assert_eq!(silence.text, "");
}

/// What one preview decode costs: Parakeet on 2, 5, 10, and 20 s of speech.
/// Needs the Parakeet download of the test above.
#[test]
#[ignore = "needs the Parakeet TDT v3 download"]
fn parakeet_preview_decode_cost() {
    let folder = models_dir().join("parakeet-tdt-v3");
    assert!(
        folder.join(".sayso-downloaded").exists(),
        "run parakeet_final_pass_and_live_preview first"
    );
    let started = Instant::now();
    let mut model = Parakeet::load(&folder).unwrap();
    println!("load: {} ms", started.elapsed().as_millis());
    let long = clip("long30.wav");
    println!("long30.wav rms {:.3}", rms(&long));
    let _ = model.transcribe(&long[..16_000], "en", &[]).unwrap(); // Warm up.
    for seconds in [2usize, 5, 10, 20, 29] {
        let samples = &long[..(seconds * 16_000).min(long.len())];
        let runs = 3;
        let started = Instant::now();
        let mut text = String::new();
        for _ in 0..runs {
            text = model.transcribe(samples, "en", &[]).unwrap();
        }
        let ms = started.elapsed().as_millis() / runs;
        println!(
            "{seconds:>2} s of audio: {ms} ms per decode ({:.0}x real time), {} words",
            seconds as f64 * 1000.0 / ms.max(1) as f64,
            text.split_whitespace().count()
        );
    }
}

/// Parakeet as a preview decoder that records the length and time of each decode.
struct TimedDecoder<'a> {
    model: &'a mut Parakeet,
    decodes: Vec<(usize, Duration)>,
}

impl Decoder for TimedDecoder<'_> {
    fn decode(&mut self, samples: &[f32]) -> sayso_engine::protocol::Result<String> {
        let started = Instant::now();
        let text = self.model.transcribe(samples, "en", &[]);
        self.decodes.push((samples.len(), started.elapsed()));
        text
    }
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// The words of `reference` that `text` misses and the words that `text`
/// adds, by a longest common subsequence of the words.
fn word_diff(reference: &[String], text: &[String]) -> (Vec<String>, Vec<String>) {
    let (n, m) = (reference.len(), text.len());
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if reference[i] == text[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut missing, mut extra) = (Vec::new(), Vec::new());
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && reference[i] == text[j] {
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            extra.push(format!("{} (after word {i})", text[j]));
            j += 1;
        } else {
            missing.push(format!("{} (word {i})", reference[i]));
            i += 1;
        }
    }
    (missing, extra)
}

/// The live preview over a long recording with several commit thresholds:
/// the cost of the decodes, and the words that the preview loses or repeats
/// at the commit points, against one final pass over the whole recording.
/// The preview logic runs without threads, with a tick every 640 ms of audio
/// (the worker ticks every 600 ms). `SAYSO_PREVIEW_WAV` picks the recording
/// (default `long30.wav`; 60 s or more of speech shows several commits).
#[test]
#[ignore = "needs the Parakeet TDT v3 download"]
fn parakeet_preview_commit_thresholds() {
    let folder = models_dir().join("parakeet-tdt-v3");
    assert!(
        folder.join(".sayso-downloaded").exists(),
        "run parakeet_final_pass_and_live_preview first"
    );
    let mut model = Parakeet::load(&folder).unwrap();
    let samples = match std::env::var_os("SAYSO_PREVIEW_WAV") {
        Some(path) => read_wav(Path::new(&path)).unwrap(),
        None => clip("long30.wav"),
    };
    let seconds = samples.len() as f64 / 16_000.0;
    let reference = model.transcribe(&samples, "en", &[]).unwrap();
    println!(
        "{seconds:.1} s of audio, final pass: {} words",
        words(&reference).len()
    );
    let ms = |n: usize| n * 16;
    // (commit after, keep, search), in ms.
    let configs = [
        (20_000, 4_000, 8_000),
        (12_000, 3_000, 5_000),
        (10_000, 3_000, 4_000),
        (10_000, 2_000, 5_000),
        (8_000, 2_000, 4_000),
    ];
    for (commit_after, keep, search) in configs {
        let config = Config {
            commit_after: ms(commit_after),
            keep: ms(keep),
            search: ms(search),
            ..Config::default()
        };
        let mut decoder = TimedDecoder {
            model: &mut model,
            decodes: Vec::new(),
        };
        let preview = Mutex::new(Preview::new(config));
        let mut committed = String::new();
        let mut boundaries = Vec::new();
        for (i, chunk) in samples.chunks(ms(160)).enumerate() {
            preview.lock().unwrap().push(chunk);
            if i % 4 == 3 {
                drive(&preview, &mut decoder, false, || false);
            }
            let now = preview.lock().unwrap().committed().to_string();
            if now != committed {
                boundaries.push(now.len());
                committed = now;
            }
        }
        drive(&preview, &mut decoder, true, || false);
        let text = preview.lock().unwrap().text();
        let total: Duration = decoder.decodes.iter().map(|(_, d)| *d).sum();
        let longest = decoder.decodes.iter().map(|(_, d)| *d).max().unwrap();
        let longest_audio = decoder.decodes.iter().map(|(n, _)| *n).max().unwrap();
        println!(
            "\n== commit after {} s, keep {} s, search {} s",
            commit_after / 1000,
            keep / 1000,
            search / 1000
        );
        println!(
            "  {} decodes, {} ms in all ({:.0} ms per second of audio), longest {} ms (on {:.1} s of audio), {} commits",
            decoder.decodes.len(),
            total.as_millis(),
            total.as_millis() as f64 / seconds,
            longest.as_millis(),
            longest_audio as f64 / 16_000.0,
            boundaries.len()
        );
        for end in &boundaries {
            let before: String = committed[..*end].chars().rev().take(40).collect();
            let before: String = before.chars().rev().collect();
            let after: String = committed[*end..].chars().take(40).collect();
            println!("  commit: ...{before} | {after}...");
        }
        let (missing, extra) = word_diff(&words(&reference), &words(&text));
        println!(
            "  vs final pass: {} missing {:?}, {} extra {:?}",
            missing.len(),
            missing,
            extra.len(),
            extra
        );
    }
}

/// `long30.wav` through the live preview at twice the speed of speech: the
/// uncommitted audio passes 20 s, so the older text gets committed.
#[test]
#[ignore = "needs the Parakeet TDT v3 download"]
fn parakeet_long_preview_commits() {
    let (client, events) = client();
    let info = info("parakeet-tdt-v3");
    ensure_downloaded(&client, &events, &info);
    client.load(&info.id).unwrap();
    let samples = clip("long30.wav");
    let session = 9;
    client
        .start_stream(session, &options(&info, Some(&info)))
        .unwrap();
    let started = Instant::now();
    for (i, chunk) in samples.chunks(2560).enumerate() {
        client.push_audio(session, chunk).unwrap();
        let due = Duration::from_millis(80 * (i as u64 + 1));
        if let Some(wait) = due.checked_sub(started.elapsed()) {
            std::thread::sleep(wait);
        }
    }
    client.end_stream(session);
    let mut partials = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while let Ok(event) = events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        if let EngineEvent::Partial {
            committed,
            tentative,
            ..
        } = event
        {
            partials.push((committed, tentative));
        }
    }
    let with_committed = partials.iter().filter(|(c, _)| !c.is_empty()).count();
    println!(
        "  {} partials, {with_committed} with committed text",
        partials.len()
    );
    if let Some((c, t)) = partials
        .iter()
        .rev()
        .find(|(c, t)| !c.is_empty() && !t.is_empty())
    {
        println!("  committed: {c:?}\n  tentative: {t:?}");
    }
    let (c, t) = partials.last().unwrap();
    let last = format!("{c} {t}");
    let final_text = client
        .transcribe(&samples, &options(&info, None))
        .unwrap()
        .text;
    let similarity = overlap(&final_text, &last);
    println!(
        "  preview end vs final pass: {:.0}% of the final words",
        similarity * 100.0
    );
    assert!(with_committed > 0, "the older text was committed");
    assert!(similarity >= 0.8, "{last:?} vs {final_text:?}");
}
