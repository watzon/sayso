//! The end rules of `Speech` against a fake host on a real socket (`docs/sdk.md`, "Mode changes
//! in `Mode::Auto`"): one end for each piece of work, also when a reply and the end of the
//! connection come at the same moment, generations, no request twice, and the retry of `hello`.
//!
//! The tests with the real host and the engines are in `sayso-host/tests/sdk_modes.rs`, because
//! this crate cannot depend on GPL crates. The engine here does not exist, so a request that
//! runs in local mode gets `unsupported`. That proves where the request went.
#![cfg(unix)]

use crossbeam_channel::{Receiver, unbounded};
use sayso_sdk::protocol::{self, ErrorCode, TranscribeFile};
use sayso_sdk::{ActiveMode, App, Engine, Event, Mode, Options, Speech, Timing};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const LONG: Duration = Duration::from_secs(60);

struct Peer {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Peer {
    fn read(&mut self) -> Option<Value> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(n) if n > 0 => serde_json::from_str(&line).ok(),
            _ => None,
        }
    }

    /// Send lines in one write, so they reach the SDK together.
    fn send(&mut self, lines: &[Value]) {
        let mut text = String::new();
        for line in lines {
            let mut line = line.clone();
            line["v"] = json!(protocol::PROTOCOL);
            text.push_str(&line.to_string());
            text.push('\n');
        }
        let _ = self.writer.write_all(text.as_bytes());
    }

    /// Read a request. On `hello`, answer. Gives the first other request.
    fn next_request(&mut self) -> Option<Value> {
        loop {
            let line = self.read()?;
            if line["type"] != "hello" {
                return Some(line);
            }
            self.send(&[json!({"type": "ok", "id": line["id"], "host_version": "fake", "protocol": protocol::PROTOCOL})]);
        }
    }

    fn close(&self) {
        let _ = self.writer.shutdown(std::net::Shutdown::Both);
    }
}

/// A host that runs `script` for each connection, one after the other.
fn host(socket: &Path, script: impl Fn(usize, Peer) + Send + Sync + 'static) -> Arc<AtomicUsize> {
    let listener = UnixListener::bind(socket).unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let counted = connections.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let number = counted.fetch_add(1, Ordering::SeqCst);
            let writer = stream.try_clone().unwrap();
            script(number, Peer { reader: BufReader::new(stream), writer });
        }
    });
    connections
}

struct Place {
    _dir: tempfile::TempDir,
    socket: PathBuf,
}

fn place() -> Place {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("host.sock");
    Place { _dir: dir, socket }
}

fn options(mode: Mode, place: &Place) -> Options {
    let folder = place.socket.with_file_name("models");
    Options::new(mode, App::new("Test")).socket(&place.socket).engine(Engine::from_program(place.socket.with_file_name("no-engine")).store(&folder).models_dir(&folder)).timing(Timing { watch: Duration::from_millis(5), ..Timing::default() })
}

fn sayso(place: &Place) -> Speech {
    Speech::connect(options(Mode::Sayso { start: false }, place)).unwrap()
}

fn job_request() -> TranscribeFile {
    TranscribeFile { path: "/a.wav".into(), ..Default::default() }
}

/// Events until `done`, which fails the test after [`LONG`].
fn wait_for(events: &Receiver<Event>, done: impl Fn(&Event) -> bool) -> Vec<Event> {
    let mut seen = Vec::new();
    loop {
        let event = events.recv_timeout(LONG).unwrap_or_else(|_| panic!("the event did not come; got {seen:?}"));
        let stop = done(&event);
        seen.push(event);
        if stop {
            return seen;
        }
    }
}

