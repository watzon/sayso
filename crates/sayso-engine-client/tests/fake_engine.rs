//! Tests against a scripted engine (`sayso-fake-engine`) that speaks the NDJSON protocol.

use crossbeam_channel::Receiver;
use sayso_core::enhance::{EnhanceError, LanguageModel};
use sayso_core::models::{ModelId, default_model};
use sayso_core::stt::{EngineEvent, EngineReason, ModelStatus, SessionOptions};
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
        ..Options::default()
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

/// Wait until the fake engine got `count` requests of type `kind`.
fn wait_for_requests(models: &TempDir, kind: &str, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let pattern = format!("\"type\":\"{kind}\"");
    while requests(models).iter().filter(|line| line.contains(&pattern)).count() < count {
        assert!(Instant::now() < deadline, "timed out waiting for {count} {kind} requests");
        std::thread::sleep(Duration::from_millis(5));
    }
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
    // An engine without `reason` gives the message only.
    let unavailable = f.client.availability().unwrap_err();
    assert_eq!(unavailable.reason, None);
    assert_eq!(unavailable.message, "Apple Intelligence is off");
}

#[test]
fn the_language_model_reason_code_reaches_the_app() {
    let f = fixture();
    let file = f.models.path().join("no-language-model");
    std::fs::write(&file, "intelligence_off").unwrap();
    let unavailable = f.client.availability().unwrap_err();
    assert_eq!(unavailable.reason, Some(EngineReason::IntelligenceOff));
    assert_eq!(unavailable.message, "Apple Intelligence is off");
    // A code that this version does not know is the same as no code.
    std::fs::write(&file, "from_the_future").unwrap();
    assert_eq!(f.client.availability().unwrap_err().reason, None);
}

#[test]
fn a_failed_load_keeps_its_reason_code() {
    let f = fixture();
    let model = ModelId::new("whisper-small");
    std::fs::write(f.models.path().join("fail-load"), "load_failed").unwrap();
    assert!(f.client.load(&model).is_err());
    assert_eq!(
        f.client.status(&model),
        ModelStatus::Failed { message: "load stopped".into(), reason: Some(EngineReason::LoadFailed) }
    );
}

#[test]
fn a_failed_download_keeps_its_reason_code_after_the_error_reply() {
    let f = fixture();
    let events = f.client.subscribe();
    let model = ModelId::new("whisper-small");
    std::fs::write(f.models.path().join("fail-download"), "incomplete_download").unwrap();
    f.client.download(&model).unwrap();
    wait_for(&events, "failed event", |e| {
        matches!(e, EngineEvent::ModelStatus { status: ModelStatus::Failed { .. }, .. }).then_some(())
    });
    // Give the `error` reply, which follows the event, time to arrive.
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        f.client.status(&model),
        ModelStatus::Failed { message: "download stopped".into(), reason: Some(EngineReason::IncompleteDownload) }
    );
}

