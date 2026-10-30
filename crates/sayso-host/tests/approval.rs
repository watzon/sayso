//! The approval question (`docs/sdk.md`, "The question"), over a real socket
//! with a fake answerer: what an app gets after each answer, the limits of the
//! question, and a program with a broken signature.

use sayso_extension::{AppHello, Client, Error};
use sayso_host::{Caller, Identity};
use sayso_protocol::{Capability, ErrorCode};
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::Duration;

mod common;
use common::*;

fn asking(capabilities: &[Capability]) -> AppHello {
    AppHello { name: None, capabilities: capabilities.to_vec() }
}

fn connect(setup: &Setup, app: &AppHello) -> JoinHandle<Result<Client, Error>> {
    let (socket, app) = (setup.socket.clone(), app.clone());
    std::thread::spawn(move || Client::connect_as(socket, &app))
}

fn code_of(result: Result<Client, Error>) -> Option<ErrorCode> {
    match result {
        Err(Error::Host(error)) => error.code,
        Err(other) => panic!("expected an error reply, got {other:?}"),
        Ok(client) => panic!("expected an error reply, got {:?}", client.hello()),
    }
}

fn signed(identifier: &str) -> Caller {
    Caller { identity: Identity::Signed { team_id: "ABCDE12345".into(), identifier: identifier.into() }, name: "Notes".into(), signer: Some("Example LLC".into()), interpreter: false }
}

fn path(name: &str) -> Caller {
    Caller::from_path(PathBuf::from("/opt").join(name))
}

#[test]
fn an_app_in_any_folder_is_asked_once_and_works_after_allow() {
    let setup = setup();
    let services = FakeServices::new(Some(FakeEngine::new()));
    let _host = start_with(&setup, services.clone());

    // No install step and no record: the test program runs from a target folder.
    let app = asking(&[Capability::TranscribeFile]);
    let waiting = connect(&setup, &app);
    services.wait_for_approvals(1);
    let question = services.approvals.lock()[0].clone();
    assert_eq!(question.app_id, id());
    assert_eq!(question.identity, Identity::Path(this_program()));
    assert_eq!(question.capabilities, vec![Capability::TranscribeFile]);
    assert!(!question.approved);
    assert!(!waiting.is_finished(), "the reply to hello waits for the answer");

    services.answer(true);
    let client = waiting.join().unwrap().unwrap();
    assert_eq!(client.hello().capabilities, vec![Capability::ModelsRead, Capability::TranscribeFile]);
    assert!(client.hello().denied.is_empty());
    client.request("list_models", fields(json!({}))).unwrap();

    // The next connection and the next start of Sayso ask nothing.
    let again = Client::connect_as(&setup.socket, &app).unwrap();
    assert_eq!(again.hello().capabilities, client.hello().capabilities);
    assert_eq!(services.approvals.lock().len(), 1);
    let text = std::fs::read_to_string(&setup.apps).unwrap();
    assert!(text.contains("allowed = true") && text.contains("transcribe.file") && text.contains("first_seen"), "{text}");
}

#[test]
fn dont_allow_gives_not_approved_and_asks_no_more() {
    let setup = setup();
    let services = FakeServices::new(None);
    let _host = start_with(&setup, services.clone());
    let waiting = connect(&setup, &asking(&[Capability::AudioMic]));
    services.wait_for_approvals(1);
    services.answer(false);
    assert_eq!(code_of(waiting.join().unwrap()), Some(ErrorCode::NotApproved));

    assert_eq!(code_of(Client::connect_as(&setup.socket, &asking(&[Capability::AiEnhance]))), Some(ErrorCode::NotApproved));
    assert_eq!(services.approvals.lock().len(), 1, "one answer for the identity");
    let text = std::fs::read_to_string(&setup.apps).unwrap();
    assert!(text.contains("allowed = false") && text.contains("denied = [\"audio.mic\"]"), "{text}");
}

