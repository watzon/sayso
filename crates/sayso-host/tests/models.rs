//! `list_models` and `download_model` over a real socket, with a fake engine
//! and a fake user (see `common`).

use crossbeam_channel::Receiver;
use sayso_core::models::{self, ModelId, ModelTask};
use sayso_core::stt::{EngineEvent, EngineReason, ModelStatus};
use sayso_extension::protocol::{self, ErrorCode, ListModelsReply, ModelState, Role};
use sayso_extension::{Client, Event, Fields};
use sayso_platform::EngineStart;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod common;
use common::*;

struct World {
    // The host closes when it drops.
    _host: sayso_host::Host,
    _setup: Setup,
    client: Client,
    events: Receiver<Event>,
    engine: Arc<FakeEngine>,
    services: Arc<FakeServices>,
}

fn world(granted: &[&str]) -> World {
    let engine = FakeEngine::new();
    world_with(granted, Some(engine.clone()), engine)
}

fn world_with(granted: &[&str], services_engine: Option<Arc<FakeEngine>>, engine: Arc<FakeEngine>) -> World {
    let setup = setup();
    write_state(&setup.state, &here(), granted, None);
    let services = FakeServices::new(services_engine);
    let host = start_with(&setup, services.clone());
    let client = Client::connect_to(&setup.socket).unwrap();
    let events = client.events();
    World { _host: host, _setup: setup, client, events, engine, services }
}

fn first_of(task: ModelTask) -> ModelId {
    models::models_for(task).next().unwrap().id.clone()
}

/// A speech model for the final pass.
fn speech() -> ModelId {
    models::local_models().iter().find(|m| m.final_pass && m.task() == ModelTask::Speech).unwrap().id.clone()
}

fn download_request(model: &ModelId) -> Fields {
    fields(json!({"model": model.as_str(), "reason": "To write your meeting notes"}))
}

fn list(client: &Client, body: serde_json::Value) -> Vec<protocol::ModelInfo> {
    client.request("list_models", fields(body)).unwrap().parse::<ListModelsReply>().unwrap().models
}

/// The events with an id that have arrived so far, in order.
fn events_of(events: &Receiver<Event>, id: Option<&str>) -> Vec<protocol::Event> {
    events
        .try_iter()
        .filter_map(|event| match event {
            Event::Host { id: got, event } if id.is_none() || got.as_deref() == id => Some(event),
            _ => None,
        })
        .collect()
}

fn downloading(done: u64) -> ModelStatus {
    ModelStatus::Downloading { fraction: done as f32 / 100., bytes_done: done, bytes_total: 100 }
}

fn failed(message: &str) -> ModelStatus {
    ModelStatus::Failed { message: message.into(), reason: Some(EngineReason::DownloadFailed) }
}

#[test]
fn list_models_gives_the_local_models_with_their_state_and_choice() {
    let world = world(&[]);
    let (final_model, vad, diarization) = (speech(), first_of(ModelTask::Vad), first_of(ModelTask::Diarization));
    world.engine.preset(&final_model, ModelStatus::Ready);
    world.engine.preset(&vad, downloading(40));
    world.engine.preset(&diarization, ModelStatus::Downloaded);
    world.services.config.lock().dictation.model = final_model.clone();

    let listed = list(&world.client, json!({}));
    assert_eq!(listed.len(), models::local_models().len());
    let find = |id: &ModelId| listed.iter().find(|m| m.id == id.as_str()).unwrap();
    assert_eq!(find(&final_model).state, ModelState::Ready);
    assert!(find(&final_model).choice.contains(&Role::Final));
    assert_eq!(find(&vad).state, ModelState::Downloading);
    assert!(find(&vad).choice.is_empty(), "a VAD model that is not on disk is not in use");
    assert_eq!(find(&diarization).state, ModelState::Downloaded);
    assert_eq!(find(&diarization).choice, vec![Role::Diarization]);
    assert_eq!(find(&diarization).roles, vec![Role::Diarization]);
    assert!(find(&vad).size_bytes > 0 && !find(&vad).name.is_empty());
    assert!(listed.iter().all(|m| m.state != ModelState::Ready || m.id == final_model.as_str()));

    // The settings count at the next request.
    world.services.config.lock().models.vad = sayso_core::config::RoleChoice::Model(ModelId::new("basic"));
    world.engine.preset(&vad, ModelStatus::Downloaded);
    assert!(list(&world.client, json!({})).iter().all(|m| !m.choice.contains(&Role::Vad)), "Basic VAD is no model");
}

