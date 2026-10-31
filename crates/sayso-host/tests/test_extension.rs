//! The test extension (`sayso-sdk/examples/sayso-ext.rs`) against a
//! host on a real socket. The test builds the program with cargo, and the
//! record of `apps.toml` has its path, so the identity check is the real one.

use sayso_core::config::{Provider, ProviderKind};
use sayso_core::style::StyleLibrary;
use sayso_sdk::protocol::{DictationDone, ErrorCode, Event, HostMessage, Reply, decode, Decoded, Message};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

mod common;
use common::*;

/// Build the program once, into the target folder of this test.
fn program() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        // <target>/<profile>/deps/<this test>
        let exe = std::env::current_exe().unwrap();
        let profile_dir = exe.parent().and_then(Path::parent).unwrap().to_path_buf();
        let target_dir = profile_dir.parent().unwrap();
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut build = Command::new(cargo);
        build.args(["build", "--quiet", "-p", "sayso-sdk", "--example", "sayso-ext", "--manifest-path"]).arg(&manifest).arg("--target-dir").arg(target_dir);
        if profile_dir.file_name().is_some_and(|name| name == "release") {
            build.arg("--release");
        }
        let status = build.status().expect("cargo runs");
        assert!(status.success(), "the test extension builds");
        profile_dir.join("examples").join(format!("sayso-ext{}", std::env::consts::EXE_SUFFIX))
    })
}

struct World {
    host: sayso_host::Host,
    setup: Setup,
    _styles: tempfile::TempDir,
}

fn world(granted: &[&str]) -> World {
    let program = program();
    let setup = setup();
    write_app(&setup.apps, &program.canonicalize().unwrap(), granted);
    let services = FakeServices::new(Some(FakeEngine::new()));
    {
        let mut config = services.config.lock();
        config.ai.enabled = true;
        config.ai.providers = vec![Provider { id: "claude".into(), name: "Claude".into(), kind: ProviderKind::ClaudeCli { path: None, model: "haiku".into(), config_dir: None, thinking: None } }];
        config.ai.default_provider = Some("claude".into());
    }
    let styles = tempfile::tempdir().unwrap();
    *services.styles.lock() = Some(StyleLibrary::load(styles.path()));
    let host = start_with(&setup, services);
    World { host, setup, _styles: styles }
}

impl World {
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(program());
        command.args(args).env(sayso_protocol::socket::SOCKET_ENV, &self.setup.socket);
        command
    }

    fn run(&self, args: &[&str]) -> (Output, Vec<(Option<String>, HostMessage)>) {
        let output = self.command(args).output().unwrap();
        let lines = String::from_utf8_lossy(&output.stdout).lines().map(message).collect();
        (output, lines)
    }
}

/// One line of the output: a message of the host.
fn message(line: &str) -> (Option<String>, HostMessage) {
    match decode::<HostMessage>(line) {
        Ok(Decoded::Known(Message { id, body })) => (id, body),
        other => panic!("not a message of the host: {line} ({other:?})"),
    }
}

fn ok_fields(found: &[(Option<String>, HostMessage)]) -> serde_json::Map<String, serde_json::Value> {
    match found.last() {
        Some((_, HostMessage::Reply(Reply::Ok(fields)))) => fields.clone(),
        other => panic!("the last line is not ok: {other:?}"),
    }
}

#[test]
fn hello_list_models_and_enhance() {
    let world = world(&["ai.enhance"]);
    let (output, found) = world.run(&["hello"]);
    assert!(output.status.success(), "{output:?}");
    assert!(ok_fields(&found)["capabilities"].as_array().unwrap().contains(&"ai.enhance".into()));

    let (output, found) = world.run(&["list-models", "--role", "vad"]);
    assert!(output.status.success(), "{output:?}");
    let models = ok_fields(&found)["models"].as_array().unwrap().clone();
    assert!(!models.is_empty() && models.iter().all(|m| m["roles"].as_array().unwrap().contains(&"vad".into())), "{models:?}");

    let (output, found) = world.run(&["enhance", "um hello there", "--style", "email"]);
    assert!(output.status.success(), "{output:?}");
    let fields = ok_fields(&found);
    assert_eq!((fields["text"].as_str(), fields["provider"].as_str()), (Some("[email] um hello there"), Some("claude")));
}

#[test]
fn a_refused_request_prints_the_error_and_ends_with_1() {
    let world = world(&[]);
    let (output, found) = world.run(&["transcribe", "/tmp/talk.wav"]);
    assert_eq!(output.status.code(), Some(1));
    let Some((_, HostMessage::Reply(Reply::Error(error)))) = found.last() else { panic!("no error line: {found:?}") };
    assert_eq!(error.code, Some(ErrorCode::Capability));
    let (output, _) = world.run(&["no-such-command"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn watch_prints_each_dictation_until_sayso_closes() {
    let world = world(&["dictation.observe"]);
    let mut child = world.command(&["watch"]).stdout(Stdio::piped()).spawn().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut next_line = move || lines.next().expect("a line").unwrap();
    // The reply to `subscribe` comes first.
    let (_, first) = message(&next_line());
    assert!(matches!(&first, HostMessage::Reply(Reply::Ok(fields)) if fields["events"] == serde_json::json!(["dictation_done"])), "{first:?}");

    let done = DictationDone { text: "Hello there.".into(), transcript: "hello there".into(), app: Some("Notes".into()), site: None, style: Some("clean".into()), time: "2026-10-26T09:41:00Z".into() };
    world.host.dictation_done(done.clone(), true, false);
    world.host.dictation_done(done.clone(), false, false);
    // Only the second: incognito was on at the start of the first.
    assert_eq!(message(&next_line()), (None, HostMessage::Event(Event::DictationDone(done))));

    world.host.close();
    assert_eq!(next_line(), r#"{"type":"host_gone"}"#);
    let status = wait(&mut child, Duration::from_secs(10));
    assert!(status.success());
}

fn wait(child: &mut std::process::Child, limit: Duration) -> std::process::ExitStatus {
    let deadline = std::time::Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("the test extension did not end");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
