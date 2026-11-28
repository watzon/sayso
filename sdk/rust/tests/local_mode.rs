//! Local mode against `sayso-fake-engine`: the engine of the app starts as a process, the model
//! store v1 is real, and the jobs and the live sessions of the crate run on top of it (scenarios G).
#![cfg(feature = "local")]

mod common;

use common::*;
use sayso_sdk::protocol::{
    self, ErrorCode, Event as Host, ModelState, Role, SessionStart, Source, TranscribeFile,
};
use sayso_sdk::{ActiveMode, App, Engine, Event, Mode, ModelChoices, Options, Speech};
use std::path::{Path, PathBuf};

struct Local {
    speech: Speech,
    folder: tempfile::TempDir,
}

impl Local {
    fn models(&self) -> PathBuf {
        self.folder.path().join("models")
    }
}

fn options(program: &Path, folder: &Path) -> Options {
    let engine = Engine::from_program(program)
        .store(folder.join("models").join("v1"))
        .models_dir(folder.join("models"));
    Options::new(Mode::Local, App::new("Notes"))
        .engine(engine)
        .socket(folder.join("no-sayso.sock"))
}

fn start() -> Option<Local> {
    let program = fake_engine()?;
    let folder = tempfile::tempdir().unwrap();
    let speech = Speech::connect(options(&program, folder.path())).unwrap();
    Some(Local { speech, folder })
}

fn job_end(speech: &Speech, seen: &mut Vec<Event>, job: u64) -> Result<String, String> {
    until(speech, seen, |e| match e {
        Event::Message(Host::JobDone { job: done, text }) if *done == job => Some(Ok(text.clone())),
        Event::Message(Host::JobFailed {
            job: failed,
            message,
            ..
        }) if *failed == job => Some(Err(message.clone())),
        _ => None,
    })
}

