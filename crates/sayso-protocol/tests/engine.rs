//! Each message of the engine protocol reads from a line in the form of the
//! spec (`docs/engine-protocol.md`) and writes back to the same JSON.

use sayso_protocol::engine::*;
use sayso_protocol::{Decoded, DecodeError, DownloadProgress, ErrorCode, ErrorReply, Kind, Message, ModelState, ModelStateEvent, Role, Segment, SpeechPart, TimestampLevel, Word, decode, encode, parse_ok};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn round_trip<T: Kind + std::fmt::Debug + PartialEq>(line: &str, expected: &T) {
    let Decoded::Known(message) = decode::<T>(line).unwrap_or_else(|e| panic!("{line}: {e}")) else {
        panic!("{line}: the type is unknown");
    };
    assert_eq!(&message.body, expected, "{line}");
    let written = encode(message.id.as_deref(), &message.body);
    let written: Value = serde_json::from_str(&written).unwrap();
    let original: Value = serde_json::from_str(line).unwrap();
    assert_eq!(written, original, "{line}");
}

fn object(value: Value) -> serde_json::Map<String, Value> {
    let Value::Object(map) = value else { panic!("not an object") };
    map
}

fn requests() -> Vec<(&'static str, Request)> {
    let sherpa = object(json!({"kind": "sherpa_vad", "recipe": "silero", "file": "silero_vad.onnx"}));
    vec![
        (r#"{"v":1,"type":"hello","id":"h1"}"#, Request::Hello),
        (r#"{"v":1,"type":"list_models","id":"l1"}"#, Request::ListModels(ListModels { models: None })),
        (
            r#"{"v":1,"type":"list_models","id":"l2","models":[{"id":"silero-vad","engine":{"kind":"sherpa_vad","recipe":"silero","file":"silero_vad.onnx"}},{"id":"parakeet-tdt-v3"}]}"#,
            Request::ListModels(ListModels {
                models: Some(vec![ListedModel { id: "silero-vad".into(), engine: Some(sherpa.clone()) }, ListedModel { id: "parakeet-tdt-v3".into(), engine: None }]),
            }),
        ),
        (
            r#"{"v":1,"type":"download","id":"d1","model":"silero-vad","engine":{"kind":"sherpa_vad","recipe":"silero","file":"silero_vad.onnx"},"size_bytes":2300000}"#,
            Request::Download(Download { model: "silero-vad".into(), engine: Some(sherpa.clone()), size_bytes: Some(2_300_000) }),
        ),
        (r#"{"v":1,"type":"delete","id":"d2","model":"silero-vad"}"#, Request::Delete(ModelRef { model: "silero-vad".into(), engine: None })),
        (r#"{"v":1,"type":"load","id":"l3","model":"parakeet-tdt-v3"}"#, Request::Load(ModelRef { model: "parakeet-tdt-v3".into(), engine: None })),
        (r#"{"v":1,"type":"unload","id":"u1","model":"m1","keep_preview":true}"#, Request::Unload(Unload { model: "m1".into(), keep_preview: Some(true) })),
        (
            r#"{"v":1,"type":"prepare","id":"p1","model":"m1","preview":true,"vocabulary":["Sayso"]}"#,
            Request::Prepare(Prepare { model: "m1".into(), preview: true, vocabulary: vec!["Sayso".into()] }),
        ),
        (
            r#"{"v":1,"type":"transcribe","id":"t1","model":"m1","path":"/tmp/a.wav","language":"auto","vocabulary":[],"segments":true}"#,
            Request::Transcribe(Transcribe { model: "m1".into(), path: "/tmp/a.wav".into(), language: "auto".into(), vocabulary: vec![], segments: Some(true) }),
        ),
        (r#"{"v":1,"type":"stream_start","id":"s1","session":1,"model":"m1","language":"en"}"#, Request::StreamStart(StreamStart { session: 1, model: "m1".into(), language: Some("en".into()) })),
        (r#"{"v":1,"type":"stream_audio","session":1,"pcm":"AQD+/w=="}"#, Request::StreamAudio(StreamAudio { session: 1, pcm: "AQD+/w==".into() })),
        (r#"{"v":1,"type":"stream_end","id":"s2","session":1}"#, Request::StreamEnd(StreamEnd { session: 1 })),
        (r#"{"v":1,"type":"vad","id":"v1","model":"silero-vad","path":"/tmp/a.wav"}"#, Request::Vad(Vad { model: "silero-vad".into(), path: "/tmp/a.wav".into() })),
        (r#"{"v":1,"type":"vad_start","id":"v2","session":2,"model":"silero-vad"}"#, Request::VadStart(VadStart { session: 2, model: "silero-vad".into() })),
        (
            r#"{"v":1,"type":"diarize","id":"z1","model":"speaker-diarization","path":"/tmp/a.wav","speakers":2}"#,
            Request::Diarize(Diarize { model: "speaker-diarization".into(), path: "/tmp/a.wav".into(), speakers: Some(2) }),
        ),
        (r#"{"v":1,"type":"language_model","id":"g0"}"#, Request::LanguageModel),
        (
            r#"{"v":1,"type":"generate","id":"g1","instructions":"Fix the text.","prompt":"helo","timeout_ms":5000,"temperature":0.2}"#,
            Request::Generate(Generate { instructions: "Fix the text.".into(), prompt: "helo".into(), timeout_ms: 5000, temperature: Some(0.2) }),
        ),
        (r#"{"v":1,"type":"shutdown","id":"x1"}"#, Request::Shutdown),
    ]
}

fn replies() -> Vec<(&'static str, Reply)> {
    vec![
        (
            r#"{"v":1,"type":"hello","id":"h1","version":"0.11.0","protocol":1,"features":["keep_preview","unload"]}"#,
            Reply::Hello(Hello { version: "0.11.0".into(), protocol: 1, features: vec!["keep_preview".into(), "unload".into()] }),
        ),
        (r#"{"v":1,"type":"ok","id":"x1"}"#, Reply::Ok(Default::default())),
        (
            r#"{"v":1,"type":"final","id":"t1","text":"Hello there.","elapsed_ms":212,"segments":[{"start_ms":320,"end_ms":1180,"text":"Hello there.","words":[{"start_ms":320,"end_ms":640,"text":"Hello"},{"start_ms":700,"end_ms":1180,"text":"there."}]}]}"#,
            Reply::Final(Final {
                text: "Hello there.".into(),
                elapsed_ms: 212,
                segments: Some(vec![Segment {
                    start_ms: 320,
                    end_ms: 1180,
                    text: "Hello there.".into(),
                    words: Some(vec![Word { start_ms: 320, end_ms: 640, text: "Hello".into() }, Word { start_ms: 700, end_ms: 1180, text: "there.".into() }]),
                }]),
            }),
        ),
        (r#"{"v":1,"type":"final","id":"t2","text":"Hi","elapsed_ms":5}"#, Reply::Final(Final { text: "Hi".into(), elapsed_ms: 5, segments: None })),
        (
            r#"{"v":1,"type":"error","id":"l1","code":"unknown_model","message":"model nope is not in the catalog"}"#,
            Reply::Error(ErrorReply { code: Some(ErrorCode::UnknownModel), message: "model nope is not in the catalog".into(), capability: None, reason: None }),
        ),
        (r#"{"v":1,"type":"error","message":"the line is not JSON"}"#, Reply::Error(ErrorReply { code: None, message: "the line is not JSON".into(), capability: None, reason: None })),
    ]
}

fn events() -> Vec<(&'static str, Event)> {
    vec![
        (
            r#"{"v":1,"type":"model_state","id":"l3","model":"m1","state":"ready","load_ms":812}"#,
            Event::ModelState(ModelStateEvent { model: "m1".into(), state: ModelState::Ready, load_ms: Some(812), preview: None, message: None, reason: None }),
        ),
        (
            r#"{"v":1,"type":"download_progress","id":"d1","model":"m1","fraction":0.5,"bytes_done":5,"bytes_total":10}"#,
            Event::DownloadProgress(DownloadProgress { model: "m1".into(), fraction: 0.5, bytes_done: 5, bytes_total: 10 }),
        ),
        (r#"{"v":1,"type":"part_state","id":"p1","model":"m1","part":"preview","state":"ready"}"#, Event::PartState(PartState { model: "m1".into(), part: "preview".into(), state: "ready".into() })),
        (r#"{"v":1,"type":"partial","id":"s1","session":1,"committed":"Hello","tentative":" there"}"#, Event::Partial(Partial { session: 1, committed: "Hello".into(), tentative: " there".into() })),
        (r#"{"v":1,"type":"speech_start","id":"v2","session":2,"start_ms":300}"#, Event::SpeechStart(SpeechStart { session: 2, start_ms: 300 })),
        (r#"{"v":1,"type":"speech_end","id":"v2","session":2,"start_ms":300,"end_ms":1900}"#, Event::SpeechEnd(SpeechEnd { session: 2, start_ms: 300, end_ms: 1900 })),
        (r#"{"v":1,"type":"log","level":"info","message":"loaded"}"#, Event::Log(Log { level: "info".into(), message: "loaded".into() })),
    ]
}

#[test]
fn every_request_round_trips() {
    let samples = requests();
    for (line, request) in &samples {
        round_trip(line, request);
    }
    let kinds: BTreeSet<_> = samples.iter().map(|(_, r)| r.kind()).collect();
    assert_eq!(kinds, Request::TYPES.iter().copied().collect(), "a sample for each request type");
}

#[test]
fn every_reply_round_trips() {
    let samples = replies();
    for (line, reply) in &samples {
        round_trip(line, reply);
        round_trip(line, &FromEngine::Reply(reply.clone()));
    }
    let kinds: BTreeSet<_> = samples.iter().map(|(_, r)| r.kind()).collect();
    assert_eq!(kinds, Reply::TYPES.iter().copied().collect(), "a sample for each reply type");
}

#[test]
fn every_event_round_trips() {
    let samples = events();
    for (line, event) in &samples {
        round_trip(line, event);
        round_trip(line, &FromEngine::Event(event.clone()));
    }
    let kinds: BTreeSet<_> = samples.iter().map(|(_, e)| e.kind()).collect();
    assert_eq!(kinds, Event::TYPES.iter().copied().collect(), "a sample for each event type");
}

fn ok_fields(line: &str) -> serde_json::Map<String, Value> {
    let Decoded::Known(Message { body: Reply::Ok(fields), .. }) = decode::<Reply>(line).unwrap() else { panic!("{line}: not an ok") };
    fields
}

#[test]
fn the_typed_ok_bodies_read_from_ok_fields() {
    let catalog: ListModelsReply = parse_ok(&ok_fields(
        r#"{"v":1,"type":"ok","id":"l1","models":[{"id":"parakeet-tdt-v3","name":"Parakeet TDT v3","engine":{"kind":"sherpa","recipe":"parakeet_tdt"},"size_bytes":670000000,"languages":["en","de"],"roles":["final","streaming"],"timestamps":"word","recommended":true}],"retired":[{"id":"old","message":"use parakeet-tdt-v3"}]}"#,
    ))
    .unwrap();
    assert_eq!(catalog.models[0].roles, vec![Role::Final, Role::Streaming]);
    assert_eq!(catalog.models[0].timestamps, TimestampLevel::Word);
    assert_eq!(catalog.models[0].engine["kind"], "sherpa");
    assert!(catalog.models[0].recommended);
    assert_eq!(catalog.retired[0].id, "old");

    let empty: ListModelsReply = parse_ok(&ok_fields(r#"{"v":1,"type":"ok","id":"l2"}"#)).unwrap();
    assert!(empty.models.is_empty() && empty.retired.is_empty(), "an ok without the catalog");

    let vad: VadReply = parse_ok(&ok_fields(r#"{"v":1,"type":"ok","id":"v1","speech":[{"start_ms":320,"end_ms":1180}]}"#)).unwrap();
    assert_eq!(vad.speech, vec![SpeechPart { start_ms: 320, end_ms: 1180 }]);

    let diarize: DiarizeReply = parse_ok(&ok_fields(r#"{"v":1,"type":"ok","id":"z1","speakers":2,"turns":[{"start_ms":0,"end_ms":900,"speaker":0},{"start_ms":900,"end_ms":1500,"speaker":1}]}"#)).unwrap();
    assert_eq!(diarize.speakers, 2);
    assert_eq!(diarize.turns[1], SpeakerTurn { start_ms: 900, end_ms: 1500, speaker: 1 });

    let ended: StreamEndReply = parse_ok(&ok_fields(r#"{"v":1,"type":"ok","id":"s2","session":1,"text":"Hello there"}"#)).unwrap();
    assert_eq!(ended, StreamEndReply { session: 1, text: Some("Hello there".into()) });
    let ended: StreamEndReply = parse_ok(&ok_fields(r#"{"v":1,"type":"ok","id":"s3","session":2}"#)).unwrap();
    assert_eq!(ended.text, None, "a VAD session has no text");

    let model: LanguageModelReply = parse_ok(&ok_fields(r#"{"v":1,"type":"ok","id":"g0","available":false,"message":"Apple Intelligence is off","reason":"intelligence_off"}"#)).unwrap();
    assert!(!model.available);
    assert_eq!(model.reason.as_deref(), Some("intelligence_off"));

    let generated: GenerateReply = parse_ok(&ok_fields(r#"{"v":1,"type":"ok","id":"g1","text":"hello","model":"apple"}"#)).unwrap();
    assert_eq!(generated.text, "hello");
}

#[test]
fn an_event_without_an_id_is_known_and_an_unknown_type_is_not() {
    let Decoded::Known(message) = decode::<FromEngine>(r#"{"v":1,"type":"log","level":"warn","message":"slow"}"#).unwrap() else { panic!() };
    assert_eq!(message.id, None);
    let Decoded::Unknown { kind, .. } = decode::<FromEngine>(r#"{"v":1,"type":"cpu_load","value":3}"#).unwrap() else { panic!("a newer event type is unknown") };
    assert_eq!(kind, "cpu_load");
    // Fields that this version does not know do not matter.
    assert!(matches!(decode::<FromEngine>(r#"{"v":1,"type":"partial","session":1,"committed":"","tentative":"","extra":true}"#), Ok(Decoded::Known(_))));
    assert!(matches!(decode::<FromEngine>(r#"{"v":1,"type":"partial","session":"x"}"#), Err(DecodeError::BadFields { .. })));
}
