//! Tests against a scripted engine (`sayso-fake-engine`) that speaks the NDJSON protocol.

use crossbeam_channel::Receiver;
use sayso_core::enhance::{EnhanceError, LanguageModel};
use sayso_core::models::{ModelId, default_model};
use sayso_core::stt::{EngineEvent, ModelStatus, SessionOptions};
use sayso_engine_client::{EngineClient, Options};
use sayso_platform::SttBackend;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const FAKE_ENGINE: &str = env!("CARGO_BIN_EXE_sayso-fake-engine");

struct Fixture {
    models: TempDir,
    cache: TempDir,
    client: EngineClient,
}

fn fast_options() -> Options {
    Options {
        request_timeout: Duration::from_secs(10),
        long_timeout: Duration::from_secs(30),
        backoff_initial: Duration::from_millis(20),
        backoff_max: Duration::from_millis(100),
    }
}

fn fixture() -> Fixture {
    let models = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let client = EngineClient::spawn_with(
        PathBuf::from(FAKE_ENGINE),
        models.path().to_path_buf(),
        cache.path().to_path_buf(),
        fast_options(),
    )
    .expect("fake engine starts");
    Fixture {
        models,
        cache,
        client,
    }
}

fn options_for(model: &str) -> SessionOptions {
    SessionOptions {
        final_model: ModelId::new(model),
        preview_model: Some(default_model()),
        language: "en".into(),
        vocabulary: vec!["Sayso".into()],
    }
}

/// Wait for an event that `pick` accepts. Other events are skipped.
fn wait_for<T>(
    events: &Receiver<EngineEvent>,
    what: &str,
    mut pick: impl FnMut(&EngineEvent) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(event) => {
                if let Some(found) = pick(&event) {
                    return found;
                }
            }
            Err(_) => panic!("timed out waiting for {what}"),
        }
    }
}

