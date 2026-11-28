//! A host older than the list of host features sends `features: []`. The SDK then sends each
//! request and handles the `error` reply (scenario H07).
#![cfg(unix)]

use sayso_sdk::protocol::{self, Capability, ErrorCode, Source};
use sayso_sdk::{App, Mode, Options, Speech};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::mpsc::channel;

mod common;

/// A host with the features `features`: it answers `hello`, then gives each request the
/// `unsupported` error, and tells the test what it got.
fn serve(listener: UnixListener, features: Value, seen: std::sync::mpsc::Sender<Value>) {
    std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            let request: Value = serde_json::from_str(&line).unwrap();
            line.clear();
            let reply = if request["type"] == "hello" {
                json!({"v": 1, "type": "ok", "id": request["id"], "host_version": "0.17.0", "protocol": 1, "features": features, "capabilities": ["session.live", "audio.mic"]})
            } else {
                let reply = json!({"v": 1, "type": "error", "id": request["id"], "code": "unsupported", "message": "the host said so"});
                seen.send(request).unwrap();
                reply
            };
            let _ = writeln!(writer, "{reply}");
        }
    });
}

fn session_with_microphone() -> protocol::SessionStart {
    protocol::SessionStart {
        sources: vec![Source::Mic],
        silence_limit_ms: Some(1000),
        record_to: Some("/tmp/rec".into()),
        ..Default::default()
    }
}

fn speech(socket: &std::path::Path) -> Speech {
    Speech::connect(
        Options::new(Mode::Sayso { start: false }, App::new("Old"))
            .capabilities([Capability::SessionLive, Capability::AudioMic])
            .socket(socket),
    )
    .unwrap()
}

#[test]
fn a_host_with_no_features_gets_the_request_and_answers_for_itself() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("host.sock");
    let (tx, seen) = channel();
    serve(UnixListener::bind(&socket).unwrap(), json!([]), tx);
    let speech = speech(&socket);
    let error = speech.start_session(session_with_microphone()).unwrap_err();
    assert_eq!(error.code, Some(ErrorCode::Unsupported));
    assert_eq!(
        error.message, "the host said so",
        "the host answered, not the SDK"
    );
    let request = seen
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert_eq!(request["type"], "session_start");
    assert_eq!(request["sources"], json!(["mic"]));
    assert!(
        speech
            .enhance(protocol::Enhance {
                text: Some("a".into()),
                ..Default::default()
            })
            .is_err()
    );
    assert_eq!(
        seen.recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()["type"],
        "enhance"
    );
    assert!(speech.subscribe(&["dictation_done"]).is_err());
    assert_eq!(
        seen.recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()["type"],
        "subscribe"
    );
    common::passed("H07");
}

#[test]
fn a_host_with_features_that_lack_one_is_not_asked() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("host.sock");
    let (tx, seen) = channel();
    serve(
        UnixListener::bind(&socket).unwrap(),
        json!(["dictation_done"]),
        tx,
    );
    let speech = speech(&socket);
    for request in [
        session_with_microphone(),
        protocol::SessionStart {
            sources: vec![Source::Client],
            silence_limit_ms: Some(1000),
            ..Default::default()
        },
        protocol::SessionStart {
            sources: vec![Source::Client],
            record_to: Some("/tmp/x".into()),
            ..Default::default()
        },
    ] {
        let error = speech.start_session(request).unwrap_err();
        assert_eq!(error.code, Some(ErrorCode::Unsupported));
        assert_ne!(
            error.message, "the host said so",
            "the SDK refused before the request went out"
        );
    }
    assert!(
        speech
            .enhance(protocol::Enhance {
                text: Some("a".into()),
                ..Default::default()
            })
            .unwrap_err()
            .message
            .contains("enhance")
    );
    assert!(
        seen.recv_timeout(std::time::Duration::from_millis(300))
            .is_err(),
        "no request reached the host"
    );
}