fn ends_of_job(events: &[Event]) -> Vec<&protocol::Event> {
    events.iter().filter_map(|event| match event {
        Event::Message(message @ (protocol::Event::JobDone { .. } | protocol::Event::JobFailed { .. })) => Some(message),
        _ => None,
    }).collect()
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let start = Instant::now();
    while !check() {
        assert!(start.elapsed() < LONG, "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Runs each round on a new socket and a new `Speech`, so it can catch a race.
const ROUNDS: usize = 150;

#[test]
fn a_job_done_and_a_lost_connection_at_one_moment_give_one_end() {
    for _ in 0..ROUNDS {
        let place = place();
        host(&place.socket, |_, mut peer| {
            let request = peer.next_request().unwrap();
            peer.send(&[json!({"type": "ok", "id": request["id"], "job": 7}), json!({"type": "job_done", "job": 7, "text": "done"})]);
            peer.close();
        });
        let speech = sayso(&place);
        let events = speech.events();
        let job = speech.transcribe_file(job_request()).unwrap();
        let seen = wait_for(&events, |event| matches!(event, Event::Disconnected { .. }));
        assert_eq!(ends_of_job(&seen), vec![&protocol::Event::JobDone { job, text: "done".into() }], "{seen:?}");
    }
}

#[test]
fn a_reply_to_a_stop_and_a_lost_connection_at_one_moment_give_one_end() {
    for _ in 0..ROUNDS {
        let place = place();
        host(&place.socket, |_, mut peer| {
            let start = peer.next_request().unwrap();
            peer.send(&[json!({"type": "ok", "id": start["id"], "session": 3})]);
            let stop = peer.next_request().unwrap();
            peer.send(&[json!({"type": "ok", "id": stop["id"], "text": "words"}), json!({"type": "session_ended", "session": 3, "text": "words"})]);
            peer.close();
        });
        let speech = sayso(&place);
        let events = speech.events();
        let session = speech.start_session(Default::default()).unwrap().session;
        assert_eq!(speech.stop_session(session).unwrap(), "words");
        let seen = wait_for(&events, |event| matches!(event, Event::Disconnected { .. }));
        assert!(!seen.iter().any(|event| matches!(event, Event::Message(protocol::Event::SessionEnded { .. }))), "the stop was the end: {seen:?}");
    }
}

#[test]
fn a_session_end_that_passes_the_reply_of_the_stop_gives_the_stop_its_text() {
    let place = place();
    host(&place.socket, |_, mut peer| {
        let start = peer.next_request().unwrap();
        peer.send(&[json!({"type": "ok", "id": start["id"], "session": 3})]);
        let stop = peer.next_request().unwrap();
        peer.send(&[json!({"type": "session_ended", "session": 3, "text": "words"}), json!({"type": "ok", "id": stop["id"], "text": "late"})]);
        while peer.read().is_some() {}
    });
    let speech = sayso(&place);
    let session = speech.start_session(Default::default()).unwrap().session;
    assert_eq!(speech.stop_session(session).unwrap(), "words");
    assert!(speech.events().try_iter().next().is_none(), "the stop was the only end");
}

#[test]
fn source_events_have_the_ids_of_the_sdk_and_stop_at_the_end_of_the_session() {
    let place = place();
    host(&place.socket, move |_, mut peer| {
        let first = peer.next_request().unwrap();
        peer.send(&[json!({"type": "ok", "id": first["id"], "session": 80})]);
        let start = peer.next_request().unwrap();
        peer.send(&[json!({"type": "ok", "id": start["id"], "session": 90, "excludes": ["sayso", "app"]}), json!({"type": "source_gap", "session": 90, "source": "system", "ms": 240}), json!({"type": "source_ended", "session": 90, "source": "system", "reason": "permission"}), json!({"type": "source_gap", "session": 91, "source": "system", "ms": 1})]);
        let stop = peer.next_request().unwrap();
        peer.send(&[json!({"type": "ok", "id": stop["id"], "text": "words"}), json!({"type": "source_gap", "session": 90, "source": "mic", "ms": 1})]);
        while peer.read().is_some() {}
    });
    let speech = sayso(&place);
    let events = speech.events();
    speech.start_session(Default::default()).unwrap();
    let started = speech.start_session(Default::default()).unwrap();
    assert_eq!(started.excludes, ["sayso", "app"]);
    assert_ne!(started.session, 90, "the id comes from the SDK");
    let seen = wait_for(&events, |event| matches!(event, Event::Message(protocol::Event::SourceEnded { .. })));
    assert_eq!(seen, [Event::Message(protocol::Event::SourceGap { session: started.session, source: protocol::Source::System, ms: 240 }), Event::Message(protocol::Event::SourceEnded { session: started.session, source: protocol::Source::System, reason: protocol::SourceEndReason::Permission })]);
    assert_eq!(speech.stop_session(started.session).unwrap(), "words");
    // This can only pass falsely on a slow machine, never fail falsely.
    std::thread::sleep(Duration::from_millis(50));
    assert!(events.try_iter().next().is_none(), "nothing after the end, and nothing for another session");
}

#[test]
fn a_refused_capture_gives_the_code_permission_and_leaves_no_open_work() {
    let place = place();
    host(&place.socket, |_, mut peer| {
        let start = peer.next_request().unwrap();
        peer.send(&[json!({"type": "error", "id": start["id"], "code": "permission", "message": "the source system did not start"})]);
        peer.close();
    });
    let speech = sayso(&place);
    let events = speech.events();
    let error = speech.start_session(Default::default()).unwrap_err();
    assert_eq!(error.code, Some(ErrorCode::Permission));
    let seen = wait_for(&events, |event| matches!(event, Event::Disconnected { .. }));
    assert!(!seen.iter().any(|event| matches!(event, Event::Message(_))), "no session was open: {seen:?}");
}

#[test]
fn a_lost_connection_ends_each_piece_of_work_once_and_sends_nothing_again() {
    let place = place();
    let requests = Arc::new(AtomicUsize::new(0));
    let counted = requests.clone();
    host(&place.socket, move |_, mut peer| {
        let job = peer.next_request().unwrap();
        peer.send(&[json!({"type": "ok", "id": job["id"], "job": 1})]);
        let session = peer.next_request().unwrap();
        peer.send(&[json!({"type": "ok", "id": session["id"], "session": 1})]);
        let waiting = peer.next_request().unwrap();
        assert_eq!(waiting["type"], "list_models");
        counted.fetch_add(1, Ordering::SeqCst);
        peer.close();
        while peer.read().is_some() {
            counted.fetch_add(1, Ordering::SeqCst);
        }
    });
    let speech = sayso(&place);
    let events = speech.events();
    let job = speech.transcribe_file(job_request()).unwrap();
    let session = speech.start_session(Default::default()).unwrap().session;
    let error = speech.list_models(None).unwrap_err();
    assert_eq!(error.code, Some(ErrorCode::ConnectionLost));
    let seen = wait_for(&events, |event| matches!(event, Event::Disconnected { code: ErrorCode::ConnectionLost }));
    assert_eq!(ends_of_job(&seen).len(), 1);
    assert!(seen.contains(&Event::Message(protocol::Event::JobFailed { job, message: "The connection to Sayso was lost during this job.".into(), code: Some(ErrorCode::ConnectionLost) })), "{seen:?}");
    assert!(seen.contains(&Event::Message(protocol::Event::SessionEnded { session, text: String::new(), code: Some(ErrorCode::ConnectionLost) })), "{seen:?}");
    assert_eq!(requests.load(Ordering::SeqCst), 1, "no request goes out twice");
    assert_eq!(speech.list_models(None).unwrap_err().code, Some(ErrorCode::HostNotRunning), "Mode::Sayso does not change mode");
    assert_eq!(speech.stop_session(session).unwrap_err().code, Some(ErrorCode::HostNotRunning), "the session ended, and no request runs until reconnect");
    assert!(speech.events().try_iter().next().is_none(), "no second end");
}

#[test]
fn a_stop_without_a_reply_gets_the_loss_as_its_end() {
    let place = place();
    host(&place.socket, |_, mut peer| {
        let start = peer.next_request().unwrap();
        peer.send(&[json!({"type": "ok", "id": start["id"], "session": 1})]);
        let _ = peer.next_request();
        peer.close();
    });
    let speech = sayso(&place);
    let events = speech.events();
    let session = speech.start_session(Default::default()).unwrap().session;
    assert_eq!(speech.stop_session(session).unwrap_err().code, Some(ErrorCode::ConnectionLost));
    let seen = wait_for(&events, |event| matches!(event, Event::Disconnected { .. }));
    assert!(!seen.iter().any(|event| matches!(event, Event::Message(_))), "the reply was the end: {seen:?}");
}

#[test]
fn host_closing_ends_the_work_and_changes_to_local_mode_in_one_step() {
    let place = place();
    host(&place.socket, |number, mut peer| {
        if number > 0 {
            return;
        }
        let job = peer.next_request().unwrap();
        peer.send(&[json!({"type": "ok", "id": job["id"], "job": 1}), json!({"type": "host_closing"})]);
        peer.close();
    });
    let speech = Speech::connect(options(Mode::Auto, &place)).unwrap();
    assert_eq!(speech.mode(), ActiveMode::Sayso);
    let events = speech.events();
    let job = speech.transcribe_file(job_request()).unwrap();
    let seen = wait_for(&events, |event| matches!(event, Event::ModeChanged { .. }));
    assert_eq!(ends_of_job(&seen).len(), 1, "{seen:?}");
    assert!(matches!(ends_of_job(&seen)[0], protocol::Event::JobFailed { job: failed, code: Some(ErrorCode::HostClosing), .. } if *failed == job));
    assert!(matches!(seen.last(), Some(Event::ModeChanged { mode: ActiveMode::Local, .. })), "the end of the work comes first");
    // The app saw the change, so this request runs in local mode: the engine does not exist.
    assert_eq!(speech.mode(), ActiveMode::Local);
    assert_eq!(speech.list_models(None).unwrap_err().code, Some(ErrorCode::Unsupported));
}

#[test]
fn the_first_request_after_a_lost_connection_runs_in_local_mode() {
    for _ in 0..ROUNDS {
        let place = place();
        host(&place.socket, |number, mut peer| {
            if number == 0 {
                let _ = peer.next_request();
                peer.close();
            }
        });
        let speech = Speech::connect(options(Mode::Auto, &place)).unwrap();
        let events = speech.events();
        assert_eq!(speech.list_models(None).unwrap_err().code, Some(ErrorCode::ConnectionLost));
        wait_for(&events, |event| matches!(event, Event::ModeChanged { mode: ActiveMode::Local, .. }));
        assert_eq!(speech.list_models(None).unwrap_err().code, Some(ErrorCode::Unsupported));
    }
}

#[test]
fn ids_are_not_used_again_and_events_of_ended_work_are_dropped() {
    let place = place();
    let (go_on, wait) = unbounded::<()>();
    host(&place.socket, move |number, mut peer| {
        let job = peer.next_request().unwrap();
        peer.send(&[json!({"type": "ok", "id": job["id"], "job": 7})]);
        if number == 0 {
            peer.close();
            return;
        }
        let _ = wait.recv();
        // The number of the first connection again, and then a number of no job.
        peer.send(&[json!({"type": "job_progress", "job": 99, "fraction": 0.5}), json!({"type": "job_progress", "job": 7, "fraction": 0.5}), json!({"type": "job_done", "job": 7, "text": "second"}), json!({"type": "job_done", "job": 7, "text": "twice"})]);
        while peer.read().is_some() {}
    });
    let speech = sayso(&place);
    let events = speech.events();
    let first = speech.transcribe_file(job_request()).unwrap();
    wait_for(&events, |event| matches!(event, Event::Disconnected { .. }));
    speech.reconnect().unwrap();
    let second = speech.transcribe_file(job_request()).unwrap();
    assert_ne!(first, second, "an id does not come back");
    go_on.send(()).unwrap();
    let seen = wait_for(&events, |event| matches!(event, Event::Message(protocol::Event::JobDone { .. })));
    assert_eq!(&seen[seen.len() - 2..], [Event::Message(protocol::Event::JobProgress { job: second, fraction: 0.5 }), Event::Message(protocol::Event::JobDone { job: second, text: "second".into() })]);
    assert!(!seen.iter().any(|event| matches!(event, Event::Message(protocol::Event::JobProgress { job, .. }) if *job != second)), "{seen:?}");
    speech.cancel_job(first).unwrap();
    speech.cancel_job(second).unwrap();
    assert_eq!(speech.cancel_job(second + 1).unwrap_err().code, Some(ErrorCode::BadRequest), "not a job of this app");
    assert!(speech.events().try_iter().next().is_none());
}

/// A host that refuses `hello` while `refuse` says so.
fn refusing_host(socket: &Path, hellos: Arc<AtomicUsize>, refuse: bool) {
    host(socket, move |_, mut peer| {
        while let Some(line) = peer.read() {
            if line["type"] != "hello" {
                continue;
            }
            hellos.fetch_add(1, Ordering::SeqCst);
            if refuse {
                peer.send(&[json!({"type": "error", "id": line["id"], "code": "not_approved", "message": "no"})]);
            } else {
                peer.send(&[json!({"type": "ok", "id": line["id"], "host_version": "fake", "protocol": protocol::PROTOCOL})]);
            }
        }
    });
}

#[test]
fn a_refused_app_in_auto_stays_local_until_the_retry_time() {
    let place = place();
    let hellos = Arc::new(AtomicUsize::new(0));
    refusing_host(&place.socket, hellos.clone(), true);
    let timing = Timing { watch: Duration::from_millis(5), retry: Duration::from_millis(50), ..Timing::default() };
    let speech = Speech::connect(options(Mode::Auto, &place).timing(timing)).unwrap();
    assert_eq!(speech.mode(), ActiveMode::Local);
    eventually("it tries again after the retry time", || hellos.load(Ordering::SeqCst) >= 3);
    assert_eq!(speech.mode(), ActiveMode::Local);
    assert!(speech.events().try_iter().next().is_none(), "a failed hello changes nothing for the app");
}

#[test]
fn a_refused_app_does_not_try_again_on_the_same_socket_before_the_retry_time() {
    let place = place();
    let hellos = Arc::new(AtomicUsize::new(0));
    refusing_host(&place.socket, hellos.clone(), true);
    let speech = Speech::connect(options(Mode::Auto, &place)).unwrap();
    // This can only pass falsely on a slow machine, never fail falsely.
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(hellos.load(Ordering::SeqCst), 1);
    drop(speech);
}

#[test]
fn a_new_socket_gets_a_new_try_at_once() {
    let place = place();
    let hellos = Arc::new(AtomicUsize::new(0));
    refusing_host(&place.socket, hellos.clone(), true);
    let speech = Speech::connect(options(Mode::Auto, &place)).unwrap();
    let events = speech.events();
    assert_eq!(speech.mode(), ActiveMode::Local);
    // Sayso restarts with the other answer. The retry time is 30 s here.
    std::fs::remove_file(&place.socket).unwrap();
    refusing_host(&place.socket, Arc::new(AtomicUsize::new(0)), false);
    wait_for(&events, |event| matches!(event, Event::ModeChanged { mode: ActiveMode::Sayso, .. }));
    assert_eq!(speech.mode(), ActiveMode::Sayso);
}

#[test]
fn sayso_mode_fails_when_sayso_does_not_run_and_can_start_it() {
    let place = place();
    let error = Speech::connect(options(Mode::Sayso { start: false }, &place)).err().unwrap();
    assert_eq!(error.code, Some(ErrorCode::HostNotRunning));

    let starts = Arc::new(AtomicUsize::new(0));
    let mut starting = options(Mode::Sayso { start: true }, &place);
    let (socket, counted) = (place.socket.clone(), starts.clone());
    starting.start = Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        refusing_host(&socket, Arc::new(AtomicUsize::new(0)), false);
        Ok(())
    });
    let speech = Speech::connect(starting).unwrap();
    assert_eq!((speech.mode(), starts.load(Ordering::SeqCst)), (ActiveMode::Sayso, 1));
}

#[test]
fn an_app_that_the_user_refused_gets_not_approved_in_sayso_mode() {
    let place = place();
    refusing_host(&place.socket, Arc::new(AtomicUsize::new(0)), true);
    let error = Speech::connect(options(Mode::Sayso { start: false }, &place)).err().unwrap();
    assert_eq!(error.code, Some(ErrorCode::NotApproved));
}

#[test]
fn local_mode_never_talks_to_sayso() {
    let place = place();
    let hellos = Arc::new(AtomicUsize::new(0));
    refusing_host(&place.socket, hellos.clone(), false);
    let program = place.socket.with_file_name("engine");
    std::fs::write(&program, "").unwrap();
    let speech = Speech::connect(options(Mode::Local, &place).engine(Engine::from_program(&program))).unwrap();
    assert_eq!(speech.mode(), ActiveMode::Local);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(hellos.load(Ordering::SeqCst), 0);
    assert_eq!(Speech::connect(options(Mode::Local, &place)).err().unwrap().code, Some(ErrorCode::Unsupported), "the engine of the app is not there");
    assert_eq!(Speech::connect(Options::new(Mode::Local, App::new("Test")).socket(&place.socket)).err().unwrap().code, Some(ErrorCode::BadRequest), "the app names no engine");
}