fn requests(models: &TempDir) -> Vec<String> {
    std::fs::read_to_string(models.path().join("requests.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn wav_files(cache: &TempDir) -> usize {
    std::fs::read_dir(cache.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "wav"))
        .count()
}

#[test]
fn spawn_checks_disk_status_and_catalog_matches_core() {
    let f = fixture();
    assert_eq!(f.client.catalog(), sayso_core::models::catalog());
    // list_models ran during spawn, so the cache already knows every model.
    let default = sayso_core::models::default_model();
    assert_eq!(f.client.status(&default), ModelStatus::NotDownloaded);
    assert_eq!(
        f.client.status(&ModelId::new("not-in-catalog")),
        ModelStatus::NotDownloaded
    );
    let first = requests(&f.models).into_iter().next().unwrap();
    assert!(
        first.contains(r#""type":"hello""#),
        "first request is hello: {first}"
    );
    assert!(first.contains(r#""v":1"#));
}

#[test]
fn missing_binary_is_an_error() {
    let models = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let result = EngineClient::spawn(
        PathBuf::from("/nonexistent/SaysoEngine"),
        models.path().to_path_buf(),
        cache.path().to_path_buf(),
    );
    assert!(result.is_err());
}

#[test]
fn replies_are_matched_to_their_requests() {
    let f = fixture();
    let slow = {
        let client = f.client.clone();
        thread::spawn(move || client.transcribe(&[0.0; 1600], &options_for("slow-a")))
    };
    thread::sleep(Duration::from_millis(50));
    // The fast call starts later and finishes first, so replies arrive out of order.
    let started = Instant::now();
    let fast = f
        .client
        .transcribe(&[0.1; 3200], &options_for("fast-b"))
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "fast call was not blocked by the slow one"
    );
    let slow = slow.join().unwrap().unwrap();

    // A WAV file is 44 header bytes plus 2 bytes per sample.
    assert_eq!(fast.text, format!("model=fast-b bytes={}", 44 + 3200 * 2));
    assert_eq!(slow.text, format!("model=slow-a bytes={}", 44 + 1600 * 2));
    assert_eq!(fast.model, ModelId::new("fast-b"));
    assert_eq!(fast.elapsed_ms, 5);
    assert_eq!(wav_files(&f.cache), 0, "temporary WAV files are deleted");
}

#[test]
fn engine_errors_come_back_as_errors() {
    let f = fixture();
    let err = f.client.load(&ModelId::new("not-in-catalog")).unwrap_err();
    assert!(err.to_string().contains("unknown model"), "{err}");
    assert_eq!(wav_files(&f.cache), 0);
}

#[test]
fn the_language_model_reports_its_name_or_why_it_cannot_run() {
    let f = fixture();
    assert_eq!(f.client.model_name().unwrap(), "Fake LM");
    std::fs::write(f.models.path().join("no-language-model"), "").unwrap();
    let err = f.client.model_name().unwrap_err();
    assert_eq!(
        err,
        EnhanceError::NotConfigured("Apple Intelligence is off".into())
    );
}

#[test]
fn generate_returns_the_text_and_maps_the_error_codes() {
    let f = fixture();
    let run = |prompt: &str, timeout_ms: u64| {
        f.client.generate(
            "instructions",
            prompt,
            Some(0.0),
            Duration::from_millis(timeout_ms),
        )
    };
    let done = run("hello", 5000).unwrap();
    assert_eq!((done.text.as_str(), done.model.as_str()), ("HELLO", "Fake LM"));
    let sent = requests(&f.models).into_iter().find(|line| line.contains("\"generate\"")).unwrap();
    let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
    assert_eq!(sent["instructions"], "instructions");
    assert_eq!(sent["timeout_ms"], 5000);
    assert_eq!(sent["temperature"], 0.0);

    assert_eq!(
        run("refuse", 5000).unwrap_err(),
        EnhanceError::Refused("the model refused".into())
    );
    assert_eq!(
        run("unavailable", 5000).unwrap_err(),
        EnhanceError::NotConfigured("the model is not ready".into())
    );
    let started = Instant::now();
    assert_eq!(run("slow", 100).unwrap_err(), EnhanceError::Timeout(100));
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn events_reach_every_subscriber() {
    let f = fixture();
    let first = f.client.subscribe();
    let second = f.client.subscribe();
    let model = default_model();

    f.client.load(&model).unwrap();
    f.client
        .start_stream(9, &options_for(default_model().as_str()))
        .unwrap();
    f.client.push_audio(9, &[0.25; 320]).unwrap();

    for events in [&first, &second] {
        // Both subscribers see the same events: the Ready status and the partial.
        let (mut ready, mut partial) = (false, None);
        wait_for(events, "ready and partial", |e| {
            match e {
                EngineEvent::ModelStatus {
                    status: ModelStatus::Ready,
                    ..
                } => ready = true,
                EngineEvent::Partial {
                    session, committed, ..
                } if committed.starts_with("chunk 1") => {
                    partial = Some(*session);
                }
                _ => {}
            }
            (ready && partial.is_some()).then_some(())
        });
        assert_eq!(partial, Some(9));
    }
    assert_eq!(f.client.status(&model), ModelStatus::Ready);
    f.client.end_stream(9);
}

#[test]
fn download_progress_fills_the_status_cache() {
    let f = fixture();
    let events = f.client.subscribe();
    let model = ModelId::new("whisper-small");
    let size = sayso_core::models::find(&model).unwrap().size_bytes;

    f.client.download(&model).unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let seen = loop {
        if let status @ ModelStatus::Downloading { .. } = f.client.status(&model) {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "no download progress reached the cache"
        );
        thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(
        seen,
        ModelStatus::Downloading {
            fraction: 0.5,
            bytes_done: size / 2,
            bytes_total: size
        }
    );
    // The same change is broadcast, once (progress and state carry the same numbers).
    wait_for(&events, "downloading event", |e| {
        matches!(
            e,
            EngineEvent::ModelStatus {
                status: ModelStatus::Downloading { .. },
                ..
            }
        )
        .then_some(())
    });
    wait_for(&events, "downloaded event", |e| {
        matches!(
            e,
            EngineEvent::ModelStatus {
                status: ModelStatus::Downloaded,
                ..
            }
        )
        .then_some(())
    });
    assert_eq!(f.client.status(&model), ModelStatus::Downloaded);

    f.client.delete(&model).unwrap();
    assert_eq!(f.client.status(&model), ModelStatus::NotDownloaded);
}

#[test]
fn unload_updates_the_status_and_survives_a_restart() {
    let f = fixture();
    let events = f.client.subscribe();
    let model = default_model();
    // The fake engine lists a model with this file as downloaded.
    std::fs::write(f.models.path().join(format!("downloaded-{model}")), "").unwrap();
    f.client.load(&model).unwrap();
    assert_eq!(f.client.status(&model), ModelStatus::Ready);

    // The call does not wait for the engine, and the cache changes at once.
    f.client.unload(&model, false).unwrap();
    assert_eq!(f.client.status(&model), ModelStatus::Downloaded);
    wait_for(&events, "downloaded event", |e| {
        matches!(
            e,
            EngineEvent::ModelStatus {
                status: ModelStatus::Downloaded,
                ..
            }
        )
        .then_some(())
    });

    // After a crash, the client does not load the model again.
    let _ = f.client.transcribe(&[0.0; 160], &options_for("crash-once"));
    wait_for(&events, "Restarted", |e| {
        matches!(e, EngineEvent::Restarted).then_some(())
    });
    assert_eq!(f.client.status(&model), ModelStatus::Downloaded);
    let sent = requests(&f.models);
    assert!(
        sent.iter().any(|r| r.contains(r#""type":"unload""#)),
        "{sent:?}"
    );
    let loads = sent
        .iter()
        .filter(|r| r.contains(r#""type":"load""#))
        .count();
    assert_eq!(loads, 1, "an unloaded model stays out of memory");
}

#[test]
fn unload_with_keep_preview_leaves_the_preview_ready() {
    let f = fixture();
    let model = default_model();
    f.client.load(&model).unwrap();
    assert!(
        !f.client.preview_ready(&model),
        "a Ready model needs no flag"
    );

    f.client.unload(&model, true).unwrap();
    assert_eq!(f.client.status(&model), ModelStatus::Downloaded);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f.client.preview_ready(&model) {
        assert!(
            Instant::now() < deadline,
            "the engine did not report the preview"
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        requests(&f.models)
            .iter()
            .any(|r| r.contains(r#""keep_preview":true"#))
    );

    // The load makes the model Ready, and the flag goes away.
    f.client.load(&model).unwrap();
    assert_eq!(f.client.status(&model), ModelStatus::Ready);
    assert!(!f.client.preview_ready(&model));

    // A full unload leaves no preview.
    f.client.unload(&model, false).unwrap();
    thread::sleep(Duration::from_millis(100));
    assert!(!f.client.preview_ready(&model));
}

#[test]
fn crash_restarts_the_engine_and_loads_models_again() {
    let f = fixture();
    let events = f.client.subscribe();
    let model = default_model();
    f.client.load(&model).unwrap();
    assert_eq!(f.client.status(&model), ModelStatus::Ready);

    // The fake dies while it handles this request. The call must not wait for its timeout.
    let started = Instant::now();
    let result = f.client.transcribe(&[0.0; 160], &options_for("crash-once"));
    let err = result.expect_err("a crash fails the pending call");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "failed fast, took {:?}",
        started.elapsed()
    );
    assert!(err.to_string().contains("engine"), "{err}");

    let message = wait_for(&events, "Crashed", |e| match e {
        EngineEvent::Crashed { message } => Some(message.clone()),
        _ => None,
    });
    assert!(message.contains("exited"), "{message}");
    wait_for(&events, "Restarted", |e| {
        matches!(e, EngineEvent::Restarted).then_some(())
    });

    // The model is loaded again in the new process, and the engine answers.
    assert_eq!(f.client.status(&model), ModelStatus::Ready);
    let loads = requests(&f.models)
        .iter()
        .filter(|r| r.contains(r#""type":"load""#))
        .count();
    assert_eq!(loads, 2, "load was sent again after the restart");
    let again = f
        .client
        .transcribe(&[0.0; 160], &options_for("crash-once"))
        .unwrap();
    assert!(again.text.starts_with("model=crash-once"));
}

#[test]
fn calls_fail_fast_while_the_engine_is_down() {
    let models = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let options = Options {
        backoff_initial: Duration::from_secs(30),
        backoff_max: Duration::from_secs(30),
        ..fast_options()
    };
    let client = EngineClient::spawn_with(
        PathBuf::from(FAKE_ENGINE),
        models.path().to_path_buf(),
        cache.path().to_path_buf(),
        options,
    )
    .unwrap();
    let events = client.subscribe();
    let _ = client.transcribe(&[0.0; 160], &options_for("crash-once"));
    wait_for(&events, "Crashed", |e| {
        matches!(e, EngineEvent::Crashed { .. }).then_some(())
    });

    let started = Instant::now();
    assert!(client.load(&default_model()).is_err());
    assert!(client.push_audio(1, &[0.0; 320]).is_err());
    assert!(started.elapsed() < Duration::from_millis(500));
}

#[test]
fn dropping_the_client_stops_the_engine() {
    let f = fixture();
    let pid_file = f.models.path().join("pid");
    let pid = std::fs::read_to_string(&pid_file).unwrap();
    #[cfg(unix)]
    let alive = |pid: &str| {
        std::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    #[cfg(windows)]
    let alive = |pid: &str| {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().any(|w| w == pid.trim()))
            .unwrap_or(false)
    };
    assert!(alive(&pid));
    drop(f.client);
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(&pid) {
        assert!(Instant::now() < deadline, "engine {pid} is still running");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn push_audio_does_not_wait_for_the_engine() {
    let f = fixture();
    f.client
        .start_stream(1, &options_for(default_model().as_str()))
        .unwrap();
    let started = Instant::now();
    for _ in 0..2000 {
        f.client.push_audio(1, &[0.1; 320]).unwrap();
    }
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "took {:?}",
        started.elapsed()
    );
}

#[test]
fn default_engine_path_names_the_sidecar() {
    // Without SAYSO_ENGINE_PATH the dev path or a bundled copy comes back.
    if std::env::var_os("SAYSO_ENGINE_PATH").is_none() {
        let name = if cfg!(target_os = "macos") {
            "SaysoEngine"
        } else if cfg!(windows) {
            "SaysoEngine.exe"
        } else {
            "sayso-engine"
        };
        assert!(EngineClient::default_engine_path().ends_with(name));
    }
}