#[test]
fn a_grant_that_is_taken_back_stops_the_next_request() {
    let setup = setup();
    let services = FakeServices::new(None);
    let host = start_with(&setup, services.clone());
    let waiting = connect(&setup, &asking(&[Capability::TranscribeFile]));
    services.wait_for_approvals(1);
    services.answer(true);
    let client = waiting.join().unwrap().unwrap();
    let ask = || client.request("cancel_job", fields(json!({"job": 1})));
    assert_eq!(refused(ask()).code, Some(ErrorCode::BadRequest), "granted: the host looks for the job");

    // The switch in Settings.
    host.apps().set_grant(&id(), &Capability::TranscribeFile, false).unwrap();
    let error = refused(ask());
    assert_eq!((error.code, error.capability), (Some(ErrorCode::Capability), Some(Capability::TranscribeFile)));

    // A later connection is told what the user took back.
    let second = Client::connect_as(&setup.socket, &asking(&[Capability::TranscribeFile])).unwrap();
    assert_eq!(second.hello().capabilities, vec![Capability::ModelsRead]);
    assert_eq!(second.hello().denied, vec![Capability::TranscribeFile]);
    assert_eq!(services.approvals.lock().len(), 1, "a capability that the user refused is not asked again");

    // Remove in Settings: the next request of each connection gets not_approved, and the host closes.
    host.apps().remove(&id()).unwrap();
    assert_eq!(refused(ask()).code, Some(ErrorCode::NotApproved));
    assert!(matches!(ask(), Err(Error::Gone)));
    assert_eq!(refused(second.request("list_models", fields(json!({})))).code, Some(ErrorCode::NotApproved));

    // After Remove, the next connection is asked again.
    let waiting = connect(&setup, &asking(&[]));
    services.wait_for_approvals(2);
    services.answer(true);
    waiting.join().unwrap().unwrap();
}

#[test]
fn an_edit_of_the_file_counts_at_the_next_request() {
    let setup = setup();
    write_apps(&setup.apps, &["transcribe.file"]);
    let _host = start_with(&setup, FakeServices::new(None));
    let client = Client::connect_to(&setup.socket).unwrap();
    let ask = || client.request("cancel_job", fields(json!({"job": 1})));
    assert_eq!(refused(ask()).code, Some(ErrorCode::BadRequest));
    let text = std::fs::read_to_string(&setup.apps).unwrap().replace("allowed = true", "allowed = false");
    std::fs::write(&setup.apps, text).unwrap();
    assert_eq!(refused(ask()).code, Some(ErrorCode::NotApproved));
}

#[test]
fn a_new_capability_asks_about_only_that_one() {
    let setup = setup();
    write_apps(&setup.apps, &["transcribe.file"]);
    let services = FakeServices::new(None);
    let _host = start_with(&setup, services.clone());

    let asked = [Capability::ModelsRead, Capability::TranscribeFile, Capability::AudioMic, Capability::Other("camera.read".into())];
    let waiting = connect(&setup, &asking(&asked));
    services.wait_for_approvals(1);
    let question = services.approvals.lock()[0].clone();
    assert_eq!(question.capabilities, vec![Capability::AudioMic], "not models.read, not a grant, not an unknown name");
    assert!(question.approved);

    // An app that waits for an answer keeps its old grants on its other connections.
    let other = Client::connect_to(&setup.socket).unwrap();
    assert_eq!(refused(other.request("cancel_job", fields(json!({"job": 1})))).code, Some(ErrorCode::BadRequest));

    services.answer(false);
    let client = waiting.join().unwrap().unwrap();
    assert_eq!(client.hello().capabilities, vec![Capability::ModelsRead, Capability::TranscribeFile], "Don't allow keeps the old grants");
    assert_eq!(client.hello().denied, vec![Capability::AudioMic, Capability::Other("camera.read".into())]);

    let again = Client::connect_as(&setup.socket, &asking(&asked)).unwrap();
    assert_eq!(again.hello().capabilities, client.hello().capabilities);
    assert_eq!(services.approvals.lock().len(), 1, "the answer stays");
}