#[test]
fn local_mode_lists_the_catalog_downloads_a_model_and_transcribes_a_file() {
    let Some(local) = start() else { return };
    let speech = &local.speech;
    assert_eq!(speech.mode(), ActiveMode::Local);
    let features = speech.features();
    assert!(
        !features.enhance && !features.subscribe && !features.download_question,
        "{features:?}"
    );

    let models = speech.list_models(None).unwrap();
    let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
    assert!(
        ids.contains(&"fake-final") && ids.contains(&"fake-vad"),
        "{ids:?}"
    );
    assert!(models.iter().all(|m| m.state == ModelState::NotDownloaded));
    assert_eq!(
        speech
            .list_models(Some(Role::Vad))
            .unwrap()
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        ["fake-vad"]
    );
    assert!(
        speech
            .features()
            .engine
            .is_some_and(|f| f.iter().any(|f| f == "store_v1")),
        "the engine started"
    );

    let path = speech_wav(local.folder.path(), "speech.wav", &[900, 600]);
    let refused = speech
        .transcribe_file(TranscribeFile {
            path: path.clone(),
            model: Some("fake-final".into()),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(
        refused.code,
        Some(ErrorCode::ModelNotDownloaded),
        "{refused}"
    );

    speech.download_model("fake-final", "tests").unwrap();
    assert!(
        local.models().join("v1").join("artifacts").is_dir(),
        "the engine wrote the model store v1"
    );
    assert!(
        speech
            .list_models(None)
            .unwrap()
            .iter()
            .any(|m| m.id == "fake-final" && m.state == ModelState::Downloaded)
    );

    let job = speech
        .transcribe_file(TranscribeFile {
            path,
            model: Some("fake-final".into()),
            ..Default::default()
        })
        .unwrap();
    let mut seen = Vec::new();
    let text = job_end(speech, &mut seen, job).unwrap();
    assert!(text.starts_with("alpha"), "{text}");
    assert!(
        seen.iter()
            .any(|e| matches!(e, Event::Message(Host::Segment(s)) if s.job == Some(job))),
        "{:?}",
        seen.iter().map(kind).collect::<Vec<_>>()
    );
    assert_eq!(
        seen.iter()
            .filter(|e| matches!(
                e,
                Event::Message(Host::JobDone { .. } | Host::JobFailed { .. })
            ))
            .count(),
        1,
        "one end"
    );
    passed("G01");
}

#[test]
fn a_file_with_a_vad_model_gets_the_speakers_of_a_diarization_model() {
    let Some(local) = start() else { return };
    let speech = &local.speech;
    for model in ["fake-final", "fake-diar"] {
        speech.download_model(model, "tests").unwrap();
    }
    let path = speech_wav(local.folder.path(), "two.wav", &[900, 900, 900]);
    let job = speech
        .transcribe_file(TranscribeFile {
            path,
            model: Some("fake-final".into()),
            diarize: Some(true),
            diarization_model: Some("fake-diar".into()),
            speakers: Some(2),
            ..Default::default()
        })
        .unwrap();
    let mut seen = Vec::new();
    let turns = until(speech, &mut seen, |e| match e {
        Event::Message(Host::Speakers(s)) if s.job == Some(job) => Some(s.turns.clone()),
        _ => None,
    });
    assert!(turns.len() >= 2, "{turns:?}");
    job_end(speech, &mut seen, job).unwrap();
}

#[test]
fn cancel_job_ends_a_running_job_with_one_job_failed() {
    let Some(local) = start() else { return };
    let speech = &local.speech;
    speech.download_model("fake-final", "tests").unwrap();
    let path = speech_wav(local.folder.path(), "long.wav", &[900; 40]);
    let job = speech
        .transcribe_file(TranscribeFile {
            path,
            model: Some("fake-final".into()),
            ..Default::default()
        })
        .unwrap();
    speech.cancel_job(job).unwrap();
    let mut seen = Vec::new();
    let end = job_end(speech, &mut seen, job);
    assert_eq!(end, Err("The job was cancelled.".to_string()), "{end:?}");
    speech.cancel_job(job).unwrap();
    assert!(speech.cancel_job(job + 100).is_err());
    passed("J03");
}

#[test]
fn a_live_session_of_the_client_source_has_partials_and_a_final_text() {
    let Some(local) = start() else { return };
    let speech = &local.speech;
    speech.download_model("fake-stream", "tests").unwrap();
    let session = speech
        .start_session(SessionStart {
            sources: vec![Source::Client],
            model: Some("fake-stream".into()),
            ..Default::default()
        })
        .unwrap();
    for chunk in common::speech(&[900, 900]).chunks(3200) {
        speech.session_audio(session.session, chunk).unwrap();
    }
    let mut seen = Vec::new();
    until(speech, &mut seen, |e| {
        matches!(e, Event::Message(Host::Partial(p)) if p.session == session.session).then_some(())
    });
    let text = speech.stop_session(session.session).unwrap();
    assert!(text.contains("alpha"), "{text:?}");
    assert!(
        speech.stop_session(session.session).is_err(),
        "a stopped session has no end any more"
    );
    passed("G02");
}

#[test]
fn a_file_is_cut_where_the_voice_stops_for_long() {
    let Some(program) = fake_engine() else { return };
    let folder = tempfile::tempdir().unwrap();
    let speech = Speech::connect(options(&program, folder.path()).models(ModelChoices {
        vad_model: Some("basic".into()),
        ..Default::default()
    }))
    .unwrap();
    speech.download_model("fake-final", "tests").unwrap();
    // Two bursts of 3 words with 5 s of silence between them: more than the longest pause inside
    // one chunk (3 s), so the final pass runs on two chunks. The fake engine counts words from
    // "alpha" in each call, so the cut shows in the text.
    let mut samples = vec![0i16; 8000];
    for _ in 0..2 {
        samples.extend(common::tone(16000, 900, 0, 900));
        samples.extend(std::iter::repeat_n(0i16, 5 * 16000));
    }
    let path = folder.path().join("cut.wav");
    std::fs::write(&path, wav_bytes(16000, &samples)).unwrap();
    let job = speech
        .transcribe_file(TranscribeFile {
            path,
            model: Some("fake-final".into()),
            vad_model: Some("basic".into()),
            ..Default::default()
        })
        .unwrap();
    let mut seen = Vec::new();
    let text = job_end(&speech, &mut seen, job).unwrap();
    assert_eq!(
        text, "alpha bravo charlie alpha bravo charlie",
        "two chunks, each with its own words"
    );
    passed("G03");
}

#[cfg(unix)]
#[test]
fn an_engine_that_does_not_start_says_what_it_wrote_on_stderr() {
    use std::os::unix::fs::PermissionsExt;
    let Some(fake) = fake_engine() else { return };
    let folder = tempfile::tempdir().unwrap();
    let wrapper = folder.path().join("engine.sh");
    std::fs::write(&wrapper, format!("#!/bin/sh\nSAYSO_FAKE_SCRIPT='{{\"stderr\":[\"error while loading shared libraries: libfake.so.1: cannot open shared object file\"],\"exit_at_start\":127}}' exec '{}' \"$@\"\n", fake.display())).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let speech = Speech::connect(options(&wrapper, folder.path())).unwrap();
    let error = speech.list_models(None).unwrap_err();
    assert_eq!(error.code, Some(ErrorCode::Unsupported), "{error}");
    assert!(
        error.message.contains("libfake.so.1"),
        "the error carries the tail of stderr: {error}"
    );
    passed("G04");
}

#[test]
fn find_engine_takes_the_last_version_and_the_best_build_for_the_processor() {
    let folder = tempfile::tempdir().unwrap();
    let exe = if cfg!(windows) { ".exe" } else { "" };
    for version in ["0.11.0", "0.12.0"] {
        let dir = folder.path().join(format!("sayso-engine-{version}"));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["sayso-engine", "sayso-engine-avx", "sayso-engine-baseline"] {
            std::fs::write(dir.join(format!("{name}{exe}")), b"").unwrap();
        }
    }
    std::fs::create_dir_all(folder.path().join("other")).unwrap();
    let found = sayso_sdk::find_engine(folder.path()).unwrap();
    assert_eq!(
        found.parent().unwrap().file_name().unwrap(),
        "sayso-engine-0.12.0",
        "the folder whose name sorts last wins"
    );
    assert!(found.is_file());
    let name = found.file_name().unwrap().to_string_lossy().into_owned();
    if cfg!(target_arch = "x86_64") {
        assert!(name.starts_with("sayso-engine"), "{name}");
    } else {
        assert_eq!(name, format!("sayso-engine{exe}"));
    }
    assert!(sayso_sdk::find_engine(folder.path().join("none")).is_err());
    passed("G05");
}

#[test]
fn the_model_store_is_in_the_models_folder_of_the_data_folder() {
    assert!(sayso_sdk::model_store().ends_with(Path::new("models").join("v1")));
}

#[test]
fn local_mode_needs_the_engine_and_sayso_for_enhance_and_subscribe() {
    let Some(local) = start() else { return };
    let enhance = local
        .speech
        .enhance(protocol::Enhance {
            text: Some("hi".into()),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(enhance.code, Some(ErrorCode::Unsupported));
    let subscribe = local.speech.subscribe(&["dictation_done"]).unwrap_err();
    assert_eq!(subscribe.code, Some(ErrorCode::Unsupported));
    let socket = SocketPath::new();
    let none = Speech::connect(Options::new(Mode::Local, App::new("Notes")).socket(&socket.path))
        .err()
        .unwrap();
    assert_eq!(none.code, Some(ErrorCode::BadRequest), "{none}");
    let missing = Speech::connect(
        Options::new(Mode::Local, App::new("Notes"))
            .engine(Engine::from_program(local.folder.path().join("nothing")))
            .socket(&socket.path),
    )
    .err()
    .unwrap();
    assert_eq!(missing.code, Some(ErrorCode::Unsupported), "{missing}");
}

#[test]
fn local_mode_never_talks_to_sayso_even_when_it_runs() {
    let Some(program) = fake_engine() else { return };
    let socket = SocketPath::new();
    let Some(mut host) = TestHost::start(&socket.path, &["--approve", "ignore"]) else {
        return;
    };
    let folder = tempfile::tempdir().unwrap();
    let speech = Speech::connect(options(&program, folder.path()).socket(&socket.path)).unwrap();
    assert_eq!(speech.mode(), ActiveMode::Local);
    assert!(speech.list_models(None).is_ok());
    // A host that is asked would print the question of the approval.
    std::thread::sleep(std::time::Duration::from_millis(600));
    host.send(serde_json::json!({"approve": "allow"}));
    assert_eq!(speech.mode(), ActiveMode::Local);
}
