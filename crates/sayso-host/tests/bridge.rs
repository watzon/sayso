//! A Sayso in a Flatpak serves apps through `sayso-bridge` (`docs/sdk.md`,
//! "Flatpak"). These tests run the code of the bridge in this process, so the
//! test program is the app. The tests fork, so they are in a test program of their
//! own, as in `approval_fork.rs`. The real sandboxes are in `docs/vm-testing.md`.

#![cfg(target_os = "linux")]

use sayso_bridge::identity::Who;
use sayso_bridge::wire::{self, Attested};
use sayso_host::{Host, Identity};
use sayso_protocol::{Capability, ErrorCode};
use sayso_sdk::client::{AppHello, Client, Error};
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

mod common;
use common::*;

const HELLO: &[u8] = b"{\"v\":1,\"type\":\"hello\",\"id\":\"h1\",\"protocol\":1}\n";

struct Bridged {
    host: Host,
    setup: Setup,
    /// Where the bridge listens.
    socket: PathBuf,
    services: Arc<FakeServices>,
    bridge: Option<std::thread::JoinHandle<std::io::Result<()>>>,
}

fn bridged(granted: Option<&[&str]>) -> Bridged {
    let setup = setup();
    if let Some(granted) = granted {
        write_apps(&setup.apps, granted);
    }
    let services = FakeServices::new(None);
    let host = start_with(&setup, services.clone());
    let socket = setup.dir.path().join("outside").join("host.sock");
    let (ours, theirs) = wire::pair(libc::SOCK_SEQPACKET).unwrap();
    host.attach_bridge(ours);
    let bridge = {
        let socket = socket.clone();
        std::thread::spawn(move || sayso_bridge::serve::run(theirs, &socket))
    };
    eventually("the bridge listens", || socket.exists());
    Bridged { host, setup, socket, services, bridge: Some(bridge) }
}

/// A host with a channel, and no bridge: the test writes what a bridge writes.
fn with_channel() -> (Host, Setup, Arc<FakeServices>, std::os::fd::OwnedFd) {
    let setup = setup();
    let services = FakeServices::new(None);
    let host = start_with(&setup, services.clone());
    let (ours, theirs) = wire::pair(libc::SOCK_SEQPACKET).unwrap();
    host.attach_bridge(ours);
    (host, setup, services, theirs)
}

fn send_connection(channel: &std::os::fd::OwnedFd, pid: i32, who: Result<Who, String>) -> UnixStream {
    let (ours, theirs) = wire::pair(libc::SOCK_STREAM).unwrap();
    wire::send(channel.as_fd(), &Attested { pid, who }.encode(), Some(theirs.as_fd())).unwrap();
    UnixStream::from(ours)
}

#[test]
fn an_app_is_asked_for_once_and_works_after_allow() {
    let world = bridged(None);
    let app = AppHello { name: None, capabilities: vec![Capability::TranscribeFile] };
    let waiting = {
        let (socket, app) = (world.socket.clone(), app.clone());
        std::thread::spawn(move || Client::connect_as(socket, &app))
    };
    world.services.wait_for_approvals(1);
    let question = world.services.approvals.lock()[0].clone();
    assert_eq!(question.identity, Identity::Path(this_program()), "the identity comes from the checks of the bridge");
    world.services.answer(true);
    let client = waiting.join().unwrap().unwrap();
    assert_eq!(client.hello().capabilities, vec![Capability::ModelsRead, Capability::TranscribeFile]);
    // The line reaches the host: without an engine it says so.
    let Err(Error::Host(error)) = client.request("list_models", fields(json!({}))) else { panic!("this host has no engine") };
    assert_eq!(error.code, Some(ErrorCode::Unsupported));
    let again = Client::connect_as(&world.socket, &app).unwrap();
    assert_eq!(again.hello().capabilities, client.hello().capabilities);
    assert_eq!(world.services.approvals.lock().len(), 1, "no second question");
}

#[test]
fn dont_allow_gives_not_approved() {
    let world = bridged(None);
    let waiting = {
        let socket = world.socket.clone();
        std::thread::spawn(move || Client::connect_as(socket, &AppHello { name: None, capabilities: vec![] }))
    };
    world.services.wait_for_approvals(1);
    world.services.answer(false);
    let Err(Error::Host(error)) = waiting.join().unwrap() else { panic!("the app has no access") };
    assert_eq!(error.code, Some(ErrorCode::NotApproved));
}

#[test]
fn a_line_from_another_process_closes_the_connection_and_never_reaches_the_host() {
    let world = bridged(Some(&[]));
    let mut stream = UnixStream::connect(&world.socket).unwrap();
    let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
    stream.write_all(HELLO).unwrap();
    assert!(lines.next().unwrap().unwrap().contains("\"ok\""));
    stream.write_all(b"{\"v\":1,\"type\":\"list_models\",\"id\":\"same\"}\n").unwrap();
    assert!(lines.next().unwrap().unwrap().contains("\"same\""), "the same process goes on");

    let request = b"{\"v\":1,\"type\":\"list_models\",\"id\":\"child\"}\n";
    // SAFETY: the child makes only calls that are safe after a fork of a process with threads.
    unsafe {
        let child = libc::fork();
        assert!(child >= 0);
        if child == 0 {
            libc::write(stream.as_raw_fd(), request.as_ptr().cast(), request.len());
            libc::_exit(0);
        }
        libc::waitpid(child, std::ptr::null_mut(), 0);
    }
    assert!(lines.next().is_none(), "no reply, and the bridge closes");
    eventually("the host forgets the connection", || world.host.connected_apps().is_empty());
}

