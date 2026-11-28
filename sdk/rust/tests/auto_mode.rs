//! `Mode::Auto` against the real Host API (`sayso-testhost`) and the fake engine: Sayso when it
//! runs, the engine of the app when it does not, and a change of mode only when no work is
//! active (scenarios A01 to A03).
#![cfg(feature = "local")]

mod common;

use common::*;
use sayso_sdk::protocol::{ErrorCode, SessionStart, Source};
use sayso_sdk::{ActiveMode, App, Engine, Event, Mode, Options, Speech, Timing};
use std::path::Path;
use std::time::Duration;

fn auto(program: &Path, folder: &Path, socket: &Path) -> Speech {
    let engine = Engine::from_program(program)
        .store(folder.join("models").join("v1"))
        .models_dir(folder.join("models"));
    let options = Options::new(Mode::Auto, App::new("Notes"))
        .capabilities(ALL_CAPABILITIES)
        .engine(engine)
        .socket(socket)
        .timing(Timing {
            watch: Duration::from_millis(20),
            ..Timing::default()
        });
    Speech::connect(options).unwrap()
}

fn mode_changed(speech: &Speech, seen: &mut Vec<Event>, to: ActiveMode) -> sayso_sdk::Features {
    until(speech, seen, |e| match e {
        Event::ModeChanged { mode, features } if *mode == to => Some(features.clone()),
        _ => None,
    })
}

#[test]
fn auto_starts_local_follows_sayso_when_idle_and_goes_back_when_sayso_quits() {
    let Some(program) = fake_engine() else { return };
    let socket = SocketPath::new();
    let folder = tempfile::tempdir().unwrap();
    let speech = auto(&program, folder.path(), &socket.path);
    assert_eq!(speech.mode(), ActiveMode::Local, "no Sayso runs");
    speech.download_model("fake-stream", "tests").unwrap();
    let session = speech
        .start_session(SessionStart {
            sources: vec![Source::Client],
            model: Some("fake-stream".into()),
            ..Default::default()
        })
        .unwrap();

    let Some(mut host) = TestHost::start(&socket.path, &[]) else {
        return;
    };
    // Sayso is there, but a live session runs: the change waits until the app has no work.
    std::thread::sleep(Duration::from_millis(500));
    assert!(speech.list_models(None).is_ok());
    assert_eq!(
        speech.mode(),
        ActiveMode::Local,
        "a running session keeps local mode"
    );

    speech.stop_session(session.session).unwrap();
    let mut seen = Vec::new();
    let features = mode_changed(&speech, &mut seen, ActiveMode::Sayso);
    assert!(
        !features.host.is_empty() && features.enhance && features.subscribe,
        "{features:?}"
    );
    assert_eq!(speech.mode(), ActiveMode::Sayso);
    assert!(
        speech.list_models(None).is_ok(),
        "the request goes to Sayso now"
    );
    passed("A01");

    host.close();
    let after = speech.list_models(None);
    assert!(
        after.is_ok(),
        "the first request after Sayso quit runs in local mode: {after:?}"
    );
    let features = mode_changed(&speech, &mut seen, ActiveMode::Local);
    assert!(
        features.host.is_empty() && !features.enhance,
        "{features:?}"
    );
    assert_eq!(speech.mode(), ActiveMode::Local);
    passed("A03");
}

#[test]
fn a_host_that_is_killed_while_a_job_runs_ends_the_job_once_and_auto_goes_local() {
    let Some(program) = fake_engine() else { return };
    let socket = SocketPath::new();
    let Some(mut host) = TestHost::start(&socket.path, &["--speed", "0.5"]) else {
        return;
    };
    let folder = tempfile::tempdir().unwrap();
    let speech = auto(&program, folder.path(), &socket.path);
    assert_eq!(
        speech.mode(),
        ActiveMode::Sayso,
        "Sayso runs, so Auto uses it"
    );
    ready(&speech);
    let path = speech_wav(folder.path(), "long.wav", &[900; 60]);
    let job = speech
        .transcribe_file(sayso_sdk::protocol::TranscribeFile {
            path,
            ..Default::default()
        })
        .unwrap();
    host.kill();
    let mut seen = Vec::new();
    let code = until(&speech, &mut seen, |e| match e {
        Event::Message(sayso_sdk::protocol::Event::JobFailed {
            job: failed, code, ..
        }) if *failed == job => Some(code.clone()),
        _ => None,
    });
    assert_eq!(code, Some(ErrorCode::ConnectionLost));
    mode_changed(&speech, &mut seen, ActiveMode::Local);
    let ends = seen
        .iter()
        .filter(|e| {
            matches!(
                e,
                Event::Message(
                    sayso_sdk::protocol::Event::JobDone { .. }
                        | sayso_sdk::protocol::Event::JobFailed { .. }
                )
            )
        })
        .count();
    assert_eq!(ends, 1);
    passed("A02");
}

#[test]
fn auto_never_starts_sayso_and_a_refused_app_stays_local() {
    let Some(program) = fake_engine() else { return };
    let socket = SocketPath::new();
    let Some(_host) = TestHost::start(&socket.path, &["--approve", "deny"]) else {
        return;
    };
    let folder = tempfile::tempdir().unwrap();
    let speech = auto(&program, folder.path(), &socket.path);
    assert_eq!(
        speech.mode(),
        ActiveMode::Local,
        "the user said no, so Auto uses the engine of the app"
    );
    assert!(speech.list_models(None).is_ok());
}
