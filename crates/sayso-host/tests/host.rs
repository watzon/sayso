//! The host and the SDK over a real socket (a named pipe on Windows). The
//! identity check is the real one: the record of `state.toml` has the folder
//! of this test program, so the test process is the extension.

use sayso_extension::{Client, Error, Event, Fields};
use sayso_host::{Host, HostConfig, HostServices, Identify, Peer};
use sayso_protocol::{Capability, ErrorCode};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

struct NoServices;
impl HostServices for NoServices {}

const ID: &str = "dev.example.test";

struct Setup {
    dir: tempfile::TempDir,
    socket: PathBuf,
    state: PathBuf,
}

fn setup() -> Setup {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = tempfile::tempdir().unwrap();
    let socket = if cfg!(windows) {
        PathBuf::from(format!(r"\\.\pipe\sayso-host-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)))
    } else {
        // The host makes this folder with mode 0700.
        dir.path().join("sayso").join("host.sock")
    };
    let state = dir.path().join("state.toml");
    Setup { dir, socket, state }
}

/// The folder of this test program.
fn here() -> PathBuf {
    std::env::current_exe().unwrap().parent().unwrap().to_path_buf()
}

fn write_state(path: &Path, folder: &Path, granted: &[&str], team_id: Option<&str>) {
    let granted: Vec<String> = granted.iter().map(|g| format!("\"{g}\"")).collect();
    let team = team_id.map(|t| format!("team_id = \"{t}\"\n")).unwrap_or_default();
    // A literal string: Windows paths have backslashes.
    let text = format!("[[extensions]]\nid = \"{ID}\"\nversion = \"0.1.0\"\npath = '{}'\nsideloaded = true\ngranted = [{}]\n{team}", folder.display(), granted.join(", "));
    std::fs::write(path, text).unwrap();
}

fn start(setup: &Setup) -> Host {
    Host::start(HostConfig::new(setup.socket.clone(), setup.state.clone()), Arc::new(NoServices)).unwrap()
}

fn fields(value: serde_json::Value) -> Fields {
    value.as_object().unwrap().clone()
}

/// The error of a request that the host refused.
fn refused(result: Result<sayso_extension::Reply, Error>) -> sayso_protocol::ErrorReply {
    match result {
        Err(Error::Host(error)) => error,
        other => panic!("expected an error reply, got {other:?}"),
    }
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let start = Instant::now();
    while !check() {
        assert!(start.elapsed() < Duration::from_secs(5), "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn an_extension_says_hello_and_gets_its_capabilities() {
    let setup = setup();
    write_state(&setup.state, &here(), &["session.live", "audio.mic"], None);
    let host = start(&setup);
    let client = Client::connect_to(&setup.socket).unwrap();
    let hello = client.hello();
    assert_eq!(hello.extension_id, ID);
    assert_eq!(hello.protocol, 1);
    assert_eq!(hello.host_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(hello.features, Vec::<String>::new());
    assert_eq!(hello.capabilities, vec![Capability::ModelsRead, Capability::SessionLive, Capability::AudioMic], "models.read is always granted");
    assert_eq!(host.connected_extensions(), vec![ID.to_string()]);

    // A request without its capability.
    let error = refused(client.request("transcribe_file", fields(json!({"path": "/tmp/talk.wav"}))));
    assert_eq!(error.code, Some(ErrorCode::Capability));
    assert_eq!(error.capability, Some(Capability::TranscribeFile));
    let error = refused(client.request("session_start", fields(json!({"sources": ["mic", "system"]}))));
    assert_eq!(error.capability, Some(Capability::AudioSystem));

    // Granted, but later sub-steps build it.
    for (kind, body) in [("list_models", json!({})), ("session_start", json!({"sources": ["mic"]})), ("session_stop", json!({"session": 1}))] {
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
    eventually("the host forgets the connection after bye", || host.connected_extensions().is_empty());
}

#[test]
fn a_change_of_the_grants_counts_at_the_next_request() {
    let setup = setup();
    write_state(&setup.state, &here(), &[], None);
    let _host = start(&setup);
    let client = Client::connect_to(&setup.socket).unwrap();
    let ask = || client.request("cancel_job", fields(json!({"job": 1})));
    assert_eq!(refused(ask()).code, Some(ErrorCode::Capability));
    write_state(&setup.state, &here(), &["transcribe.file"], None);
    assert_eq!(refused(ask()).code, Some(ErrorCode::Unsupported), "granted now");
    write_state(&setup.state, &here(), &[], None);
    assert_eq!(refused(ask()).code, Some(ErrorCode::Capability), "taken back");
    // Removed: the program is no longer an extension, and the host closes.
    std::fs::write(&setup.state, "").unwrap();
    assert_eq!(refused(ask()).code, Some(ErrorCode::NotAnExtension));
    assert!(matches!(ask(), Err(Error::Gone)));
}

#[test]
fn close_tells_each_extension_that_the_host_is_gone() {
    let setup = setup();
    write_state(&setup.state, &here(), &[], None);
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

    // Sayso starts again, and the extension connects again.
    let host = start(&setup);
    assert_eq!(client.reconnect().unwrap().extension_id, ID);
    assert_eq!(host.connected_extensions(), vec![ID.to_string()]);
    drop(host);
    assert_eq!(events.recv_timeout(Duration::from_secs(5)), Ok(Event::HostGone), "dropping the host closes it");
}

#[test]
fn a_program_outside_the_folders_of_the_records_is_not_an_extension() {
    let setup = setup();
    let elsewhere = setup.dir.path().join("dev.example.test/0.1.0");
    std::fs::create_dir_all(&elsewhere).unwrap();
    write_state(&setup.state, &elsewhere, &["session.live"], None);
    let host = start(&setup);
    let error = match Client::connect_to(&setup.socket) {
        Err(Error::Host(error)) => error,
        other => panic!("expected not_an_extension, got {:?}", other.map(|c| c.hello().clone())),
    };
    assert_eq!(error.code, Some(ErrorCode::NotAnExtension));
    // No state.toml: no extensions at all.
    std::fs::remove_file(&setup.state).unwrap();
    assert!(matches!(Client::connect_to(&setup.socket), Err(Error::Host(e)) if e.code == Some(ErrorCode::NotAnExtension)));
    // A broken state.toml: the same.
    std::fs::write(&setup.state, "[[extensions]\n").unwrap();
    assert!(matches!(Client::connect_to(&setup.socket), Err(Error::Host(e)) if e.code == Some(ErrorCode::NotAnExtension)));
    eventually("the host closes the connections", || host.connected_extensions().is_empty());
}

/// The test program has no Developer ID signature, so a record with a team
/// does not match it, also in the right folder.
#[cfg(target_os = "macos")]
#[test]
fn a_record_with_a_team_needs_the_code_signature_of_that_team() {
    let setup = setup();
    write_state(&setup.state, &here(), &[], Some("ABCDE12345"));
    let _host = start(&setup);
    assert!(matches!(Client::connect_to(&setup.socket), Err(Error::Host(e)) if e.code == Some(ErrorCode::NotAnExtension)));
}

/// An identity that the test chooses.
struct FakeIdentity {
    executable: PathBuf,
    team_id: &'static str,
}

impl Identify for FakeIdentity {
    fn executable(&self, _peer: &Peer) -> std::io::Result<PathBuf> {
        Ok(self.executable.clone())
    }
    fn signed_by(&self, _peer: &Peer, team_id: &str, _bundle_id: Option<&str>) -> bool {
        team_id == self.team_id
    }
}

#[test]
fn the_identity_check_can_be_replaced() {
    let setup = setup();
    let folder = setup.dir.path().join("installed");
    std::fs::create_dir_all(folder.join("Test.app/Contents/MacOS")).unwrap();
    let executable = folder.join("Test.app/Contents/MacOS/Test");
    std::fs::write(&executable, "").unwrap();
    write_state(&setup.state, &folder, &[], Some("ABCDE12345"));
    let mut config = HostConfig::new(setup.socket.clone(), setup.state.clone());
    config.identify = Arc::new(FakeIdentity { executable, team_id: "ABCDE12345" });
    let _host = Host::start(config, Arc::new(NoServices)).unwrap();
    assert_eq!(Client::connect_to(&setup.socket).unwrap().hello().extension_id, ID);
}

#[test]
fn a_second_host_on_the_same_socket_fails() {
    let setup = setup();
    let _host = start(&setup);
    let error = Host::start(HostConfig::new(setup.socket.clone(), setup.state.clone()), Arc::new(NoServices)).err().unwrap();
    assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
}

/// Lines that are not requests, sent without the SDK.
#[cfg(unix)]
#[test]
fn a_line_that_is_not_a_request_gets_an_error_without_id() {
    use std::io::{BufRead, BufReader, Write};
    let setup = setup();
    write_state(&setup.state, &here(), &[], None);
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
    assert_eq!((reply["type"].clone(), reply["id"].clone(), reply["extension_id"].clone()), (json!("ok"), json!("h1"), json!(ID)), "an empty line gets no reply");
}
