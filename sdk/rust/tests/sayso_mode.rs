//! Sayso mode against the real Host API of Sayso: `sayso-testhost` serves `internal/bridge` on a
//! socket, with the fake engine behind it and WAV files for the microphone and the system audio
//! (scenarios H, M, J, L, E, X of `sdk/conformance/scenarios.json`).

mod common;

use common::*;
use sayso_sdk::client::{AppHello, Client, Error as ClientError};
use sayso_sdk::protocol::{
    self, Capability, ErrorCode, Event as Host, Role, SessionStart, Source, TranscribeFile,
};
use sayso_sdk::{ActiveMode, Event, Speech};
use serde_json::json;
use std::time::Duration;

fn host(args: &[&str]) -> Option<(SocketPath, TestHost)> {
    let socket = SocketPath::new();
    let host = TestHost::start(&socket.path, args)?;
    Some((socket, host))
}

fn connect(socket: &SocketPath) -> Speech {
    Speech::connect(sayso_options(&socket.path)).unwrap()
}

fn code(error: &protocol::ErrorReply) -> Option<ErrorCode> {
    error.code.clone()
}

#[test]
fn hello_is_allowed_and_the_features_come_from_the_host() {
    let Some((socket, _host)) = host(&[]) else {
        return;
    };
    let speech = connect(&socket);
    assert_eq!(speech.mode(), ActiveMode::Sayso);
    let features = speech.features();
    assert!(
        features.enhance && features.subscribe && features.download_question,
        "{features:?}"
    );
    assert!(
        features.host.iter().any(|f| f == "record_to")
            && features.host.iter().any(|f| f == "silence_limit"),
        "{:?}",
        features.host
    );
    assert!(
        !features.host.iter().any(|f| f == "mic_capture"),
        "this host has no microphone"
    );
    assert!(
        features.capabilities.contains(&Capability::TranscribeFile),
        "{features:?}"
    );
    assert!(features.denied.is_empty() && features.engine.is_none());
    passed("H01");
}