#[test]
fn a_program_that_names_an_identity_in_a_line_does_not_get_it() {
    let world = bridged(Some(&[]));
    let mut stream = UnixStream::connect(&world.socket).unwrap();
    let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
    stream.write_all(b"{\"v\":1,\"type\":\"hello\",\"id\":\"h1\",\"protocol\":1,\"pid\":1,\"app\":{\"name\":\"dev.sayso.Sayso\"},\"identity\":\"flatpak:dev.sayso.Sayso\",\"flatpak\":\"dev.sayso.Sayso\"}\n").unwrap();
    assert!(lines.next().unwrap().unwrap().contains("\"ok\""));
    assert_eq!(world.host.connected_apps(), vec![Identity::Path(this_program()).id()]);
}

#[test]
fn when_the_host_closes_the_app_hears_it_and_the_socket_goes() {
    let mut world = bridged(Some(&[]));
    let mut stream = UnixStream::connect(&world.socket).unwrap();
    let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
    stream.write_all(HELLO).unwrap();
    assert!(lines.next().unwrap().unwrap().contains("\"ok\""));
    world.host.close();
    assert!(lines.next().unwrap().unwrap().contains("host_closing"));
    assert!(lines.next().is_none());
    world.bridge.take().unwrap().join().unwrap().unwrap();
    assert!(!world.socket.exists(), "no stale socket");
}

#[test]
fn a_file_of_the_app_opens_only_through_the_bridge() {
    let world = bridged(Some(&["transcribe.file", "session.live"]));
    let client = Client::connect_to(&world.socket).unwrap();
    // Without an engine the job stops after the checks: `unsupported` means
    // that the file opened, and `bad_request` that it did not.
    let file = world.setup.dir.path().join("a.wav");
    std::fs::write(&file, b"sound").unwrap();
    let code = |path: &Path| match client.request("transcribe_file", fields(json!({"path": path}))) {
        Err(Error::Host(error)) => error.code,
        other => panic!("{other:?}"),
    };
    assert_eq!(code(&file), Some(ErrorCode::Unsupported));
    assert_eq!(code(&world.setup.dir.path().join("missing.wav")), Some(ErrorCode::BadRequest));
    assert_eq!(code(Path::new("/sayso-bridge/file/0")), Some(ErrorCode::BadRequest), "a client cannot name a file of the bridge");
}

#[test]
fn record_to_is_refused_across_the_sandbox() {
    let world = bridged(Some(&["session.live"]));
    let client = Client::connect_to(&world.socket).unwrap();
    let Err(Error::Host(error)) = client.request("session_start", fields(json!({"sources": ["client"], "record_to": "/home/me/notes"}))) else { panic!("it must be refused") };
    assert_eq!(error.code, Some(ErrorCode::Unsupported));
}

#[test]
fn a_flatpak_app_gets_the_question_and_a_record_with_its_id() {
    let (_host, setup, services, channel) = with_channel();
    let mut stream = send_connection(&channel, 4242, Ok(Who::Flatpak("dev.example.Notes".into())));
    let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
    stream.write_all(HELLO).unwrap();
    services.wait_for_approvals(1);
    let question = services.approvals.lock()[0].clone();
    assert_eq!(question.identity, Identity::Flatpak { app_id: "dev.example.Notes".into() });
    assert_eq!(question.app_id, "flatpak:dev.example.Notes");
    assert_eq!(question.name, "Notes");
    assert!(!question.interpreter);
    services.answer(true);
    while !lines.next().unwrap().unwrap().contains("\"ok\"") {}
    let text = std::fs::read_to_string(&setup.apps).unwrap();
    assert!(text.contains("flatpak = \"dev.example.Notes\"") && !text.contains("path ="), "{text}");

    // Another connection of the same id, with no new question.
    let mut second = send_connection(&channel, 4243, Ok(Who::Flatpak("dev.example.Notes".into())));
    let mut second_lines = BufReader::new(second.try_clone().unwrap()).lines();
    second.write_all(HELLO).unwrap();
    assert!(second_lines.next().unwrap().unwrap().contains("\"ok\""));
    assert_eq!(services.approvals.lock().len(), 1);
}

#[test]
fn no_identity_from_the_bridge_means_no_access() {
    let (host, _setup, services, channel) = with_channel();
    let mut stream = send_connection(&channel, 0, Err("more than one process wrote to this connection".into()));
    let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
    stream.write_all(HELLO).unwrap();
    assert!(lines.next().unwrap().unwrap().contains("not_approved"));
    assert!(lines.next().is_none(), "the connection closes");
    assert!(services.approvals.lock().is_empty(), "no question");
    assert!(host.connected_apps().is_empty());
}

#[test]
fn a_broken_message_on_the_channel_gives_no_connection() {
    let (host, _setup, _services, channel) = with_channel();
    wire::send(channel.as_fd(), b"junk", None).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(host.connected_apps().is_empty());
    // The channel still works.
    let mut stream = send_connection(&channel, 1, Err("no identity".into()));
    let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
    stream.write_all(HELLO).unwrap();
    assert!(lines.next().unwrap().unwrap().contains("not_approved"));
}