#[test]
fn list_models_can_filter_by_role() {
    let world = world(&[]);
    for (role, name) in [(Role::Final, "final"), (Role::Streaming, "streaming"), (Role::Vad, "vad"), (Role::Diarization, "diarization")] {
        let listed = list(&world.client, json!({"role": name}));
        assert!(!listed.is_empty(), "{name}");
        assert!(listed.iter().all(|m| m.roles.contains(&role)), "{name}");
    }
    assert!(list(&world.client, json!({"role": "translate"})).is_empty(), "a role that this version does not know");
}

#[test]
fn list_models_needs_an_engine() {
    let engine = FakeEngine::new();
    let world = world_with(&[], None, engine);
    let error = refused(world.client.request("list_models", Fields::new()));
    assert_eq!(error.code, Some(ErrorCode::Unsupported));
    assert!(error.message.contains("engine"), "{}", error.message);
}

#[test]
fn list_models_waits_until_the_engine_has_started() {
    let world = world(&[]);
    world.engine.set_start(EngineStart::Starting);
    let engine = world.engine.clone();
    let model = speech();
    engine.preset(&model, ModelStatus::Downloaded);
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        engine.set_start(EngineStart::Started);
    });
    let started = Instant::now();
    let listed = list(&world.client, json!({}));
    assert!(started.elapsed() >= Duration::from_millis(250), "the reply waits for the engine");
    assert_eq!(listed.iter().find(|m| m.id == model.as_str()).unwrap().state, ModelState::Downloaded, "not the empty statuses of an engine that has not answered");
    releaser.join().unwrap();

    world.engine.set_start(EngineStart::Failed);
    let error = refused(world.client.request("list_models", Fields::new()));
    assert_eq!(error.code, Some(ErrorCode::Unsupported));
    assert!(error.message.contains("did not start"), "{}", error.message);
}

#[test]
fn download_model_needs_its_capability() {
    let world = world(&[]);
    let error = refused(world.client.request("download_model", download_request(&speech())));
    assert_eq!(error.code, Some(ErrorCode::Capability));
    assert_eq!(error.capability, Some(protocol::Capability::ModelsDownload));
    assert!(world.services.asked.lock().is_empty());
}

#[test]
fn an_unknown_model_gets_unknown_model_and_no_question() {
    let world = world(&["models.download"]);
    let error = refused(world.client.request("download_model", fields(json!({"model": "no-such-model", "reason": "x"}))));
    assert_eq!(error.code, Some(ErrorCode::UnknownModel));
    // A model of a cloud provider is not a local model.
    let error = refused(world.client.request("download_model", fields(json!({"model": "openai:whisper-1", "reason": "x"}))));
    assert_eq!(error.code, Some(ErrorCode::UnknownModel));
    assert!(world.services.asked.lock().is_empty());
    assert!(world.engine.downloads.lock().is_empty());
}

#[test]
fn a_model_on_disk_gets_ok_at_once() {
    let world = world(&["models.download"]);
    let model = speech();
    world.engine.preset(&model, ModelStatus::Downloaded);
    world.client.request("download_model", download_request(&model)).unwrap();
    assert!(world.services.asked.lock().is_empty(), "no question for a model on disk");
    assert!(world.engine.downloads.lock().is_empty());
    assert!(events_of(&world.events, None).is_empty());
}

