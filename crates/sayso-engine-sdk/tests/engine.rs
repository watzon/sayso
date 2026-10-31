//! The SDK against a real engine process, without models. This is the core
//! level of "Conformance tests" in `docs/engine-protocol.md`, seen from a
//! client. `common` says which engine runs.

mod common;

use common::{engine_program, start_empty};
use sayso_engine_sdk::protocol::engine::{Reply, Request};
use sayso_engine_sdk::protocol::{ErrorCode, ModelState, Role};
use sayso_engine_sdk::{Engine, EngineOptions, Error, Event, Model, Stderr, TranscribeOptions};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn hello_gives_the_version_and_the_features() {
    let (engine, _models) = start_empty();
    assert!(!engine.version().is_empty());
    assert!(engine.has_feature("catalog"), "features: {:?}", engine.features());
    assert!(engine.has_feature("segments"));
    assert!(!engine.has_feature("no-such-feature"));
    assert!(engine.is_running());
}

#[test]
fn an_engine_with_store_v1_gets_the_model_store() {
    let folder = tempfile::tempdir().unwrap();
    let store = folder.path().join("store");
    let options = EngineOptions::new(engine_program(), folder.path().join("models")).model_store(&store);
    let engine = Engine::start(options).unwrap();
    assert!(engine.has_feature("store_v1"), "this test needs an engine with the store: {:?}", engine.features());
    assert_eq!(engine.model_store(), Some(store.as_path()));
    assert!(store.join("artifacts").is_dir(), "the engine did not open the store");
}

/// An engine of an older app does not get the variable, and stays the only process.
#[cfg(unix)]
#[test]
fn an_engine_without_store_v1_does_not_get_the_model_store() {
    use std::os::unix::fs::PermissionsExt;
    let folder = tempfile::tempdir().unwrap();
    let program = folder.path().join("old-engine");
    let starts = folder.path().join("starts");
    let script = format!(
        r#"#!/bin/sh
echo "[$SAYSO_MODEL_STORE]" >> "{}"
while read -r line; do
  id=$(printf '%s' "$line" | sed -E 's/.*"id":"([^"]*)".*/\1/')
  case "$line" in
    *'"hello"'*) printf '{{"type":"hello","id":"%s","version":"0.12.0","protocol":1,"features":[]}}\n' "$id" ;;
    *) printf '{{"type":"ok","id":"%s"}}\n' "$id"; exit 0 ;;
  esac
done
"#,
        starts.display()
    );
    std::fs::write(&program, script).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();

    let engine = Engine::start(EngineOptions::new(&program, folder.path()).model_store(folder.path().join("store"))).unwrap();

    assert_eq!(engine.version(), "0.12.0");
    assert_eq!(engine.model_store(), None);
    assert_eq!(std::fs::read_to_string(&starts).unwrap(), "[]\n");
}

#[test]
fn list_models_gives_the_built_in_catalog_and_a_state_for_each_model() {
    let (engine, _models) = start_empty();
    let catalog = engine.list_models().unwrap();
    assert!(!catalog.models.is_empty(), "the catalog has models");
    for model in &catalog.models {
        assert!(!model.id.is_empty() && !model.engine.is_empty(), "{model:?} has an id and an engine object");
        assert!(!model.roles.is_empty(), "{model:?} has roles");
    }
    assert!(catalog.models.iter().any(|m| m.roles.contains(&Role::Final)));
    assert!(catalog.models.iter().any(|m| m.roles.contains(&Role::Vad)), "the catalog has a VAD model");
    // The models folder is empty, so no model is on disk. A model of the system (Apple Speech) has no files.
    assert_eq!(catalog.states.len(), catalog.models.len(), "one model_state for each model");
    let on_disk: Vec<_> = catalog.states.iter().filter(|s| s.state != ModelState::NotDownloaded).map(|s| s.model.as_str()).collect();
    assert!(on_disk.iter().all(|id| *id == "apple-speech"), "{on_disk:?}");
}

#[test]
fn list_models_with_a_list_checks_only_those_models() {
    let (engine, _models) = start_empty();
    let catalog = engine.list_models().unwrap();
    let wanted = &catalog.models[0];
    let custom = Model::with_engine("my-model", wanted.engine.clone());
    let states = engine.check_models(&[Model::from(wanted.id.as_str()), custom]).unwrap();
    let ids: Vec<_> = states.iter().map(|s| s.model.as_str()).collect();
    assert_eq!(ids, [wanted.id.as_str(), "my-model"]);
    assert!(states.iter().all(|s| s.state == ModelState::NotDownloaded));
}

#[test]
fn an_unknown_model_is_an_error_with_a_code() {
    let (engine, _models) = start_empty();
    let error = engine.load("no-such-model").unwrap_err();
    assert!(matches!(error, Error::Engine(_)), "{error}");
    assert_eq!(error.code(), Some(&ErrorCode::UnknownModel));
    assert!(error.engine_message().is_some_and(|m| m.contains("no-such-model")), "{error}");
    assert_eq!(engine.download("no-such-model").unwrap_err().code(), Some(&ErrorCode::UnknownModel));
    // The engine goes on after an error.
    assert!(engine.list_models().is_ok());
}

#[test]
fn a_model_that_is_not_loaded_and_a_stream_that_is_not_open_are_errors() {
    let (engine, _models) = start_empty();
    let error = engine.transcribe_f32("no-such-model", &[0.0; 1600], &TranscribeOptions::default()).unwrap_err();
    assert!(matches!(error, Error::Engine(_)), "{error}");
    assert!(matches!(engine.stream_end(99), Err(Error::Engine(_))), "a session that is not open");
    assert!(matches!(engine.stream_start(1, "no-such-model", None), Err(Error::Engine(_))));
    // stream_audio has no reply, so a session that is not open is no error here.
    engine.stream_audio_i16(99, &[0; 160]).unwrap();
    assert!(engine.list_models().is_ok());
}

