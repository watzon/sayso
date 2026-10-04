//! End to end test with the real Swift sidecar. Needs the release build and the Parakeet
//! models from the engine spike, so it is ignored by default:
//!
//! ```text
//! (cd native/macos/SaysoEngine && swift build -c release)
//! cargo test -p sayso-engine-client --test real_engine -- --ignored --nocapture
//! ```
//!
//! The spike models are only read. The test links them into a models directory under
//! `target/`, where the CTC vocabulary helper is downloaded (about 100 MB, once).
#![cfg(target_os = "macos")]

use sayso_core::models;
use sayso_core::stt::{EngineEvent, ModelStatus, SessionOptions};
use sayso_engine_client::EngineClient;
use sayso_platform::SttBackend;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Samples of a 16 kHz mono 16-bit WAV file, as floats.
fn read_wav(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[0..4], b"RIFF");
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        if &bytes[at..at + 4] == b"data" {
            let end = (at + 8 + size).min(bytes.len());
            let (pairs, _) = bytes[at + 8..end].as_chunks::<2>();
            return pairs
                .iter()
                .map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0)
                .collect();
        }
        at += 8 + size + (size & 1);
    }
    panic!("no data chunk in {}", path.display());
}

#[test]
#[ignore = "needs the real sidecar and the spike models"]
fn real_sidecar_transcribes_and_streams() {
    let root = workspace();
    let engine = EngineClient::default_engine_path();
    assert!(
        engine.exists(),
        "build the sidecar first: {}",
        engine.display()
    );

    let spike_models = root.join("spikes/engine/.models/parakeet-unified-en-0.6b");
    assert!(
        spike_models.exists(),
        "spike models are missing: {}",
        spike_models.display()
    );
    let models_dir = root.join("target/engine-it-models");
    std::fs::create_dir_all(&models_dir).unwrap();
    let link = models_dir.join("parakeet-unified-en-0.6b");
    if !link.exists() {
        std::os::unix::fs::symlink(&spike_models, &link).unwrap();
    }
    let cache = tempfile::TempDir::new().unwrap();

    let started = Instant::now();
    let client = EngineClient::spawn(engine, models_dir, cache.path().to_path_buf()).unwrap();
    println!(
        "spawn + hello + list_models: {} ms",
        started.elapsed().as_millis()
    );
    let events = client.subscribe();

    // List: every catalog model has a status from the disk check.
    let model = models::default_model();
    for info in client.catalog() {
        println!("  {:<24} {:?}", info.id, client.status(&info.id));
    }

    // Download: the Parakeet folder is complete except for the CTC helper, so only that downloads.
    if client.status(&model) != ModelStatus::Downloaded {
        let t = Instant::now();
        client.download(&model).unwrap();
        let mut progress_events = 0;
        loop {
            match events
                .recv_timeout(Duration::from_secs(300))
                .expect("download finished")
            {
                EngineEvent::ModelStatus { model: m, status } if m == model => match status {
                    ModelStatus::Downloading { .. } => progress_events += 1,
                    ModelStatus::Downloaded => break,
                    ModelStatus::Failed { message } => panic!("download failed: {message}"),
                    _ => {}
                },
                _ => {}
            }
        }
        println!(
            "download (CTC helper): {} ms, {progress_events} progress events",
            t.elapsed().as_millis()
        );
    }
    assert_eq!(client.status(&model), ModelStatus::Downloaded);

    // Load.
    let t = Instant::now();
    client.load(&model).unwrap();
    println!("load: {} ms", t.elapsed().as_millis());
    assert_eq!(client.status(&model), ModelStatus::Ready);

    let samples = read_wav(&root.join("spikes/engine/audio/short.wav"));
    println!(
        "audio: {} samples, {:.2} s",
        samples.len(),
        samples.len() as f64 / 16_000.0
    );
    let mut options = SessionOptions {
        final_model: model.clone(),
        preview_model: Some(model.clone()),
        language: "en".into(),
        vocabulary: vec![],
    };

    // Final pass, without and with vocabulary boosting.
    let t = Instant::now();
    let plain = client.transcribe(&samples, &options).unwrap();
    println!(
        "transcribe: {} ms round trip, {} ms in engine\n  {:?}",
        t.elapsed().as_millis(),
        plain.elapsed_ms,
        plain.text
    );
    assert!(
        plain.text.to_lowercase().contains("dictation test"),
        "{}",
        plain.text
    );

    options.vocabulary = vec!["Sayso".into(), "Kubernetes".into()];
    let t = Instant::now();
    let boosted = client.transcribe(&samples, &options).unwrap();
    println!(
        "transcribe + vocabulary: {} ms round trip, {} ms in engine\n  {:?}",
        t.elapsed().as_millis(),
        boosted.elapsed_ms,
        boosted.text
    );
    assert!(
        boosted.text.to_lowercase().contains("dictation test"),
        "{}",
        boosted.text
    );

    // Live preview: 160 ms chunks, as fast as possible.
    let t = Instant::now();
    client.start_stream(1, &options).unwrap();
    println!("stream_start: {} ms", t.elapsed().as_millis());
    let t = Instant::now();
    for chunk in samples.chunks(2560) {
        client.push_audio(1, chunk).unwrap();
    }
    println!(
        "push_audio for the whole clip: {} ms",
        t.elapsed().as_millis()
    );
    client.end_stream(1);

    let mut partials = 0;
    let mut last = String::new();
    let mut first_partial_ms = None;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline && !last.to_lowercase().contains("5550134") {
        if let Ok(EngineEvent::Partial {
            session: 1,
            committed,
            tentative,
        }) = events.recv_timeout(Duration::from_secs(5))
        {
            assert!(tentative.is_empty());
            partials += 1;
            first_partial_ms.get_or_insert(t.elapsed().as_millis());
            last = committed;
        }
    }
    println!(
        "stream: {partials} partials, first after {} ms, all after {} ms\n  {last:?}",
        first_partial_ms.unwrap_or(0),
        t.elapsed().as_millis()
    );
    assert!(partials > 5);
    assert!(last.to_lowercase().contains("dictation test"), "{last}");
    assert_eq!(client.dropped_audio_frames(), 0);
}

