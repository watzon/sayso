//! Writes real messages, made by `encode`, as typed TypeScript constants to
//! `packages/sdk/tests/types/wire.ts`. `tsc` checks them against the generated
//! types (`pnpm test:types` in packages/sdk), so a type that does not match
//! what Rust writes fails there. `scripts/check-protocol-ts.sh` checks that the
//! file is current.
#![cfg(feature = "ts")]

use sayso_protocol::engine;
use sayso_protocol::*;
use std::path::Path;

type Rows = Vec<(&'static str, String)>;

fn line<T: Kind>(rows: &mut Rows, ty: &'static str, id: Option<&str>, body: &T) {
    rows.push((ty, encode(id, body)));
}

fn rows() -> Rows {
    let mut rows: Rows = Vec::new();
    // Events
    line(
        &mut rows,
        "Event",
        None,
        &Event::JobDone {
            job: 7,
            text: "hi there".into(),
        },
    );
    line(
        &mut rows,
        "Event",
        Some("r1"),
        &Event::JobProgress {
            job: 7,
            fraction: 0.5,
        },
    );
    line(
        &mut rows,
        "Event",
        None,
        &Event::JobFailed {
            job: 2,
            message: "boom".into(),
            code: None,
        },
    );
    line(
        &mut rows,
        "Event",
        None,
        &Event::JobFailed {
            job: 2,
            message: "boom".into(),
            code: Some(ErrorCode::Other("future_code".into())),
        },
    );
    line(
        &mut rows,
        "Event",
        None,
        &Event::SessionEnded {
            session: 3,
            text: "all".into(),
            code: Some(ErrorCode::ConnectionLost),
        },
    );
    line(&mut rows, "Event", None, &Event::ApprovalWaiting);
    line(&mut rows, "Event", None, &Event::HostClosing);
    line(
        &mut rows,
        "Event",
        Some("job1"),
        &Event::Segment(SegmentEvent {
            job: Some(1),
            session: None,
            source: None,
            segment: Segment {
                start_ms: 0,
                end_ms: 1200,
                text: "hello".into(),
                words: Some(vec![Word {
                    start_ms: 0,
                    end_ms: 600,
                    text: "hel".into(),
                }]),
            },
        }),
    );
    line(
        &mut rows,
        "Event",
        None,
        &Event::Segment(SegmentEvent {
            job: None,
            session: Some(9),
            source: Some(Source::Mic),
            segment: Segment {
                start_ms: 5,
                end_ms: 6,
                text: "x".into(),
                words: None,
            },
        }),
    );
    line(
        &mut rows,
        "Event",
        None,
        &Event::Speakers(SpeakersEvent {
            job: Some(1),
            session: None,
            source: None,
            turns: vec![Turn {
                start_ms: 0,
                end_ms: 10,
                speaker: "1".into(),
            }],
        }),
    );
    line(
        &mut rows,
        "Event",
        None,
        &Event::SourceEnded {
            session: 1,
            source: Source::Other("tap".into()),
            reason: SourceEndReason::Failed,
        },
    );
    line(
        &mut rows,
        "Event",
        None,
        &Event::ModelState(ModelStateEvent {
            model: "m".into(),
            state: ModelState::Ready,
            load_ms: Some(120),
            preview: None,
            message: None,
            reason: None,
        }),
    );
    line(
        &mut rows,
        "Event",
        None,
        &Event::Partial(PartialEvent {
            session: 1,
            source: None,
            committed: "a".into(),
            tentative: "b".into(),
        }),
    );
    line(
        &mut rows,
        "Event",
        None,
        &Event::DictationDone(DictationDone {
            text: "t".into(),
            transcript: "t".into(),
            app: None,
            site: None,
            style: Some("formal".into()),
            time: "2026-11-08T00:00:00Z".into(),
        }),
    );
    // Requests
    line(
        &mut rows,
        "Request",
        Some("q1"),
        &Request::TranscribeFile(TranscribeFile {
            path: "/tmp/a.wav".into(),
            ..Default::default()
        }),
    );
    line(
        &mut rows,
        "Request",
        Some("q2"),
        &Request::SessionStart(SessionStart {
            sources: vec![Source::Mic, Source::System],
            silence_limit_ms: Some(5000),
            ..Default::default()
        }),
    );
    line(
        &mut rows,
        "Request",
        Some("q3"),
        &Request::Hello(Hello {
            sdk: None,
            protocol: 1,
            features: vec![],
            app: Some(AppName {
                name: "demo".into(),
            }),
            capabilities: vec![Capability::SessionLive, Capability::Other("x.y".into())],
        }),
    );
    line(&mut rows, "Request", None, &Request::Bye);
    // Replies
    line(
        &mut rows,
        "Reply",
        Some("q1"),
        &Reply::error(Some(ErrorCode::Capability), "no grant"),
    );
    line(&mut rows, "Reply", Some("q1"), &Reply::ok());
    line(
        &mut rows,
        "Reply",
        Some("q3"),
        &Reply::ok_with(&HelloReply {
            host_version: "0.15.0".into(),
            protocol: 1,
            features: vec!["f".into()],
            capabilities: vec![Capability::ModelsRead],
            denied: vec![],
        }),
    );
    line(
        &mut rows,
        "HostMessage",
        None,
        &HostMessage::Event(Event::JobDone {
            job: 1,
            text: "z".into(),
        }),
    );
    line(
        &mut rows,
        "HostMessage",
        Some("q1"),
        &HostMessage::Reply(Reply::error(None, "bad")),
    );
    let lm = ListModelsReply {
        models: vec![ModelInfo {
            id: "a".into(),
            name: "A".into(),
            roles: vec![Role::Final],
            state: ModelState::Downloaded,
            languages: vec!["en".into()],
            timestamps: TimestampLevel::Word,
            size_bytes: 123456789012,
            choice: vec![],
        }],
    };
    rows.push(("ListModelsReply", serde_json::to_string(&lm).unwrap()));
    line(&mut rows, "Reply", Some("q4"), &Reply::ok_with(&lm));
    // Engine
    line(
        &mut rows,
        "EngineRequest",
        Some("t1"),
        &engine::Request::Transcribe(engine::Transcribe {
            model: "m".into(),
            path: "/tmp/talk.wav".into(),
            language: "auto".into(),
            vocabulary: vec![],
            segments: None,
        }),
    );
    line(&mut rows, "EngineRequest", None, &engine::Request::Shutdown);
    line(
        &mut rows,
        "EngineFromEngine",
        Some("t1"),
        &engine::FromEngine::Reply(engine::Reply::Final(engine::Final {
            text: "t".into(),
            elapsed_ms: 5,
            segments: None,
            language: Some("de".into()),
        })),
    );
    line(
        &mut rows,
        "EngineFromEngine",
        None,
        &engine::FromEngine::Event(engine::Event::SpeechEnd(engine::SpeechEnd {
            session: 1,
            start_ms: 2,
            end_ms: 3,
        })),
    );
    // Name types alone
    rows.push((
        "Capability",
        serde_json::to_string(&Capability::AudioMic).unwrap(),
    ));
    rows.push((
        "ErrorCode",
        serde_json::to_string(&ErrorCode::Other("brand_new".into())).unwrap(),
    ));
    rows
}

#[test]
fn write_wire_fixture() {
    let mut out = String::new();
    out.push_str(
        "// This file was generated by crates/sayso-protocol/tests/ts_wire.rs. Do not edit it.\n",
    );
    out.push_str(
        "// Each value is a message that Rust wrote. tsc checks it against the generated types.\n",
    );
    out.push_str("import type {\n    Capability,\n    ErrorCode,\n    Event,\n    HostMessage,\n    ListModelsReply,\n    Reply,\n    Request,\n    engine,\n} from \"../../src/protocol/index.js\";\n\n");
    out.push_str("// The envelope that `encode` adds to every message.\n");
    out.push_str("type Wire<T> = T & { v: 1; id?: string };\n\n");
    for (i, (ty, json)) in rows().iter().enumerate() {
        let declared = match *ty {
            "Capability" | "ErrorCode" | "ListModelsReply" => (*ty).to_owned(),
            "EngineRequest" => "Wire<engine.Request>".to_owned(),
            "EngineFromEngine" => "Wire<engine.FromEngine>".to_owned(),
            other => format!("Wire<{other}>"),
        };
        out.push_str(&format!("export const wire{i}: {declared} = {json};\n"));
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/sdk/tests/types/wire.ts");
    std::fs::write(&path, out).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}
