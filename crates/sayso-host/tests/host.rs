//! The host and the SDK over a real socket (a named pipe on Windows). The
//! identity check is the real one: the record of `apps.toml` has the path of
//! this test program, so the test process is the app.

use sayso_extension::{Client, Error, Event, Fields};
use sayso_host::{Host, HostConfig};
use sayso_protocol::{Capability, ErrorCode};
use serde_json::json;
use std::time::Duration;

mod common;
use common::*;

fn start(setup: &Setup) -> Host {
    start_with(setup, FakeServices::new(None))
}

#[test]
fn an_app_says_hello_and_gets_its_capabilities() {
    let setup = setup();
    write_apps(&setup.apps, &["session.live", "audio.mic"]);
    let host = start(&setup);
    let client = Client::connect_to(&setup.socket).unwrap();
    let hello = client.hello();
    assert_eq!(hello.protocol, 1);
    assert_eq!(hello.host_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(hello.features, Vec::<String>::new());
    assert_eq!(hello.capabilities, vec![Capability::ModelsRead, Capability::SessionLive, Capability::AudioMic], "models.read is always granted");
    assert_eq!(host.connected_apps(), vec![id()]);

    // A request without its capability.
    let error = refused(client.request("transcribe_file", fields(json!({"path": "/tmp/talk.wav"}))));
    assert_eq!(error.code, Some(ErrorCode::Capability));
    assert_eq!(error.capability, Some(Capability::TranscribeFile));
    let error = refused(client.request("session_start", fields(json!({"sources": ["mic", "system"]}))));
    assert_eq!(error.capability, Some(Capability::AudioSystem));

    // Granted, but later sub-steps build it.
    for (kind, body) in [("session_start", json!({"sources": ["mic"]})), ("session_stop", json!({"session": 1}))] {
        let error = refused(client.request(kind, fields(body)));
        assert_eq!(error.code, Some(ErrorCode::Unsupported), "{kind}");
    }

    // An unknown type, and a known type with bad fields.
    let error = refused(client.request("translate", Fields::new()));
    assert_eq!(error.code, None);
    assert!(error.message.contains("translate"), "{}", error.message);
    let error = refused(client.request("cancel_job", fields(json!({"job": "seven"}))));
    assert_eq!(error.code, Some(ErrorCode::BadRequest));

    client.close().unwrap();
    eventually("the host forgets the connection after bye", || host.connected_apps().is_empty());
}

#[test]
fn a_change_of_the_grants_counts_at_the_next_request() {
    let setup = setup();
    write_apps(&setup.apps, &[]);
    let _host = start(&setup);
    let client = Client::connect_to(&setup.socket).unwrap();
    let ask = || client.request("cancel_job", fields(json!({"job": 1})));
    assert_eq!(refused(ask()).code, Some(ErrorCode::Capability));
    write_apps(&setup.apps, &["transcribe.file"]);
    // Granted now: the host looks for the job, and this connection has none.
    assert_eq!(refused(ask()).code, Some(ErrorCode::BadRequest), "granted now");
    write_apps(&setup.apps, &[]);
    assert_eq!(refused(ask()).code, Some(ErrorCode::Capability), "taken back");
    // Removed: the app is no longer approved, and the host closes.
    std::fs::write(&setup.apps, "").unwrap();
    assert_eq!(refused(ask()).code, Some(ErrorCode::NotApproved));
    assert!(matches!(ask(), Err(Error::Gone)));
}

#[test]
fn close_tells_each_app_that_the_host_is_gone() {
    let setup = setup();
    write_apps(&setup.apps, &[]);
    let host = start(&setup);
    let mut client = Client::connect_to(&setup.socket).unwrap();
    let events = client.events();
    host.close();
    assert_eq!(events.recv_timeout(Duration::from_secs(5)), Ok(Event::HostGone));
    assert!(events.recv_timeout(Duration::from_millis(200)).is_err(), "HostGone comes once");
    assert!(matches!(client.request("list_models", Fields::new()), Err(Error::Gone)));
    #[cfg(unix)]
    assert!(!setup.socket.exists(), "close removes the socket file");
    host.close();

    // Sayso starts again, and the app connects again.
    let host = start(&setup);
    assert_eq!(client.reconnect().unwrap().capabilities, vec![Capability::ModelsRead]);
    assert_eq!(host.connected_apps(), vec![id()]);
    drop(host);
    assert_eq!(events.recv_timeout(Duration::from_secs(5)), Ok(Event::HostGone), "dropping the host closes it");
}

#[test]
fn a_second_host_on_the_same_socket_fails() {
    let setup = setup();
    let _host = start(&setup);
    let error = Host::start(HostConfig::new(setup.socket.clone(), setup.apps.clone()), FakeServices::new(None)).err().unwrap();
    assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
}

/// Lines that are not requests, sent without the SDK.
#[cfg(unix)]
#[test]
fn a_line_that_is_not_a_request_gets_an_error_without_id() {
    use std::io::{BufRead, BufReader, Write};
    let setup = setup();
    write_apps(&setup.apps, &[]);
    let _host = start(&setup);
    let mut stream = std::os::unix::net::UnixStream::connect(&setup.socket).unwrap();
    let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
    let mut next = || serde_json::from_str::<serde_json::Value>(&lines.next().unwrap().unwrap()).unwrap();
    stream.write_all(b"hello\n{\"v\":1,\"id\":\"x\"}\n\n{\"v\":1,\"type\":\"hello\",\"id\":\"h1\",\"protocol\":1}\n").unwrap();
    for _ in 0..2 {
        let reply = next();
        assert_eq!((reply["v"].clone(), reply["type"].clone(), reply["code"].clone()), (json!(1), json!("error"), json!("bad_request")));
        assert!(reply.get("id").is_none(), "{reply}");
    }
    let reply = next();
    assert_eq!((reply["type"].clone(), reply["id"].clone()), (json!("ok"), json!("h1")), "an empty line gets no reply");
}

#[test]
fn connect_or_start_starts_sayso_only_when_it_does_not_run() {
    let setup = setup();
    write_apps(&setup.apps, &[]);
    let starting = std::sync::Mutex::new(None);

    // Sayso does not run: the SDK starts it and waits for its socket.
    let client = Client::connect_or_start_with(&setup.socket, Duration::from_secs(10), || {
        let socket = setup.socket.clone();
        let state = setup.apps.clone();
        // Sayso takes a moment to listen.
        *starting.lock().unwrap() = Some(std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            Host::start(HostConfig::new(socket, state), FakeServices::new(None)).unwrap()
        }));
        Ok(())
    })
    .unwrap();
    let _host = starting.lock().unwrap().take().unwrap().join().unwrap();
    assert_eq!(client.hello().protocol, 1);

    // Sayso runs: no start.
    let again = Client::connect_or_start_with(&setup.socket, Duration::from_secs(10), || panic!("Sayso runs; nothing starts")).unwrap();
    assert_eq!(again.hello().protocol, 1);
}

#[test]
fn connect_or_start_reports_a_start_that_fails_or_takes_too_long() {
    let setup = setup();
    let failed = Client::connect_or_start_with(&setup.socket, Duration::from_secs(10), || Err(std::io::Error::other("not installed")));
    assert!(matches!(failed, Err(Error::Io(e)) if e.to_string().contains("not installed")));
    let started = std::time::Instant::now();
    let late = Client::connect_or_start_with(&setup.socket, Duration::from_millis(300), || Ok(()));
    assert!(matches!(late, Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::TimedOut));
    assert!(started.elapsed() < Duration::from_secs(5));
}