#[test]
fn an_accepted_download_sends_its_progress_before_ok() {
    let world = world(&["models.download"]);
    let model = speech();
    let info = models::find_local(&model).unwrap();
    world.services.answer(true);
    let engine = world.engine.clone();
    let id = model.clone();
    // The engine reports the download after the host asked for it.
    let driver = std::thread::spawn(move || {
        engine.wait_for_downloads(1);
        engine.set(&id, downloading(0));
        engine.set(&id, downloading(40));
        engine.set(&id, downloading(100));
        engine.set(&id, ModelStatus::Downloaded);
    });
    world.client.request("download_model", download_request(&model)).unwrap();
    driver.join().unwrap();

    assert_eq!(world.engine.downloads.lock().clone(), vec![model.clone()]);
    let asked = world.services.asked.lock().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!((asked[0].extension_id.as_str(), asked[0].model_id.as_str(), asked[0].size_bytes), (ID, model.as_str(), info.size_bytes));
    assert_eq!((asked[0].model_name.as_str(), asked[0].reason.as_str()), (info.name.as_str(), "To write your meeting notes"));

    let ids = ids_and_events(&world.events);
    assert!(ids.iter().all(|(id, _)| id.is_some()), "each event echoes the id of the request");
    let events: Vec<protocol::Event> = ids.into_iter().map(|(_, event)| event).collect();
    let states: Vec<ModelState> = events
        .iter()
        .filter_map(|event| if let protocol::Event::ModelState(state) = event { Some(state.state.clone()) } else { None })
        .collect();
    assert_eq!(states, vec![ModelState::Downloading, ModelState::Downloaded]);
    assert!(matches!(events.first(), Some(protocol::Event::ModelState(state)) if state.state == ModelState::Downloading), "model_state comes before the progress");
    let fractions: Vec<f64> = events.iter().filter_map(|event| if let protocol::Event::DownloadProgress(p) = event { Some(p.fraction) } else { None }).collect();
    assert_eq!(fractions.len(), 3);
    assert!(fractions[0] == 0.0 && (fractions[1] - 0.4).abs() < 1e-6 && fractions[2] == 1.0, "{fractions:?}");
    assert!(matches!(events.last(), Some(protocol::Event::ModelState(state)) if state.state == ModelState::Downloaded));
}

fn ids_and_events(events: &Receiver<Event>) -> Vec<(Option<String>, protocol::Event)> {
    events.try_iter().filter_map(|event| if let Event::Host { id, event } = event { Some((id, event)) } else { None }).collect()
}

#[test]
fn a_declined_download_gets_declined_and_starts_nothing() {
    let world = world(&["models.download"]);
    let model = speech();
    world.services.answer(false);
    let error = refused(world.client.request("download_model", download_request(&model)));
    assert_eq!(error.code, Some(ErrorCode::Declined));
    assert!(world.engine.downloads.lock().is_empty());
    assert!(events_of(&world.events, None).is_empty());
}

#[test]
fn a_failed_download_sends_model_state_failed_and_then_an_error() {
    let world = world(&["models.download"]);
    let model = speech();
    // A failure of an earlier try is still the status when the host asks again.
    world.engine.preset(&model, failed("old failure"));
    world.services.answer(true);
    let engine = world.engine.clone();
    let id = model.clone();
    let driver = std::thread::spawn(move || {
        engine.wait_for_downloads(1);
        engine.set(&id, downloading(10));
        engine.set(&id, failed("disk full"));
    });
    let error = refused(world.client.request("download_model", download_request(&model)));
    driver.join().unwrap();
    assert!(error.message.contains("disk full"), "{}", error.message);
    assert_eq!(error.reason.as_deref(), Some("download_failed"));

    let events = events_of(&world.events, None);
    let Some(protocol::Event::ModelState(last)) = events.last() else { panic!("{events:?}") };
    assert_eq!((last.state.clone(), last.message.as_deref(), last.reason.as_deref()), (ModelState::Failed, Some("disk full"), Some("download_failed")));
    assert!(!events.iter().any(|event| matches!(event, protocol::Event::ModelState(s) if s.message.as_deref() == Some("old failure"))), "the old failure is not this try");
}

#[test]
fn a_crash_of_the_engine_ends_the_download_with_an_error() {
    let world = world(&["models.download"]);
    let model = speech();
    world.services.answer(true);
    let engine = world.engine.clone();
    let id = model.clone();
    let driver = std::thread::spawn(move || {
        engine.wait_for_downloads(1);
        engine.set(&id, downloading(10));
        engine.emit(EngineEvent::Crashed { message: "gone".into() });
    });
    let error = refused(world.client.request("download_model", download_request(&model)));
    driver.join().unwrap();
    assert!(error.message.contains("stopped"), "{}", error.message);
}

#[test]
fn download_model_needs_an_engine() {
    let world = world_with(&["models.download"], None, FakeEngine::new());
    let error = refused(world.client.request("download_model", download_request(&speech())));
    assert_eq!(error.code, Some(ErrorCode::Unsupported));
    assert!(world.services.asked.lock().is_empty());
}

#[test]
fn a_failed_start_in_the_engine_is_an_error() {
    let world = world(&["models.download"]);
    world.services.answer(true);
    *world.engine.download_error.lock() = Some("engine is busy".into());
    let error = refused(world.client.request("download_model", download_request(&speech())));
    assert!(error.message.contains("engine is busy"), "{}", error.message);
}