#[test]
fn requests_of_a_connection_that_waits_for_the_answer_are_refused_without_a_close() {
    let setup = setup();
    let services = FakeServices::new(None);
    let _host = start_with(&setup, services.clone());
    #[cfg(unix)]
    {
        use std::io::{BufRead, BufReader, Write};
        let mut stream = std::os::unix::net::UnixStream::connect(&setup.socket).unwrap();
        let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
        let mut next = || serde_json::from_str::<serde_json::Value>(&lines.next().unwrap().unwrap()).unwrap();
        stream.write_all(b"{\"v\":1,\"type\":\"hello\",\"id\":\"h1\",\"protocol\":1}\n").unwrap();
        services.wait_for_approvals(1);
        stream.write_all(b"{\"v\":1,\"type\":\"list_models\",\"id\":\"r1\"}\n{\"v\":1,\"type\":\"hello\",\"id\":\"h2\",\"protocol\":1}\n").unwrap();
        for id in ["r1", "h2"] {
            let reply = next();
            assert_eq!((reply["id"].clone(), reply["code"].clone()), (json!(id), json!("bad_request")), "{reply}");
        }
        services.answer(true);
        let reply = next();
        assert_eq!((reply["id"].clone(), reply["type"].clone()), (json!("h1"), json!("ok")));
    }
}

#[test]
fn a_request_without_hello_gets_not_approved_and_no_question() {
    let setup = setup();
    let services = FakeServices::new(None);
    let _host = start_with(&setup, services.clone());
    #[cfg(unix)]
    {
        use std::io::{BufRead, BufReader, Write};
        let mut stream = std::os::unix::net::UnixStream::connect(&setup.socket).unwrap();
        let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
        stream.write_all(b"{\"v\":1,\"type\":\"list_models\",\"id\":\"r1\"}\n").unwrap();
        let reply: serde_json::Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
        assert_eq!(reply["code"], json!("not_approved"));
        assert!(lines.next().is_none(), "the host closes");
        assert!(services.approvals.lock().is_empty());
    }
}

#[test]
fn a_question_goes_away_when_the_app_closes_and_keeps_no_answer() {
    let setup = setup();
    let services = FakeServices::new(None);
    let _host = start_with(&setup, services.clone());
    #[cfg(unix)]
    {
        use std::io::Write;
        let mut stream = std::os::unix::net::UnixStream::connect(&setup.socket).unwrap();
        stream.write_all(b"{\"v\":1,\"type\":\"hello\",\"id\":\"h1\",\"protocol\":1}\n").unwrap();
        services.wait_for_approvals(1);
        drop(stream);
        eventually("the host takes the question back", || services.approvals_withdrawn.load(Ordering::SeqCst) == 1);
        assert!(!setup.apps.exists(), "no answer: {:?}", std::fs::read_to_string(&setup.apps));
        // The next connection is asked again.
        let waiting = connect(&setup, &asking(&[]));
        services.wait_for_approvals(2);
        services.answer(true);
        waiting.join().unwrap().unwrap();
    }
}

#[test]
fn the_name_from_the_app_is_cleaned_and_only_a_claim() {
    let setup = setup();
    let services = FakeServices::new(None);
    let _host = start_as(&setup, vec![Ok(path("python3"))], services.clone());
    let app = AppHello { name: Some(format!("Mee\u{202E}ting\nRecorder{}", "x".repeat(80))), capabilities: vec![] };
    let waiting = connect(&setup, &app);
    services.wait_for_approvals(1);
    let question = services.approvals.lock()[0].clone();
    assert_eq!(question.name, "python3", "the name from the system");
    assert!(question.interpreter);
    let claimed = question.claimed_name.unwrap();
    assert!(claimed.starts_with("MeetingRecorder") && claimed.chars().count() == 40, "{claimed}");
    services.answer(false);
    assert_eq!(code_of(waiting.join().unwrap()), Some(ErrorCode::NotApproved));
    assert!(std::fs::read_to_string(&setup.apps).unwrap().contains("name = \"python3\""), "the record keeps the name from the system");
}

#[test]
fn five_connections_of_one_identity_get_one_question_and_one_busy() {
    let setup = setup();
    let services = FakeServices::new(None);
    let _host = start_with(&setup, services.clone());
    let waiting: Vec<_> = (0..4).map(|_| connect(&setup, &asking(&[]))).collect();
    services.wait_for_approvals(1);
    // Each of the four is in the queue when the fifth comes.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(code_of(Client::connect_as(&setup.socket, &asking(&[]))), Some(ErrorCode::Busy));
    assert!(waiting.iter().all(|thread| !thread.is_finished()));
    services.answer(true);
    for thread in waiting {
        thread.join().unwrap().unwrap();
    }
    assert_eq!(services.approvals.lock().len(), 1);
}