#[test]
fn optional_requests_work_when_the_engine_has_the_feature() {
    let (engine, _models) = start_empty();
    // The spec says only that a delete of an unknown model "can get an error". The Swift engine does, and the Rust engine says ok.
    assert!(matches!(engine.delete("no-such-model"), Ok(()) | Err(Error::Engine(_))));
    assert!(matches!(engine.vad("no-such-model", "no-such-file.wav"), Err(Error::Engine(_))), "a VAD model that is not loaded");
    if engine.has_feature("language_model") {
        let model = engine.language_model().unwrap();
        assert!(model.available || model.message.is_some(), "{model:?}");
    }
    if engine.has_feature("prepare") {
        assert!(matches!(engine.prepare("no-such-model", true, &[]), Err(Error::Engine(_))), "a model that is not loaded");
    }
}

#[test]
fn requests_run_at_the_same_time_from_many_threads() {
    let (engine, _models) = start_empty();
    let engine = Arc::new(engine);
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let engine = engine.clone();
            std::thread::spawn(move || {
                for _ in 0..5 {
                    if i % 2 == 0 {
                        assert!(!engine.list_models().unwrap().models.is_empty());
                    } else {
                        assert_eq!(engine.load("no-such-model").unwrap_err().code(), Some(&ErrorCode::UnknownModel));
                    }
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
}

#[test]
fn events_come_on_the_channel_with_the_id_of_their_request() {
    let (engine, _models) = start_empty();
    let events = engine.events();
    engine.list_models().unwrap();
    let mut states = 0;
    while let Ok(event) = events.try_recv() {
        if let Event::Engine { id, event: sayso_engine_sdk::protocol::engine::Event::ModelState(_) } = event {
            assert!(id.is_some(), "the event of a request has its id");
            states += 1;
        }
    }
    assert!(states > 0, "model_state events are on the channel");
}

#[test]
fn a_call_that_waits_too_long_is_a_timeout_and_the_engine_goes_on() {
    let (engine, _models) = start_empty();
    let error = engine.call(&Request::ListModels(Default::default()), Duration::from_nanos(1)).unwrap_err();
    assert!(matches!(error, Error::Timeout { request: "list_models", .. }), "{error}");
    // The late reply has nobody to go to, and the next call works.
    assert!(matches!(engine.call(&Request::Hello, Duration::from_secs(30)).unwrap(), Reply::Hello(_)));
    let mut timeouts = engine.timeouts();
    timeouts.control = Duration::from_nanos(1);
    engine.set_timeouts(timeouts);
    assert!(matches!(engine.list_models(), Err(Error::Timeout { .. })));
}

#[test]
fn stderr_can_go_to_a_function() {
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = lines.clone();
    let models = tempfile::tempdir().unwrap();
    let options = EngineOptions::new(engine_program(), models.path()).stderr(Stderr::lines(move |line| sink.lock().unwrap().push(line.to_owned())));
    let engine = Engine::start(options).unwrap();
    engine.list_models().unwrap();
    // The text of stderr is free, so the test checks only that the engine works with a pipe there.
    assert!(engine.is_running());
}

#[test]
fn a_program_that_does_not_exist_is_an_io_error_that_names_it() {
    let models = tempfile::tempdir().unwrap();
    let error = Engine::start(EngineOptions::new(models.path().join("no-engine-here"), models.path())).unwrap_err();
    assert!(matches!(error, Error::Io(_)));
    assert!(error.to_string().contains("no-engine-here"), "{error}");
}

#[cfg(unix)]
#[test]
fn a_program_that_is_not_an_engine_is_an_error() {
    // `cat` sends the hello request back. That line has "type":"hello", but it is not a reply that this SDK accepts.
    let models = tempfile::tempdir().unwrap();
    let mut options = EngineOptions::new("cat", models.path());
    options.timeouts.startup = Duration::from_secs(5);
    let error = Engine::start(options).unwrap_err();
    assert!(matches!(error, Error::Protocol(_)), "{error}");
}

#[test]
fn shutdown_ends_the_engine() {
    let (engine, _models) = start_empty();
    let events = engine.events();
    let status = engine.shutdown().unwrap();
    assert!(status.success(), "{status}");
    // The end is on purpose, so there is no Exited event, and the channel closes.
    assert!(!matches!(events.recv_timeout(Duration::from_secs(10)), Ok(Event::Exited)));
}

#[test]
fn drop_closes_stdin_and_the_engine_exits() {
    let (engine, _models) = start_empty();
    let events = engine.events();
    drop(engine);
    // The channel closes when the stdout of the engine closes, that is, when the engine has exited.
    // Events of the dropped engine can be there first.
    loop {
        match events.recv_timeout(Duration::from_secs(10)) {
            Ok(event) => assert_ne!(event, Event::Exited, "a drop is on purpose"),
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => panic!("the engine did not exit after the drop"),
        }
    }
}

#[cfg(unix)]
#[test]
fn a_killed_engine_gives_the_exited_event_and_the_calls_fail() {
    let (engine, _models) = start_empty();
    let events = engine.events();
    let status = std::process::Command::new("kill").arg("-9").arg(engine.process_id().to_string()).status().unwrap();
    assert!(status.success());
    loop {
        match events.recv_timeout(Duration::from_secs(10)).expect("the Exited event") {
            Event::Exited => break,
            Event::Engine { .. } => {}
        }
    }
    assert!(!engine.is_running());
    assert!(matches!(engine.list_models(), Err(Error::Exited)));
    assert!(matches!(engine.stream_audio_i16(1, &[0; 16]), Err(Error::Exited)));
}