#[test]
fn two_requests_for_one_model_ask_once_and_download_once() {
    let world = world(&["models.download"]);
    let model = speech();
    let ask = || world.client.request("download_model", download_request(&model));
    std::thread::scope(|scope| {
        let first = scope.spawn(ask);
        world.services.wait_for_questions(1);
        // The second request waits behind the open question.
        let second = scope.spawn(ask);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(world.services.asked.lock().len(), 1, "one question at a time");
        world.services.answer(true);
        world.engine.wait_for_downloads(1);
        std::thread::sleep(Duration::from_millis(200));
        world.engine.set(&model, downloading(50));
        world.engine.set(&model, ModelStatus::Downloaded);
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();
    });
    assert_eq!(world.services.asked.lock().len(), 1, "the second request follows the download and does not ask");
    assert_eq!(world.engine.downloads.lock().len(), 1, "one download");
}

#[test]
fn a_request_for_a_model_that_downloads_follows_it_without_a_question() {
    let world = world(&["models.download"]);
    let model = speech();
    world.engine.preset(&model, downloading(30));
    let engine = world.engine.clone();
    let id = model.clone();
    let driver = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        engine.set(&id, downloading(80));
        engine.set(&id, ModelStatus::Downloaded);
    });
    world.client.request("download_model", download_request(&model)).unwrap();
    driver.join().unwrap();
    assert!(world.services.asked.lock().is_empty());
    assert!(world.engine.downloads.lock().is_empty(), "no second download");
    let events = events_of(&world.events, None);
    // The first thing the extension hears is the progress that exists now.
    assert!(matches!(&events[0], protocol::Event::ModelState(s) if s.state == ModelState::Downloading));
    assert!(matches!(&events[1], protocol::Event::DownloadProgress(p) if p.bytes_done == 30));
    assert!(matches!(events.last(), Some(protocol::Event::ModelState(s)) if s.state == ModelState::Downloaded));
}

#[test]
fn another_request_answers_while_a_download_waits_for_the_user() {
    let world = world(&["models.download"]);
    let model = speech();
    std::thread::scope(|scope| {
        let download = scope.spawn(|| world.client.request("download_model", download_request(&model)));
        world.services.wait_for_questions(1);
        // The question is open. The same connection still answers.
        let started = Instant::now();
        let hello = world.client.request("hello", fields(json!({"protocol": 1})));
        assert!(hello.is_ok() && started.elapsed() < Duration::from_secs(2));
        assert!(!list(&world.client, json!({"role": "vad"})).is_empty());
        assert!(!download.is_finished(), "the download still waits");
        world.services.answer(false);
        assert_eq!(refused(download.join().unwrap()).code, Some(ErrorCode::Declined));
    });
}

/// The extension goes away while the question is open. The host withdraws
/// the question (the app closes its window, as a Cancel) and starts nothing
/// for a program that is not there.
#[cfg(unix)]
#[test]
fn a_closed_connection_closes_the_question_and_starts_no_download() {
    use std::io::Write;
    let world = world(&["models.download"]);
    let model = speech();
    let mut stream = std::os::unix::net::UnixStream::connect(&world._setup.socket).unwrap();
    let request = format!("{{\"v\":1,\"type\":\"download_model\",\"id\":\"d1\",\"model\":\"{model}\",\"reason\":\"x\"}}\n");
    stream.write_all(request.as_bytes()).unwrap();
    world.services.wait_for_questions(1);
    assert_eq!(world.services.withdrawn.load(std::sync::atomic::Ordering::SeqCst), 0);
    drop(stream);
    eventually("the host withdraws the question", || world.services.withdrawn.load(std::sync::atomic::Ordering::SeqCst) == 1);
    eventually("the host sees the closed connection", || world._host.connected_extensions().len() == 1);
    std::thread::sleep(Duration::from_millis(300));
    assert!(world.engine.downloads.lock().is_empty());
}

#[test]
fn closing_the_host_closes_an_open_question() {
    let world = world(&["models.download"]);
    let model = speech();
    std::thread::scope(|scope| {
        let download = scope.spawn(|| world.client.request("download_model", download_request(&model)));
        world.services.wait_for_questions(1);
        world._host.close();
        eventually("the host withdraws the question", || world.services.withdrawn.load(std::sync::atomic::Ordering::SeqCst) == 1);
        assert!(download.join().unwrap().is_err());
    });
    assert!(world.engine.downloads.lock().is_empty());
}