#[test]
#[ignore = "needs the real sidecar and Apple Intelligence on this Mac"]
fn real_sidecar_runs_the_language_model() {
    use sayso_core::enhance::{EnhanceError, LanguageModel};

    let dir = workspace().join("target/engine-it-language-model");
    let client = EngineClient::spawn(
        EngineClient::default_engine_path(),
        dir.join("models"),
        dir.join("cache"),
    )
    .expect("the sidecar starts");
    let name = client
        .model_name()
        .expect("Apple Intelligence is available");
    println!("model: {name}");

    let instructions = "You edit dictated text. The user message holds a transcript between <transcript> tags. \
                        Remove filler words. Fix punctuation and capitalization. Return the edited text.";
    let prompt = "<transcript>\num so the meeting is uh on friday at noon\n</transcript>";
    let started = Instant::now();
    let reply = client
        .generate(instructions, prompt, Some(0.0), Duration::from_secs(20))
        .expect("the model answers");
    println!("{:?} in {:?}", reply.text, started.elapsed());
    assert_eq!(reply.model, name);
    assert!(reply.text.to_lowercase().contains("friday"), "{}", reply.text);

    let late = client.generate(
        "Write a story of 3000 words.",
        "go",
        None,
        Duration::from_millis(200),
    );
    assert_eq!(late.unwrap_err(), EnhanceError::Timeout(200));
}

/// The dictionary of the two tests below.
fn vocabulary() -> Vec<String> {
    ["Claude Code", "Devhouse", "Fenneko", "ForgeCAD", "MayFirmOS", "Pindrop", "Sayso"].map(String::from).to_vec()
}

/// Run the "clean" style on `transcript` with the real model, `times` times.
fn clean_with_the_language_model(client: &EngineClient, transcript: &str, times: usize) -> Vec<String> {
    use sayso_core::enhance::{LanguageModel, user_message};

    let style = sayso_core::style::builtin_styles().into_iter().find(|s| s.id == "clean").expect("the clean style");
    let instructions = sayso_core::style::system_prompt(&style, &vocabulary(), transcript);
    (0..times)
        .map(|_| {
            client
                .generate(&instructions, &user_message(transcript), style.temperature, Duration::from_secs(20))
                .expect("the model answers")
                .text
        })
        .collect()
}

fn language_model_client() -> EngineClient {
    let dir = workspace().join("target/engine-it-language-model");
    EngineClient::spawn(EngineClient::default_engine_path(), dir.join("models"), dir.join("cache")).expect("the sidecar starts")
}

/// Before the style prompt left out the words that the transcript cannot
/// contain, the model added "Use Claude Code, Devhouse, …" to the first of these.
#[test]
#[ignore = "needs the real sidecar and Apple Intelligence on this Mac"]
fn the_language_model_does_not_add_dictionary_words() {
    let client = language_model_client();
    let transcripts = [
        "Commit everything you have locally, get it pushed up, make sure the CI all passes, and then let's create a new release.",
        "um so the meeting is uh on friday at noon and we need the slides by thursday",
        "can you send me the report when you get a chance thanks",
        "I think we should use the second option because it is cheaper and it ships sooner",
    ];
    for transcript in transcripts {
        for reply in clean_with_the_language_model(&client, transcript, 5) {
            println!("{reply:?}");
            let added: Vec<String> = vocabulary().into_iter().filter(|word| reply.contains(word.as_str())).collect();
            assert!(added.is_empty(), "the reply has {added:?}: {reply}");
        }
    }
}

#[test]
#[ignore = "needs the real sidecar and Apple Intelligence on this Mac"]
fn the_language_model_spells_the_dictionary_words_it_heard() {
    let client = language_model_client();
    let transcript = "um open pin drop and check if say so has an update";
    for reply in clean_with_the_language_model(&client, transcript, 5) {
        println!("{reply:?}");
        assert!(reply.contains("Pindrop") && reply.contains("Sayso"), "{reply}");
        // One sentence in, one sentence out: nothing after it.
        assert!(reply.len() < transcript.len() + 20, "{reply}");
        for word in ["Claude Code", "Devhouse", "Fenneko", "ForgeCAD", "MayFirmOS"] {
            assert!(!reply.contains(word), "the reply has {word}: {reply}");
        }
    }
}