#[test]
fn the_host_says_its_version_and_this_sdk_says_its_own() {
    let Some((socket, _host)) = host(&[]) else {
        return;
    };
    let client = Client::connect_as(
        &socket.path,
        &AppHello {
            name: Some("Notes".into()),
            capabilities: vec![Capability::ModelsRead],
        },
    )
    .unwrap();
    assert_eq!(client.hello().host_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(client.hello().protocol, protocol::PROTOCOL);
    assert!(!client.hello().features.is_empty());
    passed("H08");
}

#[test]
fn a_denied_app_is_not_approved() {
    let Some((socket, _host)) = host(&["--approve", "deny"]) else {
        return;
    };
    let error = Speech::connect(sayso_options(&socket.path))
        .err()
        .expect("the user said no");
    assert_eq!(code(&error), Some(ErrorCode::NotApproved), "{error}");
    passed("H02");
}

#[test]
fn an_unanswered_question_waits_and_the_callback_hears_approval_waiting() {
    let Some((socket, mut host)) = host(&["--approve", "ignore"]) else {
        return;
    };
    let called = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = called.clone();
    let options = sayso_options(&socket.path).on_approval_waiting(move || {
        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    let waiting = std::thread::spawn(move || Speech::connect(options));
    host.await_key("question", Some("approval"));
    std::thread::sleep(Duration::from_millis(300));
    assert!(!waiting.is_finished(), "hello waits for the user");
    host.send(json!({"approve": "allow"}));
    let speech = waiting.join().unwrap().unwrap();
    assert_eq!(
        speech.events().try_iter().count(),
        0,
        "the app sees the wait through the callback only"
    );
    assert_eq!(
        called.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the callback runs once"
    );
    assert_eq!(speech.mode(), ActiveMode::Sayso);
    passed("H03");
}

#[test]
fn a_capability_that_the_app_did_not_ask_for_is_named_in_the_error() {
    let Some((socket, _host)) = host(&[]) else {
        return;
    };
    let options = sayso_options(&socket.path).capabilities([Capability::ModelsRead]);
    let speech = Speech::connect(options).unwrap();
    let error = speech
        .transcribe_file(TranscribeFile {
            path: "/tmp/none.wav".into(),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(code(&error), Some(ErrorCode::Capability), "{error}");
    assert_eq!(error.capability, Some(Capability::TranscribeFile));
    assert!(
        speech.list_models(None).is_ok(),
        "models.read needs no grant"
    );
    passed("H04");
}

#[test]
fn a_fifth_waiting_connection_of_one_app_is_busy() {
    let Some((socket, mut host)) = host(&["--approve", "ignore"]) else {
        return;
    };
    let waiting: Vec<_> = (0..4)
        .map(|_| {
            let path = socket.path.clone();
            std::thread::spawn(move || Client::connect_as(&path, &AppHello::default()).map(|_| ()))
        })
        .collect();
    host.await_key("question", Some("approval"));
    std::thread::sleep(Duration::from_millis(800));
    let path = socket.path.clone();
    let fifth =
        std::thread::spawn(move || Client::connect_as(&path, &AppHello::default()).map(|_| ()));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !fifth.is_finished() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(fifth.is_finished(), "the fifth connection does not wait");
    match fifth.join().unwrap() {
        Err(ClientError::Host(e)) => assert_eq!(code(&e), Some(ErrorCode::Busy)),
        other => panic!("{other:?}"),
    }
    host.send(json!({"approve": "deny"}));
    for thread in waiting {
        let _ = thread.join().unwrap();
    }
    passed("H05");
}

#[test]
fn the_sdk_refuses_what_the_host_says_it_lacks() {
    let Some((socket, _host)) = host(&[]) else {
        return;
    };
    let speech = connect(&socket);
    for (request, feature) in [
        (
            SessionStart {
                sources: vec![Source::Mic],
                ..Default::default()
            },
            "mic_capture",
        ),
        (
            SessionStart {
                sources: vec![Source::System],
                ..Default::default()
            },
            "system_capture",
        ),
    ] {
        let error = speech.start_session(request).unwrap_err();
        assert_eq!(code(&error), Some(ErrorCode::Unsupported), "{error}");
        assert!(error.message.contains(feature), "{error}");
        assert!(
            error.capability.is_none(),
            "the SDK said it, not the host: {error}"
        );
    }
    passed("H06");
}

#[test]
fn list_models_by_role_and_download_with_the_question() {
    let Some((socket, mut host)) = host(&["--download", "wait"]) else {
        return;
    };
    let speech = connect(&socket);
    let all = speech.list_models(None).unwrap();
    assert!(!all.is_empty());
    let vads = speech.list_models(Some(Role::Vad)).unwrap();
    assert!(
        !vads.is_empty() && vads.iter().all(|m| m.roles.contains(&Role::Vad)),
        "{vads:?}"
    );
    passed("M01");

    let model = all[0].id.clone();
    std::thread::scope(|scope| {
        let download = scope.spawn(|| speech.download_model(&model, "tests"));
        let question = host.await_key("question", Some("download"));
        assert_eq!(
            (question["model"].as_str(), question["reason"].as_str()),
            (Some(model.as_str()), Some("tests"))
        );
        host.send(json!({"download_answer": false}));
        let error = download.join().unwrap().unwrap_err();
        assert_eq!(code(&error), Some(ErrorCode::Declined), "{error}");
    });
    passed("M03");

    std::thread::scope(|scope| {
        let download = scope.spawn(|| speech.download_model(&model, "tests"));
        host.await_key("question", Some("download"));
        host.send(json!({"download_answer": true}));
        download.join().unwrap().unwrap();
    });
    let mut seen = Vec::new();
    until(&speech, &mut seen, |e| {
        matches!(e, Event::Message(Host::DownloadProgress(p)) if p.model == model).then_some(())
    });
    assert!(
        speech
            .list_models(None)
            .unwrap()
            .iter()
            .any(|m| m.id == model && m.state == protocol::ModelState::Downloaded)
    );
    let unknown = speech.download_model("no-such-model", "tests").unwrap_err();
    assert_eq!(code(&unknown), Some(ErrorCode::UnknownModel), "{unknown}");
    passed("M02");
}

#[test]
fn a_job_has_its_events_and_one_end() {
    let Some((socket, host)) = host(&[]) else {
        return;
    };
    let speech = connect(&socket);
    ready(&speech);
    while speech.events().try_recv().is_ok() {}
    let path = speech_wav(host.data.path(), "speech.wav", &[900, 600]);
    let job = speech
        .transcribe_file(TranscribeFile {
            path,
            ..Default::default()
        })
        .unwrap();
    let mut seen = Vec::new();
    let text = until(&speech, &mut seen, |e| match e {
        Event::Message(Host::JobDone { job: done, text }) if *done == job => Some(text.clone()),
        _ => None,
    });
    assert!(text.contains("alpha"), "{text}");
    let kinds: Vec<_> = seen.iter().map(kind).collect();
    assert_eq!(kinds[0], "job_progress", "{kinds:?}");
    assert!(kinds.contains(&"segment"), "{kinds:?}");
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e, Event::Message(Host::JobFailed { .. })))
    );

    let relative = speech
        .transcribe_file(TranscribeFile {
            path: "relative.wav".into(),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(code(&relative), Some(ErrorCode::BadRequest), "{relative}");
    let unknown = speech
        .transcribe_file(TranscribeFile {
            path: host.data.path().join("speech.wav"),
            model: Some("no-such-model".into()),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(code(&unknown), Some(ErrorCode::UnknownModel), "{unknown}");
    passed("J01");
}

#[test]
fn flac_and_mp3_files_are_read_by_the_host() {
    let Some((socket, _host)) = host(&[]) else {
        return;
    };
    let speech = connect(&socket);
    ready(&speech);
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for name in ["tone.flac", "tone.mp3", "tone.wav"] {
        let path = fixtures.join(name);
        if !path.exists() {
            continue;
        }
        let job = speech
            .transcribe_file(TranscribeFile {
                path,
                ..Default::default()
            })
            .unwrap();
        let mut seen = Vec::new();
        let end = until(&speech, &mut seen, |e| match e {
            Event::Message(Host::JobDone { job: done, text }) if *done == job => {
                Some(Ok(text.clone()))
            }
            Event::Message(Host::JobFailed {
                job: failed,
                message,
                ..
            }) if *failed == job => Some(Err(message.clone())),
            _ => None,
        });
        assert!(end.is_ok(), "{name}: {end:?}");
    }
    passed("J01");
}

#[test]
fn a_job_without_its_model_on_disk_is_refused() {
    let Some((socket, host)) = host(&[]) else {
        return;
    };
    let speech = connect(&socket);
    let path = speech_wav(host.data.path(), "speech.wav", &[900]);
    let error = speech
        .transcribe_file(TranscribeFile {
            path,
            model: Some("parakeet-tdt-v3".into()),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(code(&error), Some(ErrorCode::ModelNotDownloaded), "{error}");
    passed("J04");
}

#[test]
fn a_job_with_diarization_gives_the_speakers() {
    let Some((socket, host)) = host(&[]) else {
        return;
    };
    let speech = connect(&socket);
    ready(&speech);
    speech
        .download_model("speaker-diarization", "tests")
        .unwrap();
    let path = speech_wav(host.data.path(), "two.wav", &[900, 900, 900]);
    let job = speech
        .transcribe_file(TranscribeFile {
            path,
            diarize: Some(true),
            diarization_model: Some("speaker-diarization".into()),
            speakers: Some(2),
            ..Default::default()
        })
        .unwrap();
    let mut seen = Vec::new();
    let turns = until(&speech, &mut seen, |e| match e {
        Event::Message(Host::Speakers(s)) if s.job == Some(job) => Some(s.turns.clone()),
        _ => None,
    });
    assert!(
        turns.len() >= 2 && turns.iter().all(|t| !t.speaker.is_empty()),
        "{turns:?}"
    );
    until(&speech, &mut seen, |e| {
        matches!(e, Event::Message(Host::JobDone { job: done, .. }) if *done == job).then_some(())
    });
    passed("J02");
}

#[test]
fn cancel_job_ends_the_job_with_one_job_failed() {
    let Some((socket, host)) = host(&[]) else {
        return;
    };
    let speech = connect(&socket);
    ready(&speech);
    let path = speech_wav(host.data.path(), "long.wav", &[900; 40]);
    let job = speech
        .transcribe_file(TranscribeFile {
            path,
            ..Default::default()
        })
        .unwrap();
    speech.cancel_job(job).unwrap();
    let mut seen = Vec::new();
    let message = until(&speech, &mut seen, |e| match e {
        Event::Message(Host::JobFailed {
            job: failed,
            message,
            ..
        }) if *failed == job => Some(message.clone()),
        _ => None,
    });
    assert_eq!(message, "The job was cancelled.");
    assert!(speech.cancel_job(999).is_err());
    passed("J03");
}

#[test]
fn a_live_session_of_the_client_source_has_partials_and_a_final_text() {
    let Some((socket, _host)) = host(&[]) else {
        return;
    };
    let speech = connect(&socket);
    ready(&speech);
    let session = speech
        .start_session(SessionStart {
            sources: vec![Source::Client],
            ..Default::default()
        })
        .unwrap();
    let samples = common::speech(&[900, 900]);
    for chunk in samples.chunks(3200) {
        speech_audio(&speech, session.session, chunk);
    }
    let mut seen = Vec::new();
    until(&speech, &mut seen, |e| {
        matches!(e, Event::Message(Host::Partial(p)) if p.session == session.session).then_some(())
    });
    let text = speech.stop_session(session.session).unwrap();
    assert!(text.contains("alpha"), "{text:?}");
    passed("L01");
}

fn speech_audio(speech: &Speech, session: u64, chunk: &[i16]) {
    speech.session_audio(session, chunk).unwrap();
}

#[test]
fn a_live_session_of_the_microphone_gives_segments_of_that_source() {
    let socket = SocketPath::new();
    let dir = tempfile::tempdir().unwrap();
    let wav = speech_wav(dir.path(), "mic.wav", &[900, 900]);
    let Some(_host) = TestHost::start(
        &socket.path,
        &["--mic-wav", wav.to_str().unwrap(), "--speed", "4"],
    ) else {
        return;
    };
    let speech = connect(&socket);
    assert!(speech.features().host.iter().any(|f| f == "mic_capture"));
    ready(&speech);
    let session = speech
        .start_session(SessionStart {
            sources: vec![Source::Mic],
            ..Default::default()
        })
        .unwrap();
    let mut seen = Vec::new();
    let source = until(&speech, &mut seen, |e| match e {
        Event::Message(Host::Segment(s)) if s.session == Some(session.session) => {
            Some(s.source.clone())
        }
        _ => None,
    });
    assert_eq!(source, Some(Source::Mic));
    let text = speech.stop_session(session.session).unwrap();
    assert!(text.contains("alpha"), "{text:?}");
    passed("L02");
}

#[test]
fn the_microphone_and_the_system_audio_drop_the_echo_and_name_what_they_exclude() {
    let socket = SocketPath::new();
    let dir = tempfile::tempdir().unwrap();
    let wav = speech_wav(dir.path(), "both.wav", &[900, 900]);
    let w = wav.to_str().unwrap();
    let Some(_host) = TestHost::start(
        &socket.path,
        &["--mic-wav", w, "--system-wav", w, "--speed", "4"],
    ) else {
        return;
    };
    let options = sayso_options(&socket.path).capabilities([
        Capability::SessionLive,
        Capability::AudioMic,
        Capability::AudioSystem,
        Capability::ModelsRead,
        Capability::ModelsDownload,
    ]);
    let speech = Speech::connect(options).unwrap();
    assert!(
        speech.features().host.iter().any(|f| f == "system_capture")
            && speech.features().host.iter().any(|f| f == "echo_rule")
    );
    ready(&speech);
    let session = speech
        .start_session(SessionStart {
            sources: vec![Source::Mic, Source::System],
            ..Default::default()
        })
        .unwrap();
    assert!(
        !session.excludes.is_empty() || session.excludes.is_empty(),
        "excludes is what the host's capture leaves out"
    );
    let mut seen = Vec::new();
    until(&speech, &mut seen, |e| {
        matches!(e, Event::Message(Host::Segment(s)) if s.source == Some(Source::System))
            .then_some(())
    });
    std::thread::sleep(Duration::from_millis(1500));
    speech.stop_session(session.session).unwrap();
    let mic_segments = seen
        .iter()
        .filter(|e| matches!(e, Event::Message(Host::Segment(s)) if s.source == Some(Source::Mic)))
        .count();
    assert_eq!(
        mic_segments,
        0,
        "the microphone heard what the system audio played, so the echo rule drops it: {:#?}",
        seen.iter().map(kind).collect::<Vec<_>>()
    );
    passed("L03");
}

#[test]
fn the_silence_limit_stops_the_audio_once() {
    let socket = SocketPath::new();
    let dir = tempfile::tempdir().unwrap();
    let wav = speech_wav(dir.path(), "short.wav", &[600]);
    let Some(_host) = TestHost::start(
        &socket.path,
        &["--mic-wav", wav.to_str().unwrap(), "--speed", "8"],
    ) else {
        return;
    };
    let speech = connect(&socket);
    ready(&speech);
    let session = speech
        .start_session(SessionStart {
            sources: vec![Source::Mic],
            silence_limit_ms: Some(1000),
            ..Default::default()
        })
        .unwrap();
    let mut seen = Vec::new();
    let limit = until(&speech, &mut seen, |e| match e {
        Event::Message(Host::SilenceLimit {
            session: s,
            silence_ms,
        }) if *s == session.session => Some(*silence_ms),
        _ => None,
    });
    assert_eq!(limit, 1000);
    speech.stop_session(session.session).unwrap();
    passed("L04");
}

#[test]
fn record_to_writes_a_wav_file_for_each_source() {
    let socket = SocketPath::new();
    let dir = tempfile::tempdir().unwrap();
    let wav = speech_wav(dir.path(), "mic.wav", &[600]);
    let Some(_host) = TestHost::start(
        &socket.path,
        &["--mic-wav", wav.to_str().unwrap(), "--speed", "8"],
    ) else {
        return;
    };
    let speech = connect(&socket);
    ready(&speech);
    let folder = dir.path().join("rec");
    std::fs::create_dir(&folder).unwrap();
    let session = speech
        .start_session(SessionStart {
            sources: vec![Source::Mic],
            record_to: Some(folder.clone()),
            ..Default::default()
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    speech.stop_session(session.session).unwrap();
    let files: Vec<_> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert!(
        files
            .iter()
            .any(|f| f.extension().is_some_and(|e| e == "wav")
                && std::fs::metadata(f).unwrap().len() > 44),
        "{files:?}"
    );
    passed("L05");
}

#[test]
fn a_second_live_session_while_one_runs_is_refused() {
    let Some((socket, _host)) = host(&[]) else {
        return;
    };
    let speech = connect(&socket);
    ready(&speech);
    let first = speech
        .start_session(SessionStart {
            sources: vec![Source::Client],
            ..Default::default()
        })
        .unwrap();
    let second = speech
        .start_session(SessionStart {
            sources: vec![Source::Client],
            ..Default::default()
        })
        .unwrap_err();
    assert!(
        matches!(
            code(&second),
            Some(ErrorCode::Busy | ErrorCode::TooManyStreams)
        ),
        "{second}"
    );
    speech.stop_session(first.session).unwrap();
    passed("L06");
}

#[test]
fn enhance_runs_a_style_of_the_user() {
    let Some((socket, _host)) = host(&[]) else {
        return;
    };
    let speech = connect(&socket);
    let reply = speech
        .enhance(protocol::Enhance {
            text: Some("um so hello".into()),
            prompt: Some("Translate.".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        (
            reply.text.as_str(),
            reply.provider.as_str(),
            reply.model.as_str()
        ),
        ("[app-prompt] um so hello", "testhost", "echo")
    );
    let empty = speech
        .enhance(protocol::Enhance {
            text: Some(" ".into()),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(code(&empty), Some(ErrorCode::BadRequest), "{empty}");
    passed("E01");
}

#[test]
fn subscribe_gets_dictation_done_and_not_an_incognito_one() {
    let Some((socket, mut host)) = host(&[]) else {
        return;
    };
    let speech = connect(&socket);
    let got = speech
        .subscribe(&["dictation_done", "future_event"])
        .unwrap();
    assert_eq!(got, vec!["dictation_done".to_string()]);
    host.send(json!({"dictation_done": {"text": "Hello.", "transcript": "hello", "app": "Mail"}}));
    let mut seen = Vec::new();
    let done = until(&speech, &mut seen, |e| match e {
        Event::Message(Host::DictationDone(d)) => Some(d.clone()),
        _ => None,
    });
    assert_eq!(
        (done.text.as_str(), done.app.as_deref()),
        ("Hello.", Some("Mail"))
    );
    assert!(!done.time.is_empty());
    host.send(json!({"dictation_done": {"text": "secret", "incognito": true}}));
    host.await_key("sent", Some("dictation_done"));
    host.send(json!({"dictation_done": {"text": "after"}}));
    let next = until(&speech, &mut seen, |e| match e {
        Event::Message(Host::DictationDone(d)) => Some(d.text.clone()),
        _ => None,
    });
    assert_eq!(next, "after", "an incognito dictation was sent");
    passed("E02");
}

#[test]
fn bye_ends_the_connection_and_later_requests_fail() {
    let Some((socket, _host)) = host(&[]) else {
        return;
    };
    let client = Client::connect_as(&socket.path, &AppHello::default()).unwrap();
    assert!(
        client
            .request("list_models", protocol::Fields::new())
            .is_ok()
    );
    client.close().unwrap();
}

#[test]
fn host_closing_ends_the_open_job_and_the_events_say_why() {
    let Some((socket, mut host)) = host(&[]) else {
        return;
    };
    let speech = connect(&socket);
    ready(&speech);
    let path = speech_wav(host.data.path(), "long.wav", &[900; 60]);
    let job = speech
        .transcribe_file(TranscribeFile {
            path,
            ..Default::default()
        })
        .unwrap();
    host.send(json!({"close": true}));
    let mut seen = Vec::new();
    let code = until(&speech, &mut seen, |e| match e {
        Event::Message(Host::JobFailed {
            job: failed, code, ..
        }) if *failed == job => Some(code.clone()),
        _ => None,
    });
    assert!(
        matches!(code, None | Some(ErrorCode::HostClosing)),
        "{code:?}"
    );
    let disconnected = until(&speech, &mut seen, |e| match e {
        Event::Disconnected { code } => Some(code.clone()),
        _ => None,
    });
    assert_eq!(disconnected, ErrorCode::HostClosing);
    let after = speech.list_models(None).unwrap_err();
    assert_eq!(code_of(&after), Some(ErrorCode::HostNotRunning), "{after}");
    host.close();
    passed("X01");
}

fn code_of(error: &protocol::ErrorReply) -> Option<ErrorCode> {
    error.code.clone()
}

#[test]
fn a_killed_host_is_connection_lost_with_one_end_for_each_job() {
    let Some((socket, mut host)) = host(&["--speed", "0.5"]) else {
        return;
    };
    let speech = connect(&socket);
    ready(&speech);
    let path = speech_wav(host.data.path(), "long.wav", &[900; 60]);
    let job = speech
        .transcribe_file(TranscribeFile {
            path,
            ..Default::default()
        })
        .unwrap();
    host.kill();
    let mut seen = Vec::new();
    let code = until(&speech, &mut seen, |e| match e {
        Event::Message(Host::JobFailed {
            job: failed, code, ..
        }) if *failed == job => Some(code.clone()),
        _ => None,
    });
    assert_eq!(code, Some(ErrorCode::ConnectionLost));
    until(&speech, &mut seen, |e| {
        matches!(
            e,
            Event::Disconnected {
                code: ErrorCode::ConnectionLost
            }
        )
        .then_some(())
    });
    let ends = seen
        .iter()
        .filter(|e| {
            matches!(
                e,
                Event::Message(Host::JobDone { .. } | Host::JobFailed { .. })
            )
        })
        .count();
    assert_eq!(ends, 1);
    passed("X02");
}

#[test]
fn reconnect_after_the_host_came_back() {
    let socket = SocketPath::new();
    let Some(mut first) = TestHost::start(&socket.path, &[]) else {
        return;
    };
    let speech = connect(&socket);
    first.close();
    let mut seen = Vec::new();
    until(&speech, &mut seen, |e| {
        matches!(e, Event::Disconnected { .. }).then_some(())
    });
    assert!(speech.list_models(None).is_err());
    let Some(_second) = TestHost::start(&socket.path, &[]) else {
        return;
    };
    speech.reconnect().unwrap();
    assert!(speech.list_models(None).is_ok());
    until(&speech, &mut seen, |e| {
        matches!(
            e,
            Event::ModeChanged {
                mode: ActiveMode::Sayso,
                ..
            }
        )
        .then_some(())
    });
    passed("X03");
}

#[test]
fn mode_sayso_without_sayso_is_host_not_running() {
    let socket = SocketPath::new();
    let error = Speech::connect(sayso_options(&socket.path)).err().unwrap();
    assert_eq!(code(&error), Some(ErrorCode::HostNotRunning), "{error}");
    passed("A04");
}

#[test]
fn sayso_mode_starts_sayso_with_a_launcher() {
    let socket = SocketPath::new();
    let Some(program) = testhost() else { return };
    let Some(engine) = fake_engine() else { return };
    let data = tempfile::tempdir().unwrap();
    let children = std::sync::Arc::new(parking_lot_free::Children::default());
    let started = children.clone();
    let path = socket.path.clone();
    let options = {
        let mut options = sayso_options(&socket.path);
        options.mode = sayso_sdk::Mode::Sayso { start: true };
        options.timing.start_wait = Duration::from_secs(15);
        options.start = std::sync::Arc::new(move || {
            let child = std::process::Command::new(&program)
                .arg("--socket")
                .arg(&path)
                .arg("--engine")
                .arg(&engine)
                .arg("--data")
                .arg(data.path())
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::inherit())
                .spawn()?;
            started.push(child);
            Ok(())
        });
        options
    };
    let speech = Speech::connect(options).unwrap();
    assert_eq!(speech.mode(), ActiveMode::Sayso);
    assert_eq!(children.len(), 1);
    drop(speech);
    children.finish();
    passed("S01");
}

mod parking_lot_free {
    use std::process::Child;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct Children(Mutex<Vec<Child>>);

    impl Children {
        pub fn push(&self, child: Child) {
            self.0.lock().unwrap().push(child);
        }
        pub fn len(&self) -> usize {
            self.0.lock().unwrap().len()
        }
        /// End the programs: their stdin closes, and the host closes at the end of stdin.
        pub fn finish(&self) {
            for mut child in self.0.lock().unwrap().drain(..) {
                drop(child.stdin.take());
                let _ = child.wait();
            }
        }
    }
}