#[test]
fn different_identities_wait_for_one_question_at_a_time() {
    let setup = setup();
    let services = FakeServices::new(None);
    let callers = (0..5).map(|n| Ok(path(&format!("app{n}")))).collect();
    let _host = start_as(&setup, callers, services.clone());
    let waiting: Vec<_> = (0..4)
        .map(|n| {
            let thread = connect(&setup, &asking(&[]));
            // In this order, so each connection has its caller and waits before the next.
            if n == 0 {
                services.wait_for_approvals(1);
            }
            std::thread::sleep(Duration::from_millis(100));
            thread
        })
        .collect();
    assert_eq!(code_of(Client::connect_as(&setup.socket, &asking(&[]))), Some(ErrorCode::Busy));
    for shown in 1..=4 {
        services.wait_for_approvals(shown);
        services.answer(true);
    }
    for thread in waiting {
        thread.join().unwrap().unwrap();
    }
    assert_eq!(services.approvals_most.load(Ordering::SeqCst), 1, "one question on the screen");
    let names: Vec<_> = services.approvals.lock().iter().map(|question| question.name.clone()).collect();
    assert_eq!(names, ["app0", "app1", "app2", "app3"], "in the order of the connections");
}

#[test]
fn an_approved_app_that_gets_busy_keeps_its_connection() {
    let setup = setup();
    write_app(&setup.apps, std::path::Path::new("/opt/known"), &[]);
    let services = FakeServices::new(None);
    let mut callers: Vec<_> = (0..4).map(|n| Ok(path(&format!("app{n}")))).collect();
    callers.push(Ok(path("known")));
    let _host = start_as(&setup, callers, services.clone());
    let waiting: Vec<_> = (0..4)
        .map(|_| {
            let thread = connect(&setup, &asking(&[]));
            std::thread::sleep(Duration::from_millis(100));
            thread
        })
        .collect();
    services.wait_for_approvals(1);
    #[cfg(unix)]
    {
        use std::io::{BufRead, BufReader, Write};
        let mut stream = std::os::unix::net::UnixStream::connect(&setup.socket).unwrap();
        let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
        let mut next = || serde_json::from_str::<serde_json::Value>(&lines.next().unwrap().unwrap()).unwrap();
        stream.write_all(b"{\"v\":1,\"type\":\"hello\",\"id\":\"h1\",\"protocol\":1,\"capabilities\":[\"audio.mic\"]}\n").unwrap();
        assert_eq!(next()["code"], json!("busy"));
        stream.write_all(b"{\"v\":1,\"type\":\"bye\",\"id\":\"b\"}\n").unwrap();
        assert_eq!(next()["type"], json!("ok"), "the connection stays open");
    }
    for shown in 1..=4 {
        services.wait_for_approvals(shown);
        services.answer(false);
    }
    for thread in waiting {
        assert_eq!(code_of(thread.join().unwrap()), Some(ErrorCode::NotApproved));
    }
}

#[test]
fn a_program_with_no_identity_gets_not_approved_and_no_question() {
    let setup = setup();
    let services = FakeServices::new(None);
    let _host = start_as(&setup, vec![Err("the signature is not valid".to_string())], services.clone());
    assert_eq!(code_of(Client::connect_as(&setup.socket, &asking(&[Capability::AudioMic]))), Some(ErrorCode::NotApproved));
    assert!(services.approvals.lock().is_empty());
    assert!(!setup.apps.exists());
}

#[test]
fn a_broken_file_lets_no_app_connect_and_is_not_written_over() {
    let setup = setup();
    std::fs::write(&setup.apps, "[[apps]\n").unwrap();
    let services = FakeServices::new(None);
    let _host = start_with(&setup, services.clone());
    assert_eq!(code_of(Client::connect_as(&setup.socket, &asking(&[]))), Some(ErrorCode::NotApproved));
    assert!(services.approvals.lock().is_empty());
    assert_eq!(std::fs::read_to_string(&setup.apps).unwrap(), "[[apps]\n");
}