#[test]
fn a_failed_download_from_an_older_engine_has_no_reason() {
    let f = fixture();
    let events = f.client.subscribe();
    let model = ModelId::new("whisper-small");
    std::fs::write(f.models.path().join("fail-download"), "").unwrap();
    f.client.download(&model).unwrap();
    wait_for(&events, "failed event", |e| {
        matches!(e, EngineEvent::ModelStatus { status: ModelStatus::Failed { .. }, .. }).then_some(())
    });
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        f.client.status(&model),
        ModelStatus::Failed { message: "download stopped".into(), reason: None }
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

/// The app downloads and deletes the VAD and diarization models through the
/// client, and the client checks their disk state with the speech models.
#[test]
fn vad_and_diarization_models_download_with_their_engine_object() {
    let f = fixture();
    let list = requests(&f.models).into_iter().find(|r| r.contains(r#""type":"list_models""#)).unwrap();
    for (n, model) in ["silero-vad", "speaker-diarization"].into_iter().enumerate() {
        assert!(list.contains(&format!(r#""id":"{model}""#)), "{model} is not in {list}");
        let model = ModelId::new(model);
        assert_eq!(f.client.status(&model), ModelStatus::NotDownloaded);
        f.client.download(&model).unwrap();
        wait_for_requests(&f.models, "download", n + 1);
        let info = sayso_core::models::find_local(&model).unwrap();
        let sent = requests(&f.models).into_iter().rfind(|r| r.contains(r#""type":"download""#)).unwrap();
        let engine = serde_json::to_string(&info.engine).unwrap();
        assert!(sent.contains(&engine) && sent.contains(&format!(r#""size_bytes":{}"#, info.size_bytes)), "{sent}");
    }
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
fn wait_ready_wakes_when_the_model_is_ready() {
    let f = fixture();
    let model = default_model();
    let client = f.client.clone();
    let waited = model.clone();
    let wait = thread::spawn(move || {
        let started = Instant::now();
        let status = client.wait_ready(&waited, started + Duration::from_secs(10));
        (status, started.elapsed())
    });
    thread::sleep(Duration::from_millis(200));
    f.client.load(&model).unwrap();
    let (status, elapsed) = wait.join().unwrap();
    assert_eq!(status, ModelStatus::Ready);
    assert!(elapsed >= Duration::from_millis(200), "returned before the load, after {elapsed:?}");
    assert!(elapsed < Duration::from_secs(5), "took {elapsed:?}");
}

#[test]
fn wait_ready_follows_the_model_through_a_restart() {
    let models = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let options = Options {
        backoff_initial: Duration::from_millis(300),
        backoff_max: Duration::from_millis(300),
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
    let model = default_model();
    client.load(&model).unwrap();
    let _ = client.transcribe(&[0.0; 160], &options_for("crash-once"));
    // The crash takes the model out of memory; the restart loads it again.
    wait_for(&events, "the model out of memory", |e| {
        matches!(e, EngineEvent::ModelStatus { model: m, status: ModelStatus::Downloaded } if *m == model).then_some(())
    });
    let started = Instant::now();
    assert_eq!(client.wait_ready(&model, started + Duration::from_secs(10)), ModelStatus::Ready);
    assert!(started.elapsed() >= Duration::from_millis(100), "returned before the restart, after {:?}", started.elapsed());
}

#[test]
fn wait_ready_gives_up_at_the_deadline() {
    let f = fixture();
    let started = Instant::now();
    let status = f.client.wait_ready(&ModelId::new("whisper-small"), started + Duration::from_millis(200));
    assert_eq!(status, ModelStatus::NotDownloaded);
    assert!(started.elapsed() >= Duration::from_millis(200));
    assert!(started.elapsed() < Duration::from_secs(2), "took {:?}", started.elapsed());
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
    assert!(alive(&pid));
    drop(f.client);
    wait_until_gone(&pid);
}

#[cfg(unix)]
fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn alive(pid: &str) -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().any(|w| w == pid.trim()))
        .unwrap_or(false)
}

fn wait_until_gone(pid: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(pid) {
        assert!(Instant::now() < deadline, "engine {pid} is still running");
        thread::sleep(Duration::from_millis(20));
    }
}

/// Start the fake engine with marker files in its models folder.
fn start_with_markers(markers: &[(&str, &str)], options: Options) -> (TempDir, TempDir, EngineClient, Receiver<EngineEvent>) {
    let models = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    for (name, text) in markers {
        std::fs::write(models.path().join(name), text).unwrap();
    }
    let (client, events) =
        EngineClient::start_with(PathBuf::from(FAKE_ENGINE), models.path().to_path_buf(), cache.path().to_path_buf(), options)
            .expect("the fake engine runs");
    (models, cache, client, events)
}

#[test]
fn start_returns_before_the_engine_answers() {
    let default = default_model();
    let downloaded = format!("downloaded-{default}");
    let began = Instant::now();
    let (models, _cache, client, events) = start_with_markers(&[("hello-delay-ms", "800"), (&downloaded, "")], fast_options());
    assert!(began.elapsed() < Duration::from_millis(400), "start waited for the hello: {:?}", began.elapsed());
    // Nothing is known about the disk yet.
    assert_eq!(client.status(&default), ModelStatus::NotDownloaded);

    // The receiver has the events from the start on: the status from the
    // refresh, then `Started`.
    let mut statuses = Vec::new();
    wait_for(&events, "Started", |e| match e {
        EngineEvent::ModelStatus { model, status } if *model == default => {
            statuses.push(status.clone());
            None
        }
        EngineEvent::Started => Some(()),
        EngineEvent::StartFailed { message } => panic!("the start failed: {message}"),
        _ => None,
    });
    assert!(began.elapsed() >= Duration::from_millis(800));
    assert_eq!(statuses, [ModelStatus::Downloaded]);
    assert_eq!(client.status(&default), ModelStatus::Downloaded);
    let kinds: Vec<String> = requests(&models).into_iter().take(2).collect();
    assert!(kinds[0].contains(r#""type":"hello""#) && kinds[1].contains(r#""type":"list_models""#), "{kinds:?}");
}

#[test]
fn an_engine_that_ends_at_hello_fails_the_start_and_stays_stopped() {
    let (models, _cache, client, events) = start_with_markers(&[("exit-at-hello", "")], fast_options());
    let message = wait_for(&events, "StartFailed", |e| match e {
        EngineEvent::StartFailed { message } => Some(message.clone()),
        EngineEvent::Crashed { .. } | EngineEvent::Restarted | EngineEvent::Started => panic!("unexpected {e:?}"),
        _ => None,
    });
    assert!(message.contains("engine"), "{message}");
    // No restart: the engine got one hello only, and calls fail at once.
    thread::sleep(Duration::from_millis(300));
    let hellos = requests(&models).iter().filter(|r| r.contains(r#""type":"hello""#)).count();
    assert_eq!(hellos, 1);
    assert!(client.load(&default_model()).is_err());
    assert!(events.try_iter().all(|e| !matches!(e, EngineEvent::Crashed { .. } | EngineEvent::Restarted)));
}

#[test]
fn an_engine_that_does_not_answer_hello_is_stopped_after_the_timeout() {
    let options = Options { request_timeout: Duration::from_millis(300), ..fast_options() };
    let (models, _cache, _client, events) = start_with_markers(&[("hello-delay-ms", "10000")], options);
    let message = wait_for(&events, "StartFailed", |e| match e {
        EngineEvent::StartFailed { message } => Some(message.clone()),
        _ => None,
    });
    assert!(message.contains("did not answer hello"), "{message}");
    let pid = std::fs::read_to_string(models.path().join("pid")).unwrap();
    wait_until_gone(&pid);
}

#[test]
fn spawn_waits_for_the_hello_and_returns_its_failure() {
    let models = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    std::fs::write(models.path().join("exit-at-hello"), "").unwrap();
    let result = EngineClient::spawn_with(PathBuf::from(FAKE_ENGINE), models.path().to_path_buf(), cache.path().to_path_buf(), fast_options());
    assert!(result.is_err(), "spawn returned a client for an engine that did not answer");
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

/// The index of the first request line of this type.
fn position_of(log: &[String], kind: &str) -> Option<usize> {
    log.iter()
        .position(|line| line.contains(&format!("\"type\":\"{kind}\"")))
}

#[test]
fn stop_drops_the_preview_audio_that_waits_and_the_final_pass_keeps_the_whole_recording() {
    const FRAMES: usize = 300;
    let f = fixture();
    // An engine that falls behind: 20 ms for each frame, so 300 frames need 6 s.
    std::fs::write(f.models.path().join("slow-audio"), "20").unwrap();
    let options = options_for("final-model");
    f.client.start_stream(1, &options).unwrap();
    let recording = vec![0.1f32; FRAMES * 320];
    for frame in recording.chunks(320) {
        f.client.push_audio(1, frame).unwrap();
    }

    // The user stops. The preview ends, and the final pass starts at once.
    f.client.end_stream(1);
    // A frame that arrives after the stop is not queued.
    f.client.push_audio(1, &[0.1; 320]).unwrap();
    let started = Instant::now();
    let transcript = f.client.transcribe(&recording, &options).unwrap();
    let waited = started.elapsed();

    // The final pass reads a WAV file with the whole recording, not the preview stream.
    assert_eq!(
        transcript.text,
        format!("model=final-model bytes={}", 44 + 2 * recording.len())
    );

    let log = requests(&f.models);
    let audio = |range: &[String]| {
        range
            .iter()
            .filter(|line| line.contains("\"type\":\"stream_audio\""))
            .count()
    };
    let end = position_of(&log, "stream_end").expect("the engine got stream_end");
    let transcribe = position_of(&log, "transcribe").expect("the engine got transcribe");
    assert!(end < transcribe, "stream_end comes before the final pass");
    assert_eq!(audio(&log[end..]), 0, "no audio follows stream_end");
    // The engine had read some frames already (the pipe holds them). The rest never went out.
    let sent = audio(&log[..end]);
    assert!(
        sent < FRAMES / 2,
        "{sent} of {FRAMES} frames went out before the stop; the final pass waited {waited:?}"
    );
}

#[test]
fn a_stream_that_starts_again_after_its_stop_gets_audio() {
    let f = fixture();
    let options = options_for("final-model");
    // A stop drops the audio that still waits in the queue, so each stream
    // ends only after the engine got its frame.
    f.client.start_stream(1, &options).unwrap();
    f.client.push_audio(1, &[0.1; 320]).unwrap();
    wait_for_requests(&f.models, "stream_audio", 1);
    f.client.end_stream(1);
    f.client.start_stream(1, &options).unwrap();
    f.client.push_audio(1, &[0.1; 320]).unwrap();
    wait_for_requests(&f.models, "stream_audio", 2);
    f.client.end_stream(1);
    f.client.transcribe(&[0.1; 320], &options).unwrap();
    let log = requests(&f.models);
    // Both streams ended before the final pass, and the second one got its frame.
    let kinds: Vec<&str> = log
        .iter()
        .filter_map(|line| {
            ["stream_start", "stream_audio", "stream_end"]
                .into_iter()
                .find(|k| line.contains(&format!("\"type\":\"{k}\"")))
        })
        .collect();
    assert_eq!(kinds.iter().filter(|k| **k == "stream_start").count(), 2);
    assert_eq!(kinds.iter().filter(|k| **k == "stream_end").count(), 2);
    let second_start = kinds.iter().rposition(|k| *k == "stream_start").unwrap();
    assert!(kinds[second_start..].contains(&"stream_audio"), "{kinds:?}");
}

#[test]
fn default_engine_path_names_the_sidecar() {
    // Without SAYSO_ENGINE_PATH the dev path or a bundled copy comes back.
    if std::env::var_os("SAYSO_ENGINE_PATH").is_none() {
        let name = if cfg!(target_os = "macos") {
            "SaysoEngine"
        } else if cfg!(windows) {
            "sayso-engine.exe"
        } else {
            "sayso-engine"
        };
        assert!(EngineClient::default_engine_path().ends_with(name));
    }
}

#[test]
fn prepares_parts_and_replaces_word_lists_without_failing_the_model() {
    use sayso_platform::PartReadiness;
    let f = fixture();
    let model = default_model();
    f.client.load(&model).unwrap();
    f.client.prepare(&model, true, &["Sayso".into()]).unwrap();
    assert_eq!(f.client.part_readiness(&model, true), PartReadiness::Preparing);
    let deadline = Instant::now() + Duration::from_secs(3);
    while f.client.part_readiness(&model, false) != PartReadiness::Ready {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(f.client.part_readiness(&model, true), PartReadiness::Ready);
    f.client.prepare(&model, false, &["Parakeet".into()]).unwrap();
    assert_eq!(f.client.part_readiness(&model, true), PartReadiness::Unprepared);
    assert_eq!(f.client.part_readiness(&model, false), PartReadiness::Preparing);
    f.client.unload(&model, false).unwrap();
    thread::sleep(Duration::from_millis(300));
    assert_eq!(f.client.part_readiness(&model, false), PartReadiness::Unprepared);
}

#[test]
fn older_engine_can_reject_preparation_without_failing_the_model() {
    use sayso_platform::PartReadiness;
    let f = fixture();
    let model = default_model();
    f.client.load(&model).unwrap();
    std::fs::write(f.models.path().join("reject-prepare"), "").unwrap();
    f.client.prepare(&model, true, &["Sayso".into()]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while f.client.part_readiness(&model, true) == PartReadiness::Preparing {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(f.client.part_readiness(&model, true), PartReadiness::Failed);
    assert_eq!(f.client.status(&model), ModelStatus::Ready);
}

#[test]
fn newer_preparation_ignores_older_events_and_reuses_identical_requests() {
    use sayso_platform::PartReadiness;
    let f = fixture();
    let model = default_model();
    f.client.load(&model).unwrap();
    f.client.prepare(&model, true, &["old".into()]).unwrap();
    f.client.prepare(&model, false, &["new".into()]).unwrap();
    f.client.prepare(&model, false, &["new".into()]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while f.client.part_readiness(&model, false) != PartReadiness::Ready {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    thread::sleep(Duration::from_millis(300));
    assert_eq!(f.client.part_readiness(&model, true), PartReadiness::Unprepared);
    let requests: Vec<serde_json::Value> = std::fs::read_to_string(f.models.path().join("requests.log"))
        .unwrap().lines().map(|line| serde_json::from_str(line).unwrap())
        .filter(|v: &serde_json::Value| v["type"] == "prepare").collect();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1]["vocabulary"], serde_json::json!(["new"]));
}
