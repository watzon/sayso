//! Each message of the Host API reads from a line in the form of the docs
//! (`docs/extensions.md`, "Requests") and writes back to the same JSON.

use sayso_protocol::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Decode `line`, check the message and its id, then encode it and compare the
/// JSON with the line.
fn round_trip<T: Kind + std::fmt::Debug + PartialEq>(line: &str, expected: &T) {
    let Decoded::Known(message) = decode::<T>(line).unwrap_or_else(|e| panic!("{line}: {e}"))
    else {
        panic!("{line}: the type is unknown");
    };
    assert_eq!(&message.body, expected, "{line}");
    let written = encode(message.id.as_deref(), &message.body);
    let written: Value = serde_json::from_str(&written).unwrap();
    let original: Value = serde_json::from_str(line).unwrap();
    assert_eq!(written, original, "{line}");
}

fn requests() -> Vec<(&'static str, Request)> {
    vec![
        (
            r#"{"v":1,"type":"hello","id":"h1","sdk":"sayso-extension 0.11.0","protocol":1,"features":[]}"#,
            Request::Hello(Hello {
                sdk: Some("sayso-extension 0.11.0".into()),
                protocol: 1,
                features: vec![],
                app: None,
                capabilities: vec![],
            }),
        ),
        (
            r#"{"v":1,"type":"hello","id":"h2","sdk":"sayso-sdk 0.1.0","protocol":1,"features":[],"app":{"name":"Meeting Recorder"},"capabilities":["session.live","audio.mic","camera.read"]}"#,
            Request::Hello(Hello {
                sdk: Some("sayso-sdk 0.1.0".into()),
                protocol: 1,
                features: vec![],
                app: Some(AppName {
                    name: "Meeting Recorder".into(),
                }),
                capabilities: vec![
                    Capability::SessionLive,
                    Capability::AudioMic,
                    Capability::Other("camera.read".into()),
                ],
            }),
        ),
        (
            r#"{"v":1,"type":"list_models","id":"r1","role":"vad"}"#,
            Request::ListModels(ListModels {
                role: Some(Role::Vad),
            }),
        ),
        (
            r#"{"v":1,"type":"download_model","id":"r2","model":"parakeet-tdt-v3","reason":"Notes in English"}"#,
            Request::DownloadModel(DownloadModel {
                model: "parakeet-tdt-v3".into(),
                reason: "Notes in English".into(),
            }),
        ),
        (
            r#"{"v":1,"type":"transcribe_file","id":"r3","path":"/tmp/talk.m4a","model":"parakeet-tdt-v3","vad_model":"silero-vad","language":"en","diarize":true,"diarization_model":"speaker-diarization","speakers":2}"#,
            Request::TranscribeFile(TranscribeFile {
                path: "/tmp/talk.m4a".into(),
                model: Some("parakeet-tdt-v3".into()),
                vad_model: Some("silero-vad".into()),
                language: Some("en".into()),
                diarize: Some(true),
                diarization_model: Some("speaker-diarization".into()),
                speakers: Some(2),
            }),
        ),
        (
            r#"{"v":1,"type":"cancel_job","id":"r4","job":7}"#,
            Request::CancelJob(CancelJob { job: 7 }),
        ),
        (
            r#"{"v":1,"type":"session_start","id":"r5","sources":["mic","system"],"preview_model":"nemotron","diarize":true,"record_to":"/tmp/call"}"#,
            Request::SessionStart(SessionStart {
                sources: vec![Source::Mic, Source::System],
                preview_model: Some("nemotron".into()),
                diarize: Some(true),
                record_to: Some("/tmp/call".into()),
                ..Default::default()
            }),
        ),
        (
            r#"{"v":1,"type":"session_start","id":"r5b","sources":["mic"],"silence_limit_ms":30000}"#,
            Request::SessionStart(SessionStart {
                sources: vec![Source::Mic],
                silence_limit_ms: Some(30_000),
                ..Default::default()
            }),
        ),
        (
            r#"{"v":1,"type":"session_audio","session":3,"pcm":"AQD+/w=="}"#,
            Request::SessionAudio(SessionAudio {
                session: 3,
                pcm: "AQD+/w==".into(),
            }),
        ),
        (
            r#"{"v":1,"type":"session_stop","id":"r6","session":3}"#,
            Request::SessionStop(SessionStop { session: 3 }),
        ),
        (
            r#"{"v":1,"type":"enhance","id":"r7","text":"um so the plan is","style":"email","provider":"openai"}"#,
            Request::Enhance(Enhance {
                text: Some("um so the plan is".into()),
                style: Some("email".into()),
                provider: Some("openai".into()),
                prompt: None,
            }),
        ),
        (
            r#"{"v":1,"type":"subscribe","id":"r8","events":["dictation_done"]}"#,
            Request::Subscribe(Subscribe {
                events: vec!["dictation_done".into()],
            }),
        ),
        (r#"{"v":1,"type":"bye","id":"r9"}"#, Request::Bye),
    ]
}

fn replies() -> Vec<(&'static str, Reply)> {
    vec![
        (r#"{"v":1,"type":"ok","id":"r9"}"#, Reply::ok()),
        (
            r#"{"v":1,"type":"ok","id":"h1","host_version":"0.11.0","protocol":1,"features":[],"capabilities":["models.read","audio.mic"],"denied":["audio.system"]}"#,
            Reply::ok_with(&HelloReply {
                host_version: "0.11.0".into(),
                protocol: 1,
                features: vec![],
                capabilities: vec![Capability::ModelsRead, Capability::AudioMic],
                denied: vec![Capability::AudioSystem],
            }),
        ),
        (
            r#"{"v":1,"type":"error","id":"r5","code":"capability","message":"the user did not grant audio.system","capability":"audio.system"}"#,
            Reply::Error(ErrorReply {
                code: Some(ErrorCode::Capability),
                message: "the user did not grant audio.system".into(),
                capability: Some(Capability::AudioSystem),
                reason: None,
            }),
        ),
        (
            r#"{"v":1,"type":"error","message":"the line is not JSON"}"#,
            Reply::error(None, "the line is not JSON"),
        ),
    ]
}

fn events() -> Vec<(&'static str, Event)> {
    vec![
        (
            r#"{"v":1,"type":"silence_limit","session":3,"silence_ms":30000}"#,
            Event::SilenceLimit {
                session: 3,
                silence_ms: 30_000,
            },
        ),
        (
            r#"{"v":1,"type":"model_state","id":"r2","model":"parakeet-tdt-v3","state":"ready","load_ms":812}"#,
            Event::ModelState(ModelStateEvent {
                model: "parakeet-tdt-v3".into(),
                state: ModelState::Ready,
                load_ms: Some(812),
                preview: None,
                message: None,
                reason: None,
            }),
        ),
        (
            r#"{"v":1,"type":"download_progress","id":"r2","model":"parakeet-tdt-v3","fraction":0.5,"bytes_done":335000000,"bytes_total":670000000}"#,
            Event::DownloadProgress(DownloadProgress {
                model: "parakeet-tdt-v3".into(),
                fraction: 0.5,
                bytes_done: 335_000_000,
                bytes_total: 670_000_000,
            }),
        ),
        (
            r#"{"v":1,"type":"job_progress","id":"r3","job":7,"fraction":0.25}"#,
            Event::JobProgress {
                job: 7,
                fraction: 0.25,
            },
        ),
        (
            r#"{"v":1,"type":"segment","id":"r3","job":7,"start_ms":320,"end_ms":1180,"text":"Hello there.","words":[{"start_ms":320,"end_ms":640,"text":"Hello"},{"start_ms":700,"end_ms":1180,"text":"there."}]}"#,
            Event::Segment(SegmentEvent {
                job: Some(7),
                session: None,
                source: None,
                segment: Segment {
                    start_ms: 320,
                    end_ms: 1180,
                    text: "Hello there.".into(),
                    words: Some(vec![
                        Word {
                            start_ms: 320,
                            end_ms: 640,
                            text: "Hello".into(),
                        },
                        Word {
                            start_ms: 700,
                            end_ms: 1180,
                            text: "there.".into(),
                        },
                    ]),
                },
            }),
        ),
        (
            r#"{"v":1,"type":"segment","id":"r5","session":3,"source":"system","start_ms":0,"end_ms":900,"text":"Hi all."}"#,
            Event::Segment(SegmentEvent {
                job: None,
                session: Some(3),
                source: Some(Source::System),
                segment: Segment {
                    start_ms: 0,
                    end_ms: 900,
                    text: "Hi all.".into(),
                    words: None,
                },
            }),
        ),
        (
            r#"{"v":1,"type":"speakers","id":"r3","job":7,"turns":[{"start_ms":320,"end_ms":1180,"speaker":"1"},{"start_ms":1900,"end_ms":2950,"speaker":"2"}]}"#,
            Event::Speakers(SpeakersEvent {
                job: Some(7),
                session: None,
                source: None,
                turns: vec![
                    Turn {
                        start_ms: 320,
                        end_ms: 1180,
                        speaker: "1".into(),
                    },
                    Turn {
                        start_ms: 1900,
                        end_ms: 2950,
                        speaker: "2".into(),
                    },
                ],
            }),
        ),
        (
            r#"{"v":1,"type":"job_done","id":"r3","job":7,"text":"Hello there. How are you?"}"#,
            Event::JobDone {
                job: 7,
                text: "Hello there. How are you?".into(),
            },
        ),
        (
            r#"{"v":1,"type":"job_failed","id":"r3","job":7,"message":"model m1 is not on disk","code":"model_not_downloaded"}"#,
            Event::JobFailed {
                job: 7,
                message: "model m1 is not on disk".into(),
                code: Some(ErrorCode::ModelNotDownloaded),
            },
        ),
        (
            r#"{"v":1,"type":"partial","id":"r5","session":3,"source":"mic","committed":"Hello","tentative":" there"}"#,
            Event::Partial(PartialEvent {
                session: 3,
                source: Some(Source::Mic),
                committed: "Hello".into(),
                tentative: " there".into(),
            }),
        ),
        (
            r#"{"v":1,"type":"dictation_done","text":"Hello there.","transcript":"hello there","app":"com.apple.mail","style":"email","time":"2026-10-26T10:00:00Z"}"#,
            Event::DictationDone(DictationDone {
                text: "Hello there.".into(),
                transcript: "hello there".into(),
                app: Some("com.apple.mail".into()),
                site: None,
                style: Some("email".into()),
                time: "2026-10-26T10:00:00Z".into(),
            }),
        ),
        (r#"{"v":1,"type":"host_closing"}"#, Event::HostClosing),
        (
            r#"{"v":1,"type":"approval_waiting"}"#,
            Event::ApprovalWaiting,
        ),
        (
            r#"{"v":1,"type":"session_ended","session":7,"text":"Hello there."}"#,
            Event::SessionEnded {
                session: 7,
                text: "Hello there.".into(),
                code: None,
            },
        ),
        (
            r#"{"v":1,"type":"source_gap","session":7,"source":"system","ms":240}"#,
            Event::SourceGap {
                session: 7,
                source: Source::System,
                ms: 240,
            },
        ),
        (
            r#"{"v":1,"type":"source_ended","session":7,"source":"system","reason":"permission"}"#,
            Event::SourceEnded {
                session: 7,
                source: Source::System,
                reason: SourceEndReason::Permission,
            },
        ),
    ]
}

#[test]
fn every_request_round_trips() {
    let samples = requests();
    for (line, request) in &samples {
        round_trip(line, request);
    }
    let kinds: BTreeSet<_> = samples.iter().map(|(_, r)| r.kind()).collect();
    assert_eq!(
        kinds,
        Request::TYPES.iter().copied().collect(),
        "a sample for each request type"
    );
}

#[test]
fn every_reply_round_trips() {
    let samples = replies();
    for (line, reply) in &samples {
        round_trip(line, reply);
        round_trip(line, &HostMessage::Reply(reply.clone()));
    }
    let kinds: BTreeSet<_> = samples.iter().map(|(_, r)| r.kind()).collect();
    assert_eq!(kinds, Reply::TYPES.iter().copied().collect());
}

#[test]
fn every_event_round_trips() {
    let samples = events();
    for (line, event) in &samples {
        round_trip(line, event);
        round_trip(line, &HostMessage::Event(event.clone()));
    }
    let kinds: BTreeSet<_> = samples.iter().map(|(_, e)| e.kind()).collect();
    assert_eq!(
        kinds,
        Event::TYPES.iter().copied().collect(),
        "a sample for each event type"
    );
}

#[test]
fn the_typed_ok_bodies_read_from_ok_fields() {
    let Decoded::Known(Message { body: Reply::Ok(fields), .. }) = decode::<Reply>(
        r#"{"v":1,"type":"ok","id":"r1","models":[{"id":"silero-vad","name":"Silero VAD","roles":["vad"],"state":"downloaded","languages":[],"timestamps":"none","size_bytes":2300000,"choice":["vad"]}]}"#,
    )
    .unwrap() else {
        panic!("not ok")
    };
    let reply: ListModelsReply = parse_ok(&fields).unwrap();
    assert_eq!(
        reply.models,
        vec![ModelInfo {
            id: "silero-vad".into(),
            name: "Silero VAD".into(),
            roles: vec![Role::Vad],
            state: ModelState::Downloaded,
            languages: vec![],
            timestamps: TimestampLevel::None,
            size_bytes: 2_300_000,
            choice: vec![Role::Vad],
        }]
    );
    let job: JobReply = parse_ok(&Reply::ok_with(&JobReply { job: 7 }).ok_fields()).unwrap();
    assert_eq!(job.job, 7);
    for (body, expected) in [
        (
            Reply::ok_with(&SessionReply {
                session: 3,
                excludes: vec!["sayso".into(), "app".into()],
            }),
            json!({"session": 3, "excludes": ["sayso", "app"]}),
        ),
        (
            Reply::ok_with(&TextReply { text: "Hi.".into() }),
            json!({"text": "Hi."}),
        ),
        (
            Reply::ok_with(&EnhanceReply {
                text: "Hi.".into(),
                provider: "openai".into(),
                model: "gpt".into(),
            }),
            json!({"text": "Hi.", "provider": "openai", "model": "gpt"}),
        ),
        (
            Reply::ok_with(&SubscribeReply {
                events: vec!["dictation_done".into()],
            }),
            json!({"events": ["dictation_done"]}),
        ),
    ] {
        assert_eq!(Value::Object(body.ok_fields()), expected);
    }
}

trait OkFields {
    fn ok_fields(self) -> Fields;
}

impl OkFields for Reply {
    fn ok_fields(self) -> Fields {
        match self {
            Reply::Ok(fields) => fields,
            Reply::Error(e) => panic!("{e}"),
        }
    }
}

#[test]
fn a_newer_peer_does_not_break_the_reader() {
    // An unknown type, with its id and fields.
    let decoded =
        decode::<HostMessage>(r#"{"v":1,"type":"meeting_found","id":"r3","title":"Standup"}"#)
            .unwrap();
    assert_eq!(
        decoded,
        Decoded::Unknown {
            id: Some("r3".into()),
            kind: "meeting_found".into(),
            fields: json!({"title": "Standup"}).as_object().unwrap().clone()
        }
    );
    assert!(matches!(
        decode::<Request>(r#"{"v":1,"type":"translate","id":"r1"}"#).unwrap(),
        Decoded::Unknown { .. }
    ));
    // Unknown fields and unknown names.
    let Decoded::Known(message) = decode::<HostMessage>(
        r#"{"v":1,"type":"error","id":"r1","code":"too_tired","message":"later","capability":"camera.read","extra":{"a":1}}"#,
    )
    .unwrap() else {
        panic!("error is known")
    };
    assert_eq!(
        message.body,
        HostMessage::Reply(Reply::Error(ErrorReply {
            code: Some(ErrorCode::Other("too_tired".into())),
            message: "later".into(),
            capability: Some(Capability::Other("camera.read".into())),
            reason: None,
        }))
    );
    let Decoded::Known(message) = decode::<HostMessage>(
        r#"{"v":1,"type":"ok","id":"h1","host_version":"0.12.0","protocol":1,"features":["jobs"],"capabilities":["models.read","camera.read"],"motd":"hi"}"#,
    )
    .unwrap() else {
        panic!("ok is known")
    };
    let HostMessage::Reply(Reply::Ok(fields)) = message.body else {
        panic!("not ok")
    };
    let hello: HelloReply = parse_ok(&fields).unwrap();
    assert_eq!(
        hello.capabilities,
        vec![
            Capability::ModelsRead,
            Capability::Other("camera.read".into())
        ]
    );
    assert!(matches!(
        decode::<Event>(r#"{"v":1,"type":"host_closing","why":"update"}"#).unwrap(),
        Decoded::Known(Message {
            body: Event::HostClosing,
            ..
        })
    ));
}

#[test]
fn a_line_that_is_not_a_message_is_an_error_without_id() {
    for line in [
        "",
        "hello",
        "[1,2]",
        r#"{"v":1,"id":"r1"}"#,
        r#"{"v":1,"type":3,"id":"r1"}"#,
    ] {
        assert!(
            matches!(decode::<Request>(line), Err(DecodeError::NotAMessage(_))),
            "{line}"
        );
    }
}

#[test]
fn a_known_type_with_bad_fields_keeps_its_id() {
    let error =
        decode::<Request>(r#"{"v":1,"type":"cancel_job","id":"r4","job":"seven"}"#).unwrap_err();
    let DecodeError::BadFields { id, kind, .. } = error else {
        panic!("{error:?}")
    };
    assert_eq!((id.as_deref(), kind.as_str()), (Some("r4"), "cancel_job"));
    assert!(
        matches!(
            decode::<Request>(r#"{"v":1,"type":"hello","id":"h1"}"#),
            Err(DecodeError::BadFields { .. })
        ),
        "hello needs protocol"
    );
}

#[test]
fn engine_speakers_are_numbers_and_host_speakers_are_labels() {
    let turn: Turn =
        serde_json::from_value(json!({"start_ms": 0, "end_ms": 10, "speaker": 0})).unwrap();
    assert_eq!(turn.speaker, "0");
    assert_eq!(serde_json::to_value(&turn).unwrap()["speaker"], json!("0"));
    let part: SpeechPart = serde_json::from_value(json!({"start_ms": 5, "end_ms": 9})).unwrap();
    assert_eq!(
        part,
        SpeechPart {
            start_ms: 5,
            end_ms: 9
        }
    );
}

#[test]
fn requests_need_the_capabilities_of_the_table() {
    let needs = |line: &str| match decode::<Request>(line).unwrap() {
        Decoded::Known(m) => m.body.capabilities(),
        Decoded::Unknown { .. } => panic!("{line}"),
    };
    assert_eq!(needs(r#"{"type":"hello","protocol":1}"#), vec![]);
    assert_eq!(needs(r#"{"type":"bye"}"#), vec![]);
    assert_eq!(
        needs(r#"{"type":"list_models"}"#),
        vec![Capability::ModelsRead]
    );
    assert_eq!(
        needs(r#"{"type":"download_model","model":"m","reason":"r"}"#),
        vec![Capability::ModelsDownload]
    );
    assert_eq!(needs(r#"{"type":"cancel_job","job":1}"#), vec![]);
    assert_eq!(needs(r#"{"type":"session_stop","session":1}"#), vec![]);
    assert_eq!(
        needs(r#"{"type":"session_audio","session":1,"pcm":""}"#),
        vec![Capability::SessionLive]
    );
    assert_eq!(
        needs(r#"{"type":"session_start","sources":["client"]}"#),
        vec![Capability::SessionLive]
    );
    assert_eq!(
        needs(r#"{"type":"session_start","sources":["mic","system"]}"#),
        vec![
            Capability::SessionLive,
            Capability::AudioMic,
            Capability::AudioSystem
        ]
    );
    assert_eq!(
        needs(r#"{"type":"enhance","prompt":"p"}"#),
        vec![Capability::AiEnhance]
    );
    assert_eq!(
        needs(r#"{"type":"subscribe","events":[]}"#),
        vec![Capability::DictationObserve]
    );
}

#[test]
fn capabilities_and_codes_are_the_names_of_the_docs() {
    let names: Vec<_> = Capability::ALL.iter().map(Capability::as_str).collect();
    assert_eq!(
        names,
        [
            "models.read",
            "models.download",
            "transcribe.file",
            "session.live",
            "audio.mic",
            "audio.system",
            "ai.enhance",
            "dictation.observe"
        ]
    );
    for code in [
        "not_approved",
        "capability",
        "busy",
        "declined",
        "model_not_downloaded",
        "wrong_role",
        "unsupported",
        "bad_request",
        "too_many_streams",
        "unknown_model",
        "model_retired",
    ] {
        assert!(
            !matches!(ErrorCode::parse(code), ErrorCode::Other(_)),
            "{code}"
        );
    }
}

#[test]
fn every_written_message_has_the_version() {
    let line = encode(Some("e1"), &Reply::error(Some(ErrorCode::BadRequest), "no"));
    let value: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["v"], json!(1));
    assert_eq!(value["id"], json!("e1"));
    assert!(!line.contains('\n'));
    let value: Value = serde_json::from_str(&encode(None, &Event::HostClosing)).unwrap();
    assert_eq!(value, json!({"v": 1, "type": "host_closing"}));
}

#[test]
fn a_partial_joins_its_two_texts_with_one_space_where_needed() {
    let text = |committed: &str, tentative: &str| {
        PartialEvent {
            session: 1,
            source: None,
            committed: committed.into(),
            tentative: tentative.into(),
        }
        .text()
    };
    assert_eq!(text("hello", "world"), "hello world");
    assert_eq!(text("hello ", "world"), "hello world");
    assert_eq!(text("hello", " world"), "hello world");
    assert_eq!(text("hello  ", "  world"), "hello world");
    assert_eq!(text("", "world"), "world");
    assert_eq!(text("hello ", ""), "hello");
    assert_eq!(text(" ", " "), "");
    assert_eq!(text("hello", ", world"), "hello, world");
    assert_eq!(text("is it", "?"), "is it?");
    assert_eq!(text("今天", "天气"), "今天天气");
    assert_eq!(text("hello", "世界"), "hello 世界");
}