#[test]
fn a_signed_app_keeps_one_record_through_an_update() {
    let setup = setup();
    let services = FakeServices::new(None);
    let _host = start_as(&setup, vec![Ok(signed("app.example.notes"))], services.clone());
    let waiting = connect(&setup, &asking(&[Capability::AudioMic]));
    services.wait_for_approvals(1);
    let question = services.approvals.lock()[0].clone();
    assert_eq!(question.app_id, "ABCDE12345/app.example.notes");
    assert_eq!(question.signer.as_deref(), Some("Example LLC"));
    services.answer(true);
    waiting.join().unwrap().unwrap();
    let text = std::fs::read_to_string(&setup.apps).unwrap();
    assert!(text.contains("team_id = \"ABCDE12345\"") && text.contains("identifier = \"app.example.notes\"") && text.contains("signer = \"Example LLC\"") && !text.contains("path ="), "{text}");
}

/// The real checks of the system, with programs that the test changes.
#[cfg(target_os = "macos")]
mod real_signature {
    use super::*;
    use std::process::Command;

    /// The example program, built for this test.
    fn example() -> PathBuf {
        // <target>/<profile>/deps/<this test>
        let profile = this_program().parent().unwrap().parent().unwrap().to_path_buf();
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let target = profile.parent().unwrap();
        let built = Command::new(cargo).args(["build", "--quiet", "-p", "sayso-extension", "--example", "sayso-ext", "--manifest-path"]).arg(&manifest).arg("--target-dir").arg(target).status().unwrap();
        assert!(built.success());
        profile.join("examples/sayso-ext")
    }

    fn run(program: &std::path::Path, setup: &Setup) -> std::process::Output {
        Command::new(program).arg("hello").env("SAYSO_HOST_SOCKET", &setup.socket).output().unwrap()
    }

    /// Change one byte in the strings of the symbols, which the kernel does
    /// not read when it starts the program. The signature does not match now.
    fn break_signature(program: &std::path::Path) {
        let output = Command::new("otool").args(["-l"]).arg(program).output().unwrap();
        let text = String::from_utf8_lossy(&output.stdout).to_string();
        let lines: Vec<&str> = text.lines().map(str::trim).collect();
        let at = lines.iter().position(|line| *line == "cmd LC_SYMTAB").unwrap();
        let value = |key: &str| lines[at..].iter().find_map(|line| line.strip_prefix(key)).unwrap().trim().parse::<usize>().unwrap();
        let (offset, size) = (value("stroff "), value("strsize "));
        let mut bytes = std::fs::read(program).unwrap();
        bytes[offset + size / 2] ^= 0xFF;
        std::fs::write(program, bytes).unwrap();
    }

    fn sign(program: &std::path::Path) {
        assert!(Command::new("codesign").args(["--force", "--sign", "-"]).arg(program).status().unwrap().success());
    }

    #[test]
    fn a_broken_signature_gets_no_question_and_an_ad_hoc_one_counts_as_a_path() {
        let folder = tempfile::tempdir().unwrap();
        let setup = setup();
        let services = FakeServices::new(None);
        let _host = start_with(&setup, services.clone());

        // Any folder, and ad hoc: a question that names the path.
        let good = folder.path().join("good-program");
        std::fs::copy(example(), &good).unwrap();
        sign(&good);
        let waiter = {
            let (good, setup_socket) = (good.clone(), setup.socket.clone());
            std::thread::spawn(move || Command::new(good).arg("hello").env("SAYSO_HOST_SOCKET", setup_socket).output().unwrap())
        };
        services.wait_for_approvals(1);
        let question = services.approvals.lock()[0].clone();
        assert_eq!(question.identity, Identity::Path(good.canonicalize().unwrap()));
        services.answer(true);
        let output = waiter.join().unwrap();
        assert!(output.status.success(), "{output:?}");

        // The signature does not match the file: no question, and not_approved.
        let broken = folder.path().join("broken-program");
        std::fs::copy(example(), &broken).unwrap();
        sign(&broken);
        break_signature(&broken);
        let output = run(&broken, &setup);
        assert!(!output.status.success(), "{output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("did not approve"), "{stderr}");
        assert_eq!(services.approvals.lock().len(), 1, "no second question");
    }
}
